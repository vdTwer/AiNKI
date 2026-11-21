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
    now, num,
    scheduler::{self, CardState},
    service::NewCard,
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
    b"\t;,"
        .iter()
        .copied()
        .max_by_key(|&d| first.matches(char::from(d)).count())
        .filter(|&d| first.contains(char::from(d)))
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
        let front = restore_formula(rec.get(0).unwrap_or("").trim().to_string());
        let back = restore_formula(rec.get(1).unwrap_or("").trim().to_string());
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

/// Spreadsheets run cells starting with = + - @ (or tab/CR) as formulas.
/// Exported decks may contain text from other people (shared decks), so
/// such cells get a leading apostrophe, which spreadsheets hide and our
/// importer strips again.
fn defuse_formula(cell: &str) -> String {
    if cell.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{cell}") } else { cell.to_string() }
}

fn restore_formula(cell: String) -> String {
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(['=', '+', '-', '@', '\t', '\r']) => rest.to_string(),
        _ => cell,
    }
}

fn csv_escape(rows: &[(String, String)]) -> Result<String> {
    let mut w = csv::Writer::from_writer(vec![]);
    w.write_record(["front", "back"]).map_err(|e| AppError::Internal(e.into()))?;
    for (f, b) in rows {
        w.write_record([defuse_formula(f), defuse_formula(b)]).map_err(|e| AppError::Internal(e.into()))?;
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
        if let Some((c, e)) = decoded {
            out.push(c);
            rest = &rest[e + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
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
        let name = lower.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
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
            Some(e) => text.replace_range(s..=(s + e), ""),
            None => break,
        }
    }
    let text = text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string();
    (text, image)
}

/// Render a cloze field: `hide` = Some(n) blanks out cloze n, None reveals all.
fn render_cloze(text: &str, hide: Option<u32>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{c") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let num_len = after.chars().take_while(char::is_ascii_digit).count();
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
            let n = after.chars().take_while(char::is_ascii_digit).count();
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
        v |= u64::from(b & 0x7f) << shift;
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
        let end = pos + num::u64_usize(len);
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
                    let e = p + num::u64_usize(l);
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

/// Limits against decompression bombs: a small upload must not be able to
/// expand into gigabytes.
const MAX_COLLECTION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEDIA_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_MEDIA_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEDIA_FILES: usize = 5000;

/// Read at most `limit` bytes; `None` if there is more.
fn read_limited(r: impl Read, limit: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(limit + 1).read_to_end(&mut buf).ok()?;
    (buf.len() as u64 <= limit).then_some(buf)
}

fn maybe_zstd(data: Vec<u8>, limit: u64) -> Option<Vec<u8>> {
    if data.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        let decoder = zstd::stream::read::Decoder::new(Cursor::new(&data)).ok()?;
        read_limited(decoder, limit)
    } else {
        Some(data)
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
    // Sizes are checked while decompressing, not trusted from the header.
    let mut read = |name: &str, limit: u64| -> Option<Vec<u8>> {
        let f = zip.by_name(name).ok()?;
        read_limited(f, limit)
    };
    let too_big = || AppError::bad("the Anki collection in this package is too large");
    let collection = if let Some(raw) = read("collection.anki21b", MAX_COLLECTION_BYTES) {
        maybe_zstd(raw, MAX_COLLECTION_BYTES).ok_or_else(too_big)?
    } else {
        read("collection.anki21", MAX_COLLECTION_BYTES)
            .or_else(|| read("collection.anki2", MAX_COLLECTION_BYTES))
            .ok_or_else(|| AppError::bad("no Anki collection found in the package (or it is too large)"))?
    };
    let index: Vec<(String, String)> = match read("media", 16 * 1024 * 1024) {
        Some(raw) => match serde_json::from_slice::<HashMap<String, String>>(&raw) {
            Ok(map) => map.into_iter().collect(),
            Err(_) => parse_media_entries(&maybe_zstd(raw, 16 * 1024 * 1024).unwrap_or_default())
                .into_iter()
                .enumerate()
                .map(|(i, n)| (i.to_string(), n))
                .collect(),
        },
        None => vec![],
    };
    let mut media = HashMap::new();
    let mut total = 0u64;
    for (zip_name, file_name) in index {
        let lower = file_name.to_ascii_lowercase();
        let is_image = [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"].iter().any(|e| lower.ends_with(e));
        if !is_image || media.len() >= MAX_MEDIA_FILES || total >= MAX_MEDIA_TOTAL_BYTES {
            continue;
        }
        if let Some(bytes) = read(&zip_name, MAX_MEDIA_FILE_BYTES).and_then(|b| maybe_zstd(b, MAX_MEDIA_FILE_BYTES)) {
            total += bytes.len() as u64;
            media.insert(file_name, bytes);
        }
    }
    Ok(Package { collection, media })
}

#[derive(sqlx::FromRow)]
struct AnkiCard {
    id: i64,
    nid: i64,
    did: i64,
    ord: i64,
    #[sqlx(rename = "type")]
    ctype: i64,
    queue: i64,
    due: i64,
    ivl: i64,
    factor: i64,
    reps: i64,
    lapses: i64,
}

#[derive(sqlx::FromRow)]
struct AnkiRev {
    #[sqlx(rename = "id")]
    time_ms: i64,
    cid: i64,
    ease: i64,
    #[sqlx(rename = "type")]
    rtype: i64,
}

/// What an Anki collection holds, read into memory at once so the
/// untrusted file is closed before anything is written.
struct AnkiCollection {
    /// The collection's creation time; review due dates count days from it.
    crt: i64,
    deck_names: HashMap<i64, String>,
    /// Each note's fields.
    notes: HashMap<i64, Vec<String>>,
    cards: Vec<AnkiCard>,
    /// Each card's reviews, oldest first.
    revlog: HashMap<i64, Vec<AnkiRev>>,
}

fn unrecognized<E>(_: E) -> AppError {
    AppError::bad("unrecognized Anki collection")
}

async fn read_collection(path: &std::path::Path) -> Result<AnkiCollection> {
    // The file is untrusted. Read-only, and with trusted_schema off so
    // views or triggers in it can't call functions with side effects.
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .map_err(|e| AppError::Internal(e.into()))?
        .read_only(true)
        .pragma("trusted_schema", "OFF")
        .pragma("query_only", "ON");
    let mut conn =
        sqlx::ConnectOptions::connect(&opts).await.map_err(|_| AppError::bad("could not open the Anki collection"))?;
    // Every name we read must be a plain table, not a (possibly very
    // expensive) view.
    let views: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE name IN ('col', 'notes', 'cards', 'revlog', 'decks') AND type != 'table'",
    )
    .fetch_one(&mut conn)
    .await
    .map_err(unrecognized)?;
    if views > 0 {
        return Err(unrecognized(()));
    }
    let col = sqlx::query("SELECT crt, decks FROM col LIMIT 1").fetch_one(&mut conn).await.map_err(unrecognized)?;

    // Deck names: new schema has a decks table, old one a JSON blob.
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM decks").fetch_all(&mut conn).await.unwrap_or_default();
    let mut deck_names: HashMap<i64, String> =
        rows.into_iter().map(|(id, name)| (id, name.replace('\x1f', "::"))).collect();
    if deck_names.is_empty() {
        deck_names = old_deck_names(&col.try_get::<String, _>("decks").unwrap_or_default());
    }
    let notes: Vec<(i64, String)> = sqlx::query_as("SELECT id, flds FROM notes").fetch_all(&mut conn).await?;
    let cards: Vec<AnkiCard> = sqlx::query_as(
        // A card in a filtered deck keeps its home deck in odid and its real
        // due in odue; it comes home, as it would when Anki empties the deck.
        "SELECT id, nid, CASE WHEN odid != 0 THEN odid ELSE did END AS did, ord, type, queue,
                CASE WHEN odid != 0 THEN odue ELSE due END AS due, ivl, factor, reps, lapses
         FROM cards ORDER BY nid, ord",
    )
    .fetch_all(&mut conn)
    .await?;
    let revs: Vec<AnkiRev> = sqlx::query_as("SELECT id, cid, ease, type FROM revlog ORDER BY id")
        .fetch_all(&mut conn)
        .await
        .unwrap_or_default();
    let mut revlog: HashMap<i64, Vec<AnkiRev>> = HashMap::new();
    for r in revs {
        revlog.entry(r.cid).or_default().push(r);
    }
    Ok(AnkiCollection {
        crt: col.try_get("crt").unwrap_or(0),
        deck_names,
        notes: notes.into_iter().map(|(id, flds)| (id, flds.split('\x1f').map(String::from).collect())).collect(),
        cards,
        revlog,
    })
}

/// Deck names from the JSON blob older collections keep in `col.decks`.
fn old_deck_names(blob: &str) -> HashMap<i64, String> {
    let Ok(v) = serde_json::from_str::<HashMap<String, serde_json::Value>>(blob) else { return HashMap::new() };
    v.into_iter().filter_map(|(id, d)| Some((id.parse().ok()?, d.get("name")?.as_str()?.to_string()))).collect()
}

/// A card's front and back as Anki shows them: a cloze card shows its
/// own gap, a reverse card (ord 1) swaps the first two fields.
fn card_html(c: &AnkiCard, fields: &[String]) -> (String, String) {
    let f0 = fields.first().map_or("", String::as_str);
    let f1 = fields.get(1).map_or("", String::as_str);
    if !cloze_numbers(f0).is_empty() {
        let full = render_cloze(f0, None);
        let back = if f1.trim().is_empty() { full } else { format!("{full}<br><br>{f1}") };
        (render_cloze(f0, Some(num::i64_u32(c.ord + 1))), back)
    } else if c.ord == 1 && !f1.is_empty() {
        (f1.to_string(), f0.to_string())
    } else {
        (f0.to_string(), f1.to_string())
    }
}

/// One review as it is logged: when, its rating, the state before it and
/// the days since the review before.
struct LoggedReview {
    at: i64,
    ease: i64,
    state_before: CardState,
    delta_days: i64,
}

/// The reviews worth keeping (learning, review, relearning, filtered),
/// with what FSRS needs to know about each.
fn history(revs: &[AnkiRev], clock: &scheduler::Clock) -> Vec<LoggedReview> {
    let kept: Vec<&AnkiRev> = revs.iter().filter(|r| (1..=4).contains(&r.ease) && r.rtype <= 3).collect();
    let starts_new = kept.first().is_some_and(|r| r.rtype == 0);
    let mut prev: Option<i64> = None;
    (kept.iter().enumerate())
        .map(|(i, r)| {
            let at = r.time_ms / 1000;
            let state_before = match r.rtype {
                _ if i == 0 && starts_new => CardState::New,
                0 => CardState::Learning,
                2 => CardState::Relearning,
                _ => CardState::Review,
            };
            let delta_days = prev.map_or(0, |p| clock.days_between(p, at).max(0));
            prev = Some(at);
            LoggedReview { at, ease: r.ease, state_before, delta_days }
        })
        .collect()
}

/// What one import has done so far: the report, and the decks and images
/// it already made, so each is made once.
struct ImportRun {
    report: ImportReport,
    decks: HashMap<i64, i64>,
    images: HashMap<String, Option<String>>,
    media: HashMap<String, Vec<u8>>,
    fsrs: fsrs::FSRS,
}

impl App {
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `BadRequest` if a card breaks the card rules.
    /// - `Internal` if the database fails.
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
            self.insert_card(access.user_id(), deck_id, &NewCard::new(&front, &back), "import").await?;
            report.cards_imported += 1;
        }
        Ok(report)
    }

    /// # Errors
    ///
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
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
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only or limited to some decks.
    /// - `BadRequest` if the file is no readable Anki package or is too large.
    /// - `Internal` if the database or the data folder fails.
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
        let col = read_collection(path).await?;
        let mut run = ImportRun {
            report: ImportReport::default(),
            decks: HashMap::new(),
            images: HashMap::new(),
            media,
            fsrs: scheduler::fsrs_for(&access.user.params())?,
        };
        for c in &col.cards {
            self.import_card(access, &col, c, &mut run).await?;
        }
        Ok(run.report)
    }

    async fn import_card(
        &self,
        access: &Access,
        col: &AnkiCollection,
        c: &AnkiCard,
        run: &mut ImportRun,
    ) -> Result<()> {
        let Some(fields) = col.notes.get(&c.nid) else {
            run.report.skipped += 1;
            return Ok(());
        };
        let (front_html, back_html) = card_html(c, fields);
        let (front, front_img) = anki_html_to_text(&front_html);
        let (back, back_img) = anki_html_to_text(&back_html);
        let front_image = self.import_image(access.user_id(), front_img, run).await;
        let back_image = self.import_image(access.user_id(), back_img, run).await;
        if front.is_empty() && front_image.is_none() {
            run.report.skipped += 1;
            return Ok(());
        }
        let deck_id = self.import_deck(access, col, c.did, run).await?;
        let new = NewCard { front_image, back_image, ..NewCard::new(&front, &back) };
        let card_id = self.insert_card(access.user_id(), deck_id, &new, "import").await?;
        run.report.cards_imported += 1;

        let clock = access.user.clock();
        let reviews = history(col.revlog.get(&c.id).map_or(&[], Vec::as_slice), &clock);
        for r in &reviews {
            sqlx::query(
                "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at,
                    state_before, elapsed_days, scheduled_secs, source)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, 'import')",
            )
            .bind(access.user_id())
            .bind(card_id)
            .bind(deck_id)
            .bind(r.ease)
            .bind(r.at)
            .bind(r.state_before)
            .bind(r.delta_days)
            .execute(&self.db)
            .await?;
            run.report.reviews_imported += 1;
        }
        self.import_schedule(access, col, c, card_id, &reviews, &run.fsrs).await
    }

    /// Stores an image the first time a name comes up; `None` when the
    /// package lacks it or it isn't an image we take.
    async fn import_image(&self, user_id: i64, name: Option<String>, run: &mut ImportRun) -> Option<String> {
        let name = name?;
        if let Some(id) = run.images.get(&name) {
            return id.clone();
        }
        let id = match run.media.get(&name) {
            Some(bytes) => self.store_image_for(user_id, bytes.clone()).await.ok().map(|m| m.id),
            None => None,
        };
        if id.is_some() {
            run.report.images_imported += 1;
        }
        run.images.insert(name, id.clone());
        id
    }

    /// The deck an Anki deck lands in: one of the same name, else a new one.
    async fn import_deck(&self, access: &Access, col: &AnkiCollection, did: i64, run: &mut ImportRun) -> Result<i64> {
        if let Some(id) = run.decks.get(&did) {
            return Ok(*id);
        }
        let name = col.deck_names.get(&did).cloned().unwrap_or_else(|| "Imported".into());
        let id = if let Some(d) = self.deck_by_name(access, &name).await? {
            d.id
        } else {
            run.report.decks_created.push(name.clone());
            self.create_deck(access, &name, "Imported from Anki", None).await?.id
        };
        run.decks.insert(did, id);
        Ok(id)
    }

    /// Carries a card's state over: FSRS memory from its history when that
    /// starts at the beginning, else estimated from Anki's ease and interval.
    async fn import_schedule(
        &self,
        access: &Access,
        col: &AnkiCollection,
        c: &AnkiCard,
        card_id: i64,
        reviews: &[LoggedReview],
        fsrs: &fsrs::FSRS,
    ) -> Result<()> {
        let state = match c.ctype {
            1 => CardState::Learning,
            2 => CardState::Review,
            3 => CardState::Relearning,
            _ => CardState::New,
        };
        if state == CardState::New {
            if c.queue == -1 {
                sqlx::query("UPDATE cards SET suspended = 1, held = 0 WHERE id = ?")
                    .bind(card_id)
                    .execute(&self.db)
                    .await?;
            }
            return Ok(());
        }
        let starts_new = reviews.first().is_some_and(|r| r.state_before == CardState::New);
        let memory = if starts_new {
            let reviews = reviews
                .iter()
                .map(|r| fsrs::FSRSReview { rating: num::i64_u32(r.ease), delta_t: num::i64_u32(r.delta_days) })
                .collect();
            fsrs.memory_state(fsrs::FSRSItem { reviews }, None).ok()
        } else if c.factor > 0 && c.ivl > 0 {
            fsrs.memory_state_from_sm2(num::i64_f32(c.factor) / 1000.0, num::i64_f32(c.ivl), 0.9).ok()
        } else {
            None
        };
        let clock = access.user.clock();
        let due =
            if state == CardState::Review { clock.day_start(clock.date_of(col.crt + c.due * 86_400)) } else { now() };
        let last_review =
            reviews.last().map(|r| r.at).or_else(|| (state == CardState::Review).then(|| due - c.ivl.max(1) * 86_400));
        sqlx::query(
            "UPDATE cards SET state = ?, stability = ?, difficulty = ?, due = ?, last_review = ?,
                reps = ?, lapses = ?, suspended = MAX(?, held) WHERE id = ?",
        )
        .bind(state)
        .bind(memory.map(|m| f64::from(m.stability)))
        .bind(memory.map(|m| f64::from(m.difficulty)))
        .bind(due)
        .bind(last_review)
        .bind(c.reps)
        .bind(c.lapses)
        .bind(c.queue == -1)
        .bind(card_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_formulas_are_defused_and_round_trip() {
        let rows = vec![("=HYPERLINK(\"http://x\")".to_string(), "-5".to_string())];
        let out = csv_escape(&rows).unwrap();
        assert!(out.contains("'=HYPERLINK"));
        assert!(out.contains("'-5"));
        assert_eq!(parse_csv(&out).unwrap(), rows);
    }

    #[test]
    fn limited_reads() {
        assert_eq!(read_limited(&b"abc"[..], 3), Some(b"abc".to_vec()));
        assert_eq!(read_limited(&b"abcd"[..], 3), None);
        let bomb = zstd::encode_all(&vec![0u8; 10_000][..], 3).unwrap();
        assert!(maybe_zstd(bomb.clone(), 1000).is_none());
        assert_eq!(maybe_zstd(bomb, 20_000).map(|v| v.len()), Some(10_000));
    }

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
        let (t, img) =
            anki_html_to_text("Hello&nbsp;<b>world</b><br>line 2<div><img src=\"cat.jpg\"></div>[sound:x.mp3]&#x41;");
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
            0x0a, 0x09, 0x0a, 0x05, b'a', b'.', b'p', b'n', b'g', 0x10, 0x03, 0x0a, 0x07, 0x0a, 0x05, b'b', b'.', b'j',
            b'p', b'g',
        ];
        assert_eq!(parse_media_entries(&buf), vec!["a.png", "b.jpg"]);
    }
}
