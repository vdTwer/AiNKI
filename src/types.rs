//! Card types people bring themselves.
//!
//! A card type is a package: a manifest (`card-type.json`), one ES module
//! and the files it needs, uploaded as a .zip on the Card types page. To the
//! server a package is data. It is unpacked in memory, checked, stored in
//! the database and served back, and none of it ever runs here: nothing is
//! written to disk, nothing is evaluated, no file name reaches a path.
//!
//! Its code runs only in the learner's browser, in a frame sandboxed to
//! scripts alone (`/types/{token}/frame`, see `web::pages::types`). The frame
//! has an origin of its own that nothing shares: it sees no cookies, cannot
//! read the site's pages, may not connect anywhere and talks to the study page
//! only through `postMessage`. The study page owns the reveal, the rating,
//! the confidence and the timing; the type only draws.
//!
//! A card of such a type keeps its fields as a JSON object. Its front and
//! back hold a plain-text summary filled in from the manifest's template, so
//! search, lists, the coach, exports and agents work with no special case.
//! Every upload is a new row, so a card keeps the version it was written
//! with.

use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    now,
    service::{Card, MAX_CARD_TEXT, NewCard},
};

/// The manifest's name inside a package.
pub const MANIFEST: &str = "card-type.json";
/// The largest .zip accepted, and the most it may unpack to.
pub const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_FILES: usize = 64;
/// Entries read before giving up, directories included.
const MAX_ENTRIES: usize = 256;
const MAX_PATH: usize = 120;
/// What one account may store: versions count, deleted ones don't.
const MAX_TYPES: i64 = 50;
const MAX_STORED_BYTES: i64 = 20 * 1024 * 1024;
/// What a type may hand back as the learner's response, as JSON.
pub const MAX_RESPONSE: usize = 4096;
const MAX_FIELDS: usize = 16;
const MAX_FIELD_TEXT: usize = 5_000;
const MAX_TEMPLATE: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The ES module that draws the card: it exports `challenge` and
    /// `solution`.
    pub module: String,
    /// Style sheets the frame loads before the module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<String>,
    pub fields: Vec<FieldDef>,
    pub summary: Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: FieldKind,
    #[serde(default)]
    pub optional: bool,
    /// A line under the field on the card form.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    /// One line of text.
    #[default]
    Line,
    /// Several lines.
    Text,
}

/// Templates for the card's plain front and back: `{key}` stands for a
/// field's value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub front: String,
    pub back: String,
}

/// One file of a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    pub path: String,
    pub mime: &'static str,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub files: Vec<PackageFile>,
}

/// The kinds of file a package may hold. Nothing a browser would open as a
/// page: no HTML, no SVG (which can carry script), no unknown types.
pub fn mime_for(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

/// A path inside a package, as it may appear in a URL: plain segments of
/// letters, digits, `.`, `_` and `-`, none hidden, none `..`.
fn clean_path(path: &str) -> Option<String> {
    let ok = !path.is_empty()
        && path.len() <= MAX_PATH
        && path.split('/').count() <= 4
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('.')
                && seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    ok.then(|| path.to_string())
}

fn bad(msg: impl Into<String>) -> AppError {
    AppError::bad(msg)
}

/// Reads a .zip into a checked package. Sizes are counted while unpacking,
/// never taken from the archive's own headers.
///
/// # Errors
///
/// `BadRequest`, saying what is wrong, for anything that is no valid package.
pub fn read_package(data: &[u8]) -> Result<Package> {
    if data.len() > MAX_PACKAGE_BYTES {
        return Err(bad("the package is larger than 2 MB"));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| bad("this is no .zip file"))?;
    if zip.len() > MAX_ENTRIES {
        return Err(bad("the package holds too many files"));
    }
    let mut raw: Vec<(String, Vec<u8>)> = Vec::new();
    let mut total = 0usize;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|_| bad("a file in the package can't be read (is it encrypted?)"))?;
        if f.is_dir() {
            continue;
        }
        let shown = f.name().map(|n| n.to_string()).unwrap_or_default();
        if f.is_symlink() {
            return Err(bad(format!("{shown} is a link; packages hold plain files only")));
        }
        // Leftovers some systems add when zipping.
        if shown.starts_with("__MACOSX/") || shown.ends_with(".DS_Store") {
            continue;
        }
        // The name is checked as a whole below (`clean_path`): plain
        // segments only, so no `..`, no leading `/`, no `\`.
        let name = shown;
        if raw.len() >= MAX_FILES {
            return Err(bad(format!("a package holds at most {MAX_FILES} files")));
        }
        let left = MAX_PACKAGE_BYTES - total;
        let mut body = Vec::new();
        (&mut f).take(left as u64 + 1).read_to_end(&mut body).map_err(|_| bad(format!("{name} can't be unpacked")))?;
        if body.len() > left {
            return Err(bad("the package unpacks to more than 2 MB"));
        }
        total += body.len();
        raw.push((name, body));
    }
    let (manifest, files) = sort_out(raw)?;
    let manifest = manifest.ok_or_else(|| bad(format!("the package has no {MANIFEST}")))?;
    let manifest: Manifest = serde_json::from_slice(&manifest).map_err(|e| bad(format!("{MANIFEST}: {e}")))?;
    check_manifest(&manifest, &files)?;
    Ok(Package { manifest, files })
}

/// The unpacked entries as the manifest and the served files, each checked.
fn sort_out(raw: Vec<(String, Vec<u8>)>) -> Result<(Option<Vec<u8>>, Vec<PackageFile>)> {
    // A zipped folder: everything sits in one top folder with the manifest.
    let prefix = raw
        .iter()
        .find(|(n, _)| n.ends_with(&format!("/{MANIFEST}")) && n.matches('/').count() == 1)
        .filter(|_| !raw.iter().any(|(n, _)| n == MANIFEST))
        .map(|(n, _)| n.trim_end_matches(MANIFEST).to_string());
    let mut files = Vec::new();
    let mut manifest = None;
    for (name, body) in raw {
        let name = match &prefix {
            Some(p) => match name.strip_prefix(p.as_str()) {
                Some(rest) => rest.to_string(),
                None => return Err(bad(format!("{name} lies outside the package's folder"))),
            },
            None => name,
        };
        if name == MANIFEST {
            manifest = Some(body);
            continue;
        }
        let path = clean_path(&name).ok_or_else(|| bad(format!("{name} is no allowed file name")))?;
        let mime = mime_for(&path)
            .ok_or_else(|| bad(format!("{name}: packages hold .js, .css, .json, .png and .woff2 files only")))?;
        files.push(PackageFile { path, mime, body });
    }
    Ok((manifest, files))
}

fn plain_line(what: &str, s: &str, max: usize) -> Result<()> {
    if s.trim().is_empty() {
        return Err(bad(format!("{MANIFEST}: {what} is empty")));
    }
    if s.chars().count() > max || s.chars().any(char::is_control) {
        return Err(bad(format!("{MANIFEST}: {what} must be one line of at most {max} characters")));
    }
    Ok(())
}

fn check_manifest(m: &Manifest, files: &[PackageFile]) -> Result<()> {
    plain_line("name", &m.name, 60)?;
    if m.description.chars().count() > 300 {
        return Err(bad(format!("{MANIFEST}: the description has at most 300 characters")));
    }
    let has = |p: &str, ext: &str| files.iter().any(|f| f.path == p && f.path.ends_with(ext));
    if !has(&m.module, ".js") && !has(&m.module, ".mjs") {
        return Err(bad(format!("{MANIFEST}: the module {} is not a .js file in the package", m.module)));
    }
    for s in &m.styles {
        if !has(s, ".css") {
            return Err(bad(format!("{MANIFEST}: the style sheet {s} is not a .css file in the package")));
        }
    }
    if m.fields.is_empty() || m.fields.len() > MAX_FIELDS {
        return Err(bad(format!("{MANIFEST}: a type has 1 to {MAX_FIELDS} fields")));
    }
    for (i, f) in m.fields.iter().enumerate() {
        let key_ok = f.key.len() <= 32
            && f.key.starts_with(|c: char| c.is_ascii_lowercase())
            && f.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !key_ok {
            return Err(bad(format!(
                "{MANIFEST}: field key '{}' must be lowercase letters, digits and _, starting with a letter",
                f.key
            )));
        }
        if m.fields.iter().take(i).any(|g| g.key == f.key) {
            return Err(bad(format!("{MANIFEST}: the field key '{}' is used twice", f.key)));
        }
        plain_line("a field's label", &f.label, 80)?;
        if f.hint.chars().count() > 200 {
            return Err(bad(format!("{MANIFEST}: a field's hint has at most 200 characters")));
        }
    }
    if !m.fields.iter().any(|f| !f.optional) {
        return Err(bad(format!("{MANIFEST}: at least one field must be required")));
    }
    for (side, t) in [("front", &m.summary.front), ("back", &m.summary.back)] {
        if t.chars().count() > MAX_TEMPLATE {
            return Err(bad(format!("{MANIFEST}: the summary's {side} has at most {MAX_TEMPLATE} characters")));
        }
        for key in placeholders(t) {
            if !m.fields.iter().any(|f| f.key == key) {
                return Err(bad(format!("{MANIFEST}: the summary's {side} names {{{key}}}, which is no field")));
            }
        }
    }
    if m.summary.front.trim().is_empty() {
        return Err(bad(format!("{MANIFEST}: the summary's front is empty")));
    }
    Ok(())
}

/// The `{key}` names in a template.
fn placeholders(t: &str) -> impl Iterator<Item = &str> {
    t.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(k, _)| k)).filter(|k| {
        !k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

/// A template with each `{key}` replaced by that field's value.
pub fn fill(template: &str, fields: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (before, after) = rest.split_at(open);
        out.push_str(before);
        let key = after[1..].split_once('}').map(|(k, _)| k);
        if let Some((k, v)) = key.and_then(|k| fields.get(k).map(|v| (k, v))) {
            out.push_str(v);
            rest = &after[k.len() + 2..];
        } else {
            out.push('{');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// A card's fields checked against its type: every required one there,
/// lines on one line, no unknown keys. Empty optional fields are left out.
///
/// # Errors
///
/// `BadRequest`, naming the field, when one breaks a rule.
pub fn check_fields(m: &Manifest, sent: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    if let Some(k) = sent.keys().find(|k| !m.fields.iter().any(|f| &f.key == *k)) {
        return Err(bad(format!("{} has no field '{k}'", m.name)));
    }
    let mut out = BTreeMap::new();
    for f in &m.fields {
        let v = sent.get(&f.key).map_or("", |v| v.trim());
        if v.is_empty() {
            if !f.optional {
                return Err(bad(format!("fill in {}", f.label)));
            }
            continue;
        }
        if f.kind == FieldKind::Line && v.contains(['\n', '\r']) {
            return Err(bad(format!("{} is one line", f.label)));
        }
        if v.chars().count() > MAX_FIELD_TEXT {
            return Err(bad(format!("{} is too long (at most {MAX_FIELD_TEXT} characters)", f.label)));
        }
        out.insert(f.key.clone(), v.replace("\r\n", "\n"));
    }
    Ok(out)
}

/// A learner's response as a type hands it over: JSON, small.
///
/// # Errors
///
/// `BadRequest` when it is no JSON or too long.
pub fn check_response(raw: &str) -> Result<String> {
    if raw.len() > MAX_RESPONSE {
        return Err(bad("the response is too long"));
    }
    let v: Value = serde_json::from_str(raw).map_err(|_| bad("the response is no JSON"))?;
    Ok(v.to_string())
}

/// A stored card type, one version of it.
#[derive(Debug, Clone)]
pub struct CardType {
    pub id: i64,
    pub token: String,
    pub name: String,
    pub version: i64,
    pub manifest: Manifest,
    pub bytes: i64,
    pub created_at: i64,
    /// Cards written with this version.
    pub cards: i64,
}

#[derive(sqlx::FromRow)]
struct TypeRow {
    id: i64,
    token: String,
    name: String,
    version: i64,
    manifest: String,
    bytes: i64,
    created_at: i64,
    cards: i64,
}

impl TryFrom<TypeRow> for CardType {
    type Error = AppError;

    fn try_from(r: TypeRow) -> Result<Self> {
        let manifest = serde_json::from_str(&r.manifest).map_err(|e| AppError::Internal(e.into()))?;
        Ok(Self {
            id: r.id,
            token: r.token,
            name: r.name,
            version: r.version,
            manifest,
            bytes: r.bytes,
            created_at: r.created_at,
            cards: r.cards,
        })
    }
}

const TYPE_COLUMNS: &str = "t.id, t.token, t.name, t.version, t.manifest, t.bytes, t.created_at,
    (SELECT COUNT(*) FROM cards c WHERE c.type_id = t.id) AS cards";

/// A card of a brought type: the type and the card's fields.
#[derive(Debug, Clone)]
pub struct TypedCard {
    pub ty: CardType,
    pub fields: BTreeMap<String, String>,
}

impl TypedCard {
    /// The fields as the frame gets them.
    pub fn fields_json(&self) -> String {
        Value::Object(self.fields.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect::<Map<_, _>>())
            .to_string()
    }
}

impl App {
    /// # Errors
    ///
    /// - `Forbidden` for a token that may not change the account.
    /// - `BadRequest` for an invalid package, or when the account's quota is used up.
    /// - `Internal` if the database fails.
    pub async fn upload_card_type(&self, access: &Access, data: &[u8]) -> Result<CardType> {
        access.require_write()?;
        access.require_all_decks()?;
        let pkg = read_package(data)?;
        let bytes: i64 = pkg.files.iter().map(|f| f.body.len()).sum::<usize>().try_into().unwrap_or(i64::MAX);
        let (count, stored): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM card_types WHERE user_id = ?")
                .bind(access.user_id())
                .fetch_one(&self.db)
                .await?;
        if count >= MAX_TYPES {
            return Err(bad(format!("an account keeps at most {MAX_TYPES} card types; delete unused ones first")));
        }
        if stored + bytes > MAX_STORED_BYTES {
            return Err(bad("your card types use 20 MB already; delete unused ones first"));
        }
        let manifest = serde_json::to_string(&pkg.manifest).map_err(|e| AppError::Internal(e.into()))?;
        let mut tx = self.db.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO card_types (user_id, token, name, version, manifest, bytes, created_at)
             VALUES (?, ?, ?, (SELECT COALESCE(MAX(version), 0) + 1 FROM card_types WHERE user_id = ? AND name = ?),
                ?, ?, ?)
             RETURNING id",
        )
        .bind(access.user_id())
        .bind(crate::auth::random_hex(16))
        .bind(pkg.manifest.name.trim())
        .bind(access.user_id())
        .bind(pkg.manifest.name.trim())
        .bind(&manifest)
        .bind(bytes)
        .bind(now())
        .fetch_one(&mut *tx)
        .await?;
        for f in &pkg.files {
            sqlx::query("INSERT INTO card_type_files (type_id, path, mime, body) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind(&f.path)
                .bind(f.mime)
                .bind(&f.body)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        self.card_type(access, id).await
    }

    /// The account's card types, every version, newest first.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn card_types(&self, access: &Access) -> Result<Vec<CardType>> {
        let rows: Vec<TypeRow> = sqlx::query_as(&format!(
            "SELECT {TYPE_COLUMNS} FROM card_types t WHERE t.user_id = ? ORDER BY t.name, t.version DESC"
        ))
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        rows.into_iter().map(CardType::try_from).collect()
    }

    /// The newest version of each of the account's card types: the ones new
    /// cards are written with.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn latest_card_types(&self, access: &Access) -> Result<Vec<CardType>> {
        let mut all = self.card_types(access).await?;
        all.dedup_by(|b, a| a.name == b.name);
        Ok(all)
    }

    /// # Errors
    ///
    /// - `NotFound` if it isn't the user's.
    /// - `Internal` if the database fails.
    pub async fn card_type(&self, access: &Access, id: i64) -> Result<CardType> {
        let row: TypeRow =
            sqlx::query_as(&format!("SELECT {TYPE_COLUMNS} FROM card_types t WHERE t.id = ? AND t.user_id = ?"))
                .bind(id)
                .bind(access.user_id())
                .fetch_one(&self.db)
                .await?;
        row.try_into()
    }

    /// A version no card uses can go.
    ///
    /// # Errors
    ///
    /// - `Forbidden` for a token that may not change the account.
    /// - `NotFound` if it isn't the user's.
    /// - `Conflict` while cards use it.
    /// - `Internal` if the database fails.
    pub async fn delete_card_type(&self, access: &Access, id: i64) -> Result<()> {
        access.require_write()?;
        access.require_all_decks()?;
        let ty = self.card_type(access, id).await?;
        if ty.cards > 0 {
            return Err(AppError::Conflict(format!(
                "{} cards use this version; delete them first",
                crate::format::number(ty.cards)
            )));
        }
        sqlx::query("DELETE FROM card_types WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// A type by the token in its frame's address. Anyone with the address
    /// may load it: the sandboxed frame sends no cookies.
    ///
    /// # Errors
    ///
    /// - `NotFound` for an unknown token.
    /// - `Internal` if the database fails.
    pub async fn card_type_by_token(&self, token: &str) -> Result<Manifest> {
        let m: String = sqlx::query_scalar("SELECT manifest FROM card_types WHERE token = ?")
            .bind(token)
            .fetch_one(&self.db)
            .await?;
        serde_json::from_str(&m).map_err(|e| AppError::Internal(e.into()))
    }

    /// One file of a type, with its type of content.
    ///
    /// # Errors
    ///
    /// - `NotFound` for an unknown token or path.
    /// - `Internal` if the database fails.
    pub async fn card_type_file(&self, token: &str, path: &str) -> Result<(String, Vec<u8>)> {
        Ok(sqlx::query_as(
            "SELECT f.mime, f.body FROM card_type_files f JOIN card_types t ON t.id = f.type_id
             WHERE t.token = ? AND f.path = ?",
        )
        .bind(token)
        .bind(path)
        .fetch_one(&self.db)
        .await?)
    }

    /// The type and fields of a card written with a brought type; `None`
    /// for any other card.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails or the stored fields are broken.
    pub async fn typed_card(&self, card: &Card) -> Result<Option<TypedCard>> {
        let Some(type_id) = card.type_id else { return Ok(None) };
        let row: TypeRow = sqlx::query_as(&format!("SELECT {TYPE_COLUMNS} FROM card_types t WHERE t.id = ?"))
            .bind(type_id)
            .fetch_one(&self.db)
            .await?;
        let fields: Option<String> = sqlx::query_scalar("SELECT fields FROM cards WHERE id = ?")
            .bind(card.id)
            .fetch_one(&self.db)
            .await?;
        let fields = serde_json::from_str(fields.as_deref().unwrap_or("{}")).map_err(|e| AppError::Internal(e.into()))?;
        Ok(Some(TypedCard { ty: row.try_into()?, fields }))
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the deck or the type isn't the user's.
    /// - `BadRequest` if a field breaks a rule.
    /// - `Internal` if the database fails.
    pub async fn create_typed_card(
        &self,
        access: &Access,
        deck_id: i64,
        type_id: i64,
        fields: &BTreeMap<String, String>,
    ) -> Result<Card> {
        access.require_write()?;
        self.deck(access, deck_id).await?;
        let ty = self.card_type(access, type_id).await?;
        let fields = check_fields(&ty.manifest, fields)?;
        let (front, back) = summary(&ty.manifest, &fields)?;
        let source = match access.actor {
            crate::auth::Actor::Web => "web",
            crate::auth::Actor::Agent { .. } => "agent",
        };
        let id = self.insert_card(access.user_id(), deck_id, &NewCard::new(&front, &back), source).await?;
        sqlx::query("UPDATE cards SET type_id = ?, fields = ? WHERE id = ?")
            .bind(type_id)
            .bind(fields_text(&fields))
            .bind(id)
            .execute(&self.db)
            .await?;
        self.card(access, id).await
    }

    /// New fields for a card of a brought type; it keeps its version.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's.
    /// - `BadRequest` if the card has no type of its own, or a field breaks a rule.
    /// - `Internal` if the database fails.
    pub async fn update_typed_card(&self, access: &Access, id: i64, fields: &BTreeMap<String, String>) -> Result<Card> {
        access.require_write()?;
        let card = self.card(access, id).await?;
        let typed = self.typed_card(&card).await?.ok_or_else(|| bad("this card has no type of its own"))?;
        let fields = check_fields(&typed.ty.manifest, fields)?;
        let (front, back) = summary(&typed.ty.manifest, &fields)?;
        sqlx::query("UPDATE cards SET front = ?, back = ?, fields = ?, updated_at = ? WHERE id = ? AND user_id = ?")
            .bind(&front)
            .bind(&back)
            .bind(fields_text(&fields))
            .bind(now())
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        self.card(access, id).await
    }

    /// Keeps what the learner did before the reveal with their last answer
    /// to the card. Called right after the answer is stored.
    ///
    /// # Errors
    ///
    /// - `BadRequest` if it is no JSON or too long.
    /// - `Internal` if the database fails.
    pub async fn note_response(&self, access: &Access, card_id: i64, raw: &str) -> Result<()> {
        let response = check_response(raw)?;
        sqlx::query(
            "UPDATE review_log SET response = ?
             WHERE id = (SELECT MAX(id) FROM review_log WHERE user_id = ? AND card_id = ?)
               AND (SELECT type_id FROM cards WHERE id = ?) IS NOT NULL",
        )
        .bind(response)
        .bind(access.user_id())
        .bind(card_id)
        .bind(card_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }
}

fn fields_text(fields: &BTreeMap<String, String>) -> String {
    serde_json::to_string(fields).unwrap_or_else(|_| "{}".into())
}

/// The plain front and back of a typed card.
fn summary(m: &Manifest, fields: &BTreeMap<String, String>) -> Result<(String, String)> {
    // An empty optional field leaves no `{key}` behind.
    let mut all = fields.clone();
    for f in &m.fields {
        all.entry(f.key.clone()).or_default();
    }
    let front = fill(&m.summary.front, &all);
    let back = fill(&m.summary.back, &all);
    if front.is_empty() {
        return Err(bad("the card's summary came out empty; fill in more fields"));
    }
    if front.chars().count() > MAX_CARD_TEXT || back.chars().count() > MAX_CARD_TEXT {
        return Err(bad("the card is too long"));
    }
    Ok((front, back))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    const GOOD: &str = r#"{
        "name": "Chess",
        "module": "chess.js",
        "styles": ["board.css"],
        "fields": [
            {"key": "fen", "label": "Position"},
            {"key": "moves", "label": "Solution"},
            {"key": "note", "label": "Note", "kind": "text", "optional": true}
        ],
        "summary": {"front": "Chess: {fen}", "back": "{moves}\n\n{note}"}
    }"#;

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in files {
            w.start_file(*name, SimpleFileOptions::default()).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn good(extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut files: Vec<(&str, &[u8])> =
            vec![(MANIFEST, GOOD.as_bytes()), ("chess.js", b"export {}"), ("board.css", b"a{}")];
        files.extend_from_slice(extra);
        zip_of(&files)
    }

    fn refused(data: &[u8]) -> String {
        match read_package(data) {
            Err(AppError::BadRequest(m)) => m,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_package() {
        let p = read_package(&good(&[("img/piece.png", b"\x89PNG")])).unwrap();
        assert_eq!(p.manifest.name, "Chess");
        let paths: Vec<_> = p.files.iter().map(|f| (f.path.as_str(), f.mime)).collect();
        assert_eq!(paths, [
            ("chess.js", "text/javascript; charset=utf-8"),
            ("board.css", "text/css; charset=utf-8"),
            ("img/piece.png", "image/png")
        ]);
    }

    #[test]
    fn a_zipped_folder_loses_its_top_folder() {
        let data = zip_of(&[
            ("chess/card-type.json", GOOD.as_bytes()),
            ("chess/chess.js", b"export {}"),
            ("chess/board.css", b""),
            ("__MACOSX/chess/._chess.js", b"junk"),
        ]);
        let p = read_package(&data).unwrap();
        assert!(p.files.iter().any(|f| f.path == "chess.js"));
        let stray = zip_of(&[
            ("chess/card-type.json", GOOD.as_bytes()),
            ("chess/chess.js", b""),
            ("chess/board.css", b""),
            ("other.js", b""),
        ]);
        assert!(refused(&stray).contains("outside"));
    }

    #[test]
    fn refuses_what_a_browser_would_open_as_a_page() {
        for name in ["x.html", "x.svg", "x.htm", "x.xml", "x", "x.wasm", "x.JS.html"] {
            assert!(refused(&good(&[(name, b"<script>")])).contains("only"), "{name}");
        }
    }

    #[test]
    fn refuses_odd_paths() {
        for name in ["../x.js", "/etc/x.js", "a/../../x.js", ".hidden.js", "a b.js", "a\\b.js", "a/b/c/d/e.js"] {
            assert!(read_package(&good(&[(name, b"")])).is_err(), "{name}");
        }
    }

    #[test]
    fn refuses_bombs_and_crowds() {
        let big = vec![0u8; MAX_PACKAGE_BYTES];
        let data = good(&[("big.json", &big)]);
        assert!(data.len() < 100_000, "zeros pack small");
        assert!(refused(&data).contains("2 MB"));
        let names: Vec<String> = (0..70).map(|i| format!("f{i}.js")).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), b"".as_slice())).collect();
        assert!(refused(&good(&files)).contains("at most"));
        assert!(refused(b"not a zip").contains("no .zip"));
    }

    #[test]
    fn checks_the_manifest() {
        let with = |m: &str| zip_of(&[(MANIFEST, m.as_bytes()), ("chess.js", b""), ("board.css", b"")]);
        assert!(refused(&zip_of(&[("chess.js", b"")])).contains("no card-type.json"));
        assert!(refused(&with(&GOOD.replace("\"name\"", "\"nmae\""))).contains("unknown field"));
        assert!(refused(&with(&GOOD.replace("chess.js", "missing.js"))).contains("module"));
        assert!(refused(&with(&GOOD.replace("{fen}\"", "{fenn}\""))).contains("no field"));
        assert!(refused(&with(&GOOD.replace("\"moves\"", "\"fen\""))).contains("twice"));
        assert!(refused(&with(&GOOD.replace("\"note\"", "\"No te\""))).contains("lowercase"));
        assert!(refused(&with(&GOOD.replace("\"Chess\"", "\"\""))).contains("name is empty"));
    }

    #[test]
    fn fills_the_summary() {
        let f: BTreeMap<String, String> =
            [("fen".into(), "8/8 w".into()), ("moves".into(), "Nf3".into())].into_iter().collect();
        assert_eq!(fill("Chess: {fen}", &f), "Chess: 8/8 w");
        assert_eq!(fill("{moves}\n\n{note}", &f), "Nf3\n\n{note}");
        assert_eq!(fill("{ {moves}} {", &f), "{ Nf3} {");
    }

    #[test]
    fn checks_fields() {
        let m: Manifest = serde_json::from_str(GOOD).unwrap();
        let f = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
        };
        let ok = check_fields(&m, &f(&[("fen", " 8/8 w "), ("moves", "Nf3"), ("note", "")])).unwrap();
        assert_eq!(ok, f(&[("fen", "8/8 w"), ("moves", "Nf3")]));
        assert!(check_fields(&m, &f(&[("fen", "x")])).is_err(), "moves is required");
        assert!(check_fields(&m, &f(&[("fen", "a\nb"), ("moves", "x")])).is_err(), "a line");
        assert!(check_fields(&m, &f(&[("fen", "a"), ("moves", "x"), ("evil", "y")])).is_err());
        assert!(check_fields(&m, &f(&[("fen", "a"), ("moves", "x"), ("note", "two\nlines")])).is_ok());
    }

    #[test]
    fn responses_are_small_json() {
        assert_eq!(check_response(r#"{ "move": "Nf3" }"#).unwrap(), r#"{"move":"Nf3"}"#);
        assert!(check_response("<b>").is_err());
        assert!(check_response(&format!("\"{}\"", "x".repeat(MAX_RESPONSE))).is_err());
    }
}
