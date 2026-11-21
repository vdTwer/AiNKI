//! Settings and FSRS parameters.

#[allow(clippy::wildcard_imports, reason = "an area of the service sees the service's shared imports and types")]
use super::*;

/// The settings page's fields.
#[derive(Debug, Clone)]
pub struct Settings<'a> {
    pub timezone: &'a str,
    pub rollover_hour: i64,
    pub four_buttons: bool,
    pub font_scale: i64,
    pub optimize_include_agent: bool,
    pub desktop_look: &'a str,
}

impl App {
    /// # Errors
    ///
    /// - `BadRequest` if the look or the time zone is unknown.
    /// - `Internal` if the database fails.
    pub async fn update_settings(&self, user_id: i64, settings: &Settings<'_>) -> Result<()> {
        let Settings { timezone, rollover_hour, four_buttons, font_scale, optimize_include_agent, desktop_look } =
            *settings;
        if !matches!(desktop_look, "calm" | "simple") {
            return Err(AppError::bad("unknown look"));
        }
        timezone.parse::<chrono_tz::Tz>().map_err(|_| AppError::bad(format!("unknown timezone '{timezone}'")))?;
        sqlx::query(
            "UPDATE users SET timezone = ?, rollover_hour = ?, four_buttons = ?, font_scale = ?,
                optimize_include_agent = ?, desktop_look = ? WHERE id = ?",
        )
        .bind(timezone)
        .bind(rollover_hour.clamp(0, 23))
        .bind(four_buttons)
        .bind(font_scale.clamp(80, 200))
        .bind(optimize_include_agent)
        .bind(desktop_look)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only or limited to some decks.
    /// - `Internal` if the database fails.
    pub async fn reset_parameters(&self, access: &Access) -> Result<()> {
        access.require_write()?;
        access.require_all_decks()?;
        sqlx::query("UPDATE users SET fsrs_params = '[]', fsrs_params_updated_at = NULL WHERE id = ?")
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }
}
