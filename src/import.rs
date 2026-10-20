//! Import from CSV and Anki packages (.apkg/.colpkg), export to CSV.
//!
//! Anki notes are flattened to plain front/back cards: the first two fields
//! are used, HTML is stripped, the first image of each side is kept, and
//! cloze notes become one card per cloze number. Review history is imported
//! too, so FSRS memory states (and later the optimizer) can use it.

use std::{
    collections::HashMap,
    io::{Cursor, Read},
    str::FromStr,
};

use serde::Serialize;
use sqlx::{Row, sqlite::SqliteConnectOptions};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    now,
    scheduler::{self, CardState},
};

pub const MAX_IMPORT_BYTES: usize = 300 * 1024 * 1024;

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub decks_created: Vec<String>,
    pub cards_imported: i64,
    pub reviews_imported: i64,
    pub images_imported: i64,
    pub skipped: i64,
}

// ----- CSV ----------------------------------------------------------------

fn sniff_delimiter(text: &str) -> u8 {
    let first = text.lines().next().unwrap_or("");
    [b'\t', b';', b',']
        .into_iter()
        .max_by_key(|d| first.matches(*d as char).count())
        .filter(|d| first.contains(*d as char))
        .unwrap_or(b',')
}

/// Parse CSV/TSV text into (front, back) pairs. A header row named
/// front/back (or question/answer) is skipped.
pub fn parse_csv(text: &str) -> Result<Vec<(String, String)>> {
    let text = text.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(sniff_delimiter(text))
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut out = Vec::new();
    for (i, rec) in reader.records().enumerate() {
        let rec = rec.map_err(|e| AppError::bad(format!("CSV error: {e}")))?;
        let front = rec.get(0).unwrap_or("").trim().to_string();
        let back = rec.get(1).unwrap_or("").trim().to_string();
        if i == 0
            && matches!(front.to_lowercase().as_str(), "front" | "question")
            && matches!(back.to_lowercase().as_str(), "back" | "answer")
        {
            continue;
        }
        if front.is_empty() && back.is_empty() {
            continue;
        }
        out.push((front, back));
    }
    Ok(out)
}

fn csv_escape(rows: &[(String, String)]) -> Result<String> {
    let mut w = csv::Writer::from_writer(vec![]);
    w.write_record(["front", "back"]).map_err(|e| AppError::Internal(e.into()))?;
    for (f, b) in rows {
        w.write_record([f, b]).map_err(|e| AppError::Internal(e.into()))?;
    }
    String::from_utf8(w.into_inner().map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?)
        .map_err(|e| AppError::Internal(e.into()))
}

// ----- Anki HTML ----------------------------------------------------------

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let ent = &rest[1..e];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            ch.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Strip Anki field HTML to text, returning the text and the first image.
pub fn anki_html_to_text(html: &str) -> (String, Option<String>) {
    let mut image = None;
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = &rest[start + 1..start + end];
        let lower = tag.to_ascii_lowercase();
        let name = lower
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        match name {
            "br" | "div" | "p" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" => {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
            }
            "img" if image.is_none() => {
                if let Some(pos) = lower.find("src=") {
                    let val = &tag[pos + 4..];
                    let src = match val.chars().next() {
                        Some(q @ ('"' | '\'')) => val[1..].split(q).next().unwrap_or(""),
                        _ => val.split_whitespace().next().unwrap_or(""),
                    };
                    if !src.is_empty() {
                        image = Some(decode_entities(src));
                    }
                }
            }
            _ => {}
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let mut text = decode_entities(&out);
    // Drop [sound:...] tags; audio isn't supported.
    while let Some(s) = text.find("[sound:") {
        match text[s..].find(']') {
            Some(e) => text.replace_range(s..s + e + 1, ""),
            None => break,
        }
    }
    let text = text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (text, image)
}

/// Render a cloze field: `hide` = Some(n) blanks out cloze n, None reveals all.
fn render_cloze(text: &str, hide: Option<u32>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{c") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let num_len = after.chars().take_while(|c| c.is_ascii_digit()).count();
        let num: Option<u32> = after[..num_len].parse().ok();
        let body_start = num_len + 2;
        if num.is_none() || !after[num_len..].starts_with("::") {
            out.push_str("{{c");
            rest = after;
            continue;
        }
        let Some(end) = after[body_start..].find("}}") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let body = &after[body_start..body_start + end];
        let (answer, hint) = match body.split_once("::") {
            Some((a, h)) => (a, Some(h)),
            None => (body, None),
        };
        if hide == num {
            out.push_str(&match hint {
                Some(h) => format!("**[{h}]**"),
                None => "**[…]**".into(),
            });
        } else if hide.is_none() {
            out.push_str(&format!("**{answer}**"));
        } else {
            out.push_str(answer);
        }
        rest = &after[body_start + end + 2..];
    }
    out.push_str(rest);
    out
}

fn cloze_numbers(text: &str) -> Vec<u32> {
    let mut nums: Vec<u32> = text
        .match_indices("{{c")
        .filter_map(|(i, _)| {
            let after = &text[i + 3..];
            let n = after.chars().take_while(|c| c.is_ascii_digit()).count();
            if after[n..].starts_with("::") { after[..n].parse().ok() } else { None }
        })
        .collect();
    nums.sort_unstable();
    nums.dedup();
    nums
}

// ----- Anki package -------------------------------------------------------

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Parse the protobuf `MediaEntries` message used by newer .apkg files.
fn parse_media_entries(buf: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut pos = 0;
    while pos < buf.len() {
        let Some(key) = read_varint(buf, &mut pos) else { break };
        if key & 7 != 2 {
            break;
        }
        let Some(len) = read_varint(buf, &mut pos) else { break };
        let end = pos + len as usize;
        let Some(entry) = buf.get(pos..end) else { break };
        pos = end;
        // Inside MediaEntry, field 1 is the file name.
        let mut p = 0;
        let mut name = String::new();
        while p < entry.len() {
            let Some(k) = read_varint(entry, &mut p) else { break };
            match k & 7 {
                0 => {
                    read_varint(entry, &mut p);
                }
                2 => {
                    let Some(l) = read_varint(entry, &mut p) else { break };
                    let e = p + l as usize;
                    if k >> 3 == 1 {
                        name = String::from_utf8_lossy(entry.get(p..e).unwrap_or_default()).into();
                    }
                    p = e;
                }
                _ => break,
            }
        }
        names.push(name);
    }
    names
}

fn maybe_zstd(data: Vec<u8>) -> Vec<u8> {
    if data.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        zstd::decode_all(Cursor::new(&data)).unwrap_or(data)
    } else {
        data
    }
}

struct Package {
    collection: Vec<u8>,
    /// media file name -> bytes
    media: HashMap<String, Vec<u8>>,
}

fn read_package(data: &[u8]) -> Result<Package> {
    let mut zip = zip::ZipArchive::new(Cursor::new(data))
        .map_err(|_| AppError::bad("not a valid .apkg file (expected a zip archive)"))?;
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    let collection = read("collection.anki21b")
        .map(maybe_zstd)
        .or_else(|| read("collection.anki21"))
        .or_else(|| read("collection.anki2"))
        .ok_or_else(|| AppError::bad("no Anki collection found in the package"))?;
    let index: Vec<(String, String)> = match read("media") {
        Some(raw) => match serde_json::from_slice::<HashMap<String, String>>(&raw) {
            Ok(map) => map.into_iter().collect(),
            Err(_) => parse_media_entries(&maybe_zstd(raw))
                .into_iter()
                .enumerate()
                .map(|(i, n)| (i.to_string(), n))
                .collect(),
        },
        None => vec![],
    };
    let mut media = HashMap::new();
    for (zip_name, file_name) in index {
        let lower = file_name.to_ascii_lowercase();
        let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
            .iter()
            .any(|e| lower.ends_with(e));
        if is_image {
            if let Some(bytes) = read(&zip_name) {
                media.insert(file_name, maybe_zstd(bytes));
            }
        }
    }
    Ok(Package { collection, media })
}

struct AnkiCard {
    id: i64,
    nid: i64,
    did: i64,
    ord: i64,
    ctype: i64,
    queue: i64,
    due: i64,
    ivl: i64,
    factor: i64,
    reps: i64,
    lapses: i64,
}

struct AnkiRev {
    time_ms: i64,
    ease: i64,
    rtype: i64,
}

impl App {
    pub async fn import_csv(&self, access: &Access, deck_id: i64, text: &str) -> Result<ImportReport> {
        access.require_write()?;
        self.deck(access, deck_id).await?;
        let rows = parse_csv(text)?;
        let mut report = ImportReport::default();
        for (front, back) in rows {
            if front.is_empty() {
                report.skipped += 1;
                continue;
            }
            self.insert_card(access.user_id(), deck_id, &front, &back, None, None, "import")
                .await?;
            report.cards_imported += 1;
        }
        Ok(report)
    }

    pub async fn export_csv(&self, access: &Access, deck_id: i64) -> Result<String> {
        self.deck(access, deck_id).await?;
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT front, back FROM cards WHERE deck_id = ? ORDER BY position, id")
                .bind(deck_id)
                .fetch_all(&self.db)
                .await?;
        csv_escape(&rows)
    }

    /// Import an Anki package. Decks are matched by name or created.
    pub async fn import_apkg(&self, access: &Access, data: Vec<u8>) -> Result<ImportReport> {
        access.require_write()?;
        access.require_all_decks()?;
        let pkg = tokio::task::spawn_blocking(move || read_package(&data))
            .await
            .map_err(|e| AppError::Internal(e.into()))??;

        let dir = self.config.data_dir.join("tmp");
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("import-{}.sqlite", uuid::Uuid::new_v4().simple()));
        tokio::fs::write(&path, &pkg.collection).await?;
        let result = self.import_collection(access, &path, pkg.media).await;
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(path.with_extension("sqlite-wal")).await;
        let _ = tokio::fs::remove_file(path.with_extension("sqlite-shm")).await;
        result
    }

    async fn import_collection(
        &self,
        access: &Access,
        path: &std::path::Path,
        media: HashMap<String, Vec<u8>>,
    ) -> Result<ImportReport> {
        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(|e| AppError::Internal(e.into()))?
            .read_only(true);
        let mut conn = sqlx::ConnectOptions::connect(&opts)
            .await
            .map_err(|_| AppError::bad("could not open the Anki collection"))?;

        let col = sqlx::query("SELECT crt, decks FROM col LIMIT 1")
            .fetch_one(&mut conn)
            .await
            .map_err(|_| AppError::bad("unrecognized Anki collection"))?;
        let crt: i64 = col.try_get("crt").unwrap_or(0);

        // Deck names: new schema has a decks table, old one a JSON blob.
        let mut deck_names: HashMap<i64, String> = HashMap::new();
        if let Ok(rows) = sqlx::query("SELECT id, name FROM decks").fetch_all(&mut conn).await {
            for r in rows {
                let name: String = r.try_get("name").unwrap_or_default();
                deck_names.insert(r.try_get("id").unwrap_or(0), name.replace('\x1f', "::"));
            }
        }
        if deck_names.is_empty() {
            let blob: String = col.try_get("decks").unwrap_or_default();
            if let Ok(v) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&blob) {
                for (id, d) in v {
                    if let (Ok(id), Some(name)) = (id.parse(), d.get("name").and_then(|n| n.as_str())) {
                        deck_names.insert(id, name.to_string());
                    }
                }
            }
        }

        let notes: HashMap<i64, Vec<String>> = sqlx::query("SELECT id, flds FROM notes")
            .fetch_all(&mut conn)
            .await?
            .into_iter()
            .map(|r| {
                let flds: String = r.try_get("flds").unwrap_or_default();
                (r.try_get("id").unwrap_or(0), flds.split('\x1f').map(String::from).collect())
            })
            .collect();

        let cards: Vec<AnkiCard> = sqlx::query(
            "SELECT id, nid, did, ord, type, queue, due, ivl, factor, reps, lapses FROM cards ORDER BY nid, ord",
        )
        .fetch_all(&mut conn)
        .await?
        .into_iter()
        .map(|r| AnkiCard {
            id: r.get("id"),
            nid: r.get("nid"),
            did: r.get("did"),
            ord: r.get("ord"),
            ctype: r.get("type"),
            queue: r.get("queue"),
            due: r.get("due"),
            ivl: r.get("ivl"),
            factor: r.get("factor"),
            reps: r.get("reps"),
            lapses: r.get("lapses"),
        })
        .collect();

        let mut revlog: HashMap<i64, Vec<AnkiRev>> = HashMap::new();
        if let Ok(rows) = sqlx::query("SELECT id, cid, ease, type FROM revlog ORDER BY id")
            .fetch_all(&mut conn)
            .await
        {
            for r in rows {
                revlog.entry(r.get("cid")).or_default().push(AnkiRev {
                    time_ms: r.get("id"),
                    ease: r.get("ease"),
                    rtype: r.get("type"),
                });
            }
        }
        drop(conn);

        let mut report = ImportReport::default();
        let mut deck_map: HashMap<i64, i64> = HashMap::new();
        let mut image_map: HashMap<String, Option<String>> = HashMap::new();
        let params = access.user.params();
        let fsrs = scheduler::fsrs_for(&params);
        let clock = access.user.clock();

        for c in &cards {
            let Some(fields) = notes.get(&c.nid) else {
                report.skipped += 1;
                continue;
            };
            let f0 = fields.first().map(String::as_str).unwrap_or("");
            let f1 = fields.get(1).map(String::as_str).unwrap_or("");

            let (front_html, back_html) = if !cloze_numbers(f0).is_empty() {
                let n = (c.ord + 1) as u32;
                let back = if f1.trim().is_empty() {
                    render_cloze(f0, None)
                } else {
                    format!("{}<br><br>{}", render_cloze(f0, None), f1)
                };
                (render_cloze(f0, Some(n)), back)
            } else if c.ord == 1 && !f1.is_empty() {
                (f1.to_string(), f0.to_string())
            } else {
                (f0.to_string(), f1.to_string())
            };
            let (front, front_img) = anki_html_to_text(&front_html);
            let (back, back_img) = anki_html_to_text(&back_html);

            let mut resolve = async |name: Option<String>| -> Option<String> {
                let name = name?;
                if let Some(id) = image_map.get(&name) {
                    return id.clone();
                }
                let id = match media.get(&name) {
                    Some(bytes) => self
                        .store_image_for(access.user_id(), bytes.clone())
                        .await
                        .ok()
                        .map(|m| m.id),
                    None => None,
                };
                if id.is_some() {
                    report.images_imported += 1;
                }
                image_map.insert(name, id.clone());
                id
            };
            let front_image = resolve(front_img).await;
            let back_image = resolve(back_img).await;
            if front.is_empty() && front_image.is_none() {
                report.skipped += 1;
                continue;
            }

            let deck_id = match deck_map.get(&c.did) {
                Some(id) => *id,
                None => {
                    let name = deck_names
                        .get(&c.did)
                        .cloned()
                        .unwrap_or_else(|| "Imported".into());
                    let id = match self.deck_by_name(access, &name).await? {
                        Some(d) => d.id,
                        None => {
                            report.decks_created.push(name.clone());
                            self.create_deck(access, &name, "Imported from Anki", None).await?.id
                        }
                    };
                    deck_map.insert(c.did, id);
                    id
                }
            };

            let card_id = self
                .insert_card(access.user_id(), deck_id, &front, &back, front_image, back_image, "import")
                .await?;
            report.cards_imported += 1;

            // History: learning/review/relearning/filtered entries only.
            let revs: Vec<&AnkiRev> = revlog
                .get(&c.id)
                .map(|v| v.iter().filter(|r| (1..=4).contains(&r.ease) && r.rtype <= 3).collect())
                .unwrap_or_default();
            let starts_new = revs.first().is_some_and(|r| r.rtype == 0);
            let mut fsrs_reviews = Vec::new();
            let mut prev: Option<i64> = None;
            for (i, r) in revs.iter().enumerate() {
                let t = r.time_ms / 1000;
                let state_before = if i == 0 && starts_new {
                    CardState::New
                } else {
                    match r.rtype {
                        0 => CardState::Learning,
                        2 => CardState::Relearning,
                        _ => CardState::Review,
                    }
                };
                let delta = prev.map(|p| clock.days_between(p, t).max(0)).unwrap_or(0);
                prev = Some(t);
                fsrs_reviews.push(fsrs::FSRSReview {
                    rating: r.ease as u32,
                    delta_t: delta as u32,
                });
                sqlx::query(
                    "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at,
                        state_before, elapsed_days, scheduled_secs, source)
                     VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'import')",
                )
                .bind(access.user_id())
                .bind(card_id)
                .bind(deck_id)
                .bind(r.ease)
                .bind(t)
                .bind(state_before as i64)
                .bind(delta)
                .execute(&self.db)
                .await?;
                report.reviews_imported += 1;
            }

            let state = match c.ctype {
                1 => CardState::Learning,
                2 => CardState::Review,
                3 => CardState::Relearning,
                _ => CardState::New,
            };
            if state == CardState::New {
                if c.queue == -1 {
                    sqlx::query("UPDATE cards SET suspended = 1 WHERE id = ?")
                        .bind(card_id)
                        .execute(&self.db)
                        .await?;
                }
                continue;
            }
            let memory = if starts_new && !fsrs_reviews.is_empty() {
                fsrs.memory_state(fsrs::FSRSItem { reviews: fsrs_reviews }, None).ok()
            } else if c.factor > 0 && c.ivl > 0 {
                fsrs.memory_state_from_sm2(c.factor as f32 / 1000.0, c.ivl as f32, 0.9).ok()
            } else {
                None
            };
            let due = if state == CardState::Review {
                let date = clock.date_of(crt + c.due * 86_400);
                clock.day_start(date)
            } else {
                now()
            };
            let last_review = prev.or(if state == CardState::Review {
                Some(due - c.ivl.max(1) * 86_400)
            } else {
                None
            });
            sqlx::query(
                "UPDATE cards SET state = ?, stability = ?, difficulty = ?, due = ?, last_review = ?,
                    reps = ?, lapses = ?, suspended = ? WHERE id = ?",
            )
            .bind(state as i64)
            .bind(memory.map(|m| m.stability as f64))
            .bind(memory.map(|m| m.difficulty as f64))
            .bind(due)
            .bind(last_review)
            .bind(c.reps)
            .bind(c.lapses)
            .bind(c.queue == -1)
            .bind(card_id)
            .execute(&self.db)
            .await?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_parsing() {
        let rows = parse_csv("front,back\nhund,dog\n\"a, b\",c\n").unwrap();
        assert_eq!(rows, vec![("hund".into(), "dog".into()), ("a, b".into(), "c".into())]);
        let rows = parse_csv("a\tb\nc\td").unwrap();
        assert_eq!(rows.len(), 2);
        let rows = parse_csv("x;y").unwrap();
        assert_eq!(rows, vec![("x".into(), "y".into())]);
    }

    #[test]
    fn html_stripping() {
        let (t, img) = anki_html_to_text(
            "Hello&nbsp;<b>world</b><br>line 2<div><img src=\"cat.jpg\"></div>[sound:x.mp3]&#x41;",
        );
        assert_eq!(t, "Hello world\nline 2\nA");
        assert_eq!(img.as_deref(), Some("cat.jpg"));
    }

    #[test]
    fn cloze() {
        let s = "{{c1::Paris}} is the capital of {{c2::France::country}}";
        assert_eq!(cloze_numbers(s), vec![1, 2]);
        assert_eq!(render_cloze(s, Some(1)), "**[…]** is the capital of France");
        assert_eq!(render_cloze(s, Some(2)), "Paris is the capital of **[country]**");
        assert_eq!(render_cloze(s, None), "**Paris** is the capital of **France**");
    }

    #[test]
    fn media_protobuf() {
        // entries { name: "a.png" size: 3 } entries { name: "b.jpg" }
        let buf = [
            0x0a, 0x09, 0x0a, 0x05, b'a', b'.', b'p', b'n', b'g', 0x10, 0x03, 0x0a, 0x07, 0x0a,
            0x05, b'b', b'.', b'j', b'p', b'g',
        ];
        assert_eq!(parse_media_entries(&buf), vec!["a.png", "b.jpg"]);
    }
}
