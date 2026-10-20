//! Image uploads. Every image is converted to a small grayscale PNG dithered to
//! 16 gray levels, which is what most e-ink panels can show, so pictures look
//! the same on a Kindle-style screen as on a phone.

use std::path::PathBuf;

use image::{GrayImage, ImageReader, Luma, imageops::FilterType};
use serde::Serialize;

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    now,
};

pub const MAX_UPLOAD_BYTES: usize = 15 * 1024 * 1024;
const MAX_DIMENSION: u32 = 1200;
const GRAY_LEVELS: u8 = 16;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Media {
    pub id: String,
    #[serde(skip)]
    pub user_id: i64,
    pub mime: String,
    pub width: i64,
    pub height: i64,
    pub bytes: i64,
    pub created_at: i64,
}

/// Floyd–Steinberg error diffusion to `levels` evenly spaced grays.
fn dither(img: &mut GrayImage, levels: u8) {
    let (w, h) = img.dimensions();
    let (w, h) = (w as usize, h as usize);
    let step = 255.0 / (levels - 1) as f32;
    let mut buf: Vec<f32> = img.pixels().map(|p| p.0[0] as f32).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let old = buf[i].clamp(0.0, 255.0);
            let new = (old / step).round() * step;
            buf[i] = new;
            let err = old - new;
            if x + 1 < w {
                buf[i + 1] += err * 7.0 / 16.0;
            }
            if y + 1 < h {
                if x > 0 {
                    buf[i + w - 1] += err * 3.0 / 16.0;
                }
                buf[i + w] += err * 5.0 / 16.0;
                if x + 1 < w {
                    buf[i + w + 1] += err * 1.0 / 16.0;
                }
            }
        }
    }
    for (p, v) in img.pixels_mut().zip(buf) {
        *p = Luma([v.clamp(0.0, 255.0) as u8]);
    }
}

/// Decode, downscale, grayscale, dither and re-encode as PNG.
pub fn process_image(data: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let reader = ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| AppError::bad("could not read image"))?;
    let img = reader
        .decode()
        .map_err(|e| AppError::bad(format!("unsupported or broken image: {e}")))?;
    let img = if img.width() > MAX_DIMENSION || img.height() > MAX_DIMENSION {
        img.resize(MAX_DIMENSION, MAX_DIMENSION, FilterType::Lanczos3)
    } else {
        img
    };
    // Flatten transparency onto white before converting.
    let rgba = img.to_rgba8();
    let mut gray = GrayImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let a = a as f32 / 255.0;
        let l = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        gray.put_pixel(x, y, Luma([(l * a + 255.0 * (1.0 - a)).round() as u8]));
    }
    dither(&mut gray, GRAY_LEVELS);
    let mut out = Vec::new();
    gray.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| AppError::Internal(e.into()))?;
    Ok((out, gray.width(), gray.height()))
}

impl App {
    pub fn media_path(&self, id: &str) -> PathBuf {
        self.config.data_dir.join("media").join(format!("{id}.png"))
    }

    pub async fn store_image(&self, access: &Access, data: Vec<u8>) -> Result<Media> {
        access.require_write()?;
        if data.len() > MAX_UPLOAD_BYTES {
            return Err(AppError::bad("image is larger than 15 MB"));
        }
        self.store_image_for(access.user_id(), data).await
    }

    pub(crate) async fn store_image_for(&self, user_id: i64, data: Vec<u8>) -> Result<Media> {
        let (png, w, h) = tokio::task::spawn_blocking(move || process_image(&data))
            .await
            .map_err(|e| AppError::Internal(e.into()))??;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let path = self.media_path(&id);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, &png).await?;
        sqlx::query(
            "INSERT INTO media (id, user_id, mime, width, height, bytes, created_at)
             VALUES (?, ?, 'image/png', ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(w as i64)
        .bind(h as i64)
        .bind(png.len() as i64)
        .bind(now())
        .execute(&self.db)
        .await?;
        Ok(sqlx::query_as("SELECT * FROM media WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.db)
            .await?)
    }

    pub async fn media(&self, user_id: i64, id: &str) -> Result<(Media, Vec<u8>)> {
        let m: Media = sqlx::query_as("SELECT * FROM media WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_one(&self.db)
            .await?;
        let bytes = tokio::fs::read(self.media_path(&m.id))
            .await
            .map_err(|_| AppError::NotFound)?;
        Ok((m, bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processes_color_png_to_gray_levels() {
        let mut img = image::RgbaImage::new(2000, 1000);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, 80, 200, 255]);
        }
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let (out, w, h) = process_image(&png).unwrap();
        assert_eq!((w, h), (1200, 600));
        let decoded = image::load_from_memory(&out).unwrap().to_luma8();
        let step = 255 / (GRAY_LEVELS as u32 - 1);
        assert!(decoded.pixels().all(|p| p.0[0] as u32 % step == 0));
    }

    #[test]
    fn rejects_garbage() {
        assert!(process_image(b"not an image").is_err());
    }
}
