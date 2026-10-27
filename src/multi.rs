//! Multicards. A card whose back holds several things to recall (the items
//! of a list, the sections under its headings, or parts between `---`
//! lines) can be learned item by item: after the reveal you mark the items
//! you remembered, and every item keeps its own FSRS memory. Everything on
//! the back that is not an item (a line above a group, a closing remark)
//! stays and is shown with the answer. The card's own scheduling fields
//! always hold a summary of its items (see [`summarize`]), so queues, counts
//! and stats work as for any other card.

use std::cmp::Ordering;
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    format, markdown,
    scheduler::{self, CardSched, CardState, Clock, Rating},
    service::{AnswerResult, Card},
};

/// Raised whenever splitting changes, so stored items are matched to the
/// backs again on the next start (see [`App::resplit_multicards`]).
const SPLIT_VERSION: i64 = 1;

/// How a multicard's back is split into items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Split {
    /// The items of the back's lists, numbered or not. Sub-items belong to
    /// their item.
    List,
    /// One item per heading, down to the next heading of the same level.
    Sections,
    /// The parts between `---` lines.
    Parts,
}

impl Split {
    pub const ALL: [Split; 3] = [Split::List, Split::Sections, Split::Parts];

    pub fn as_str(self) -> &'static str {
        match self {
            Split::List => "list",
            Split::Sections => "sections",
            Split::Parts => "parts",
        }
    }

    pub fn parse(s: &str) -> Option<Split> {
        Split::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// What the editor calls it.
    pub fn label(self) -> &'static str {
        match self {
            Split::List => "List items",
            Split::Sections => "Sections",
            Split::Parts => "Parts",
        }
    }
}

/// A piece of a multicard's back, in order: text shown as it is, or an
/// item to recall. Both are Markdown.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Item(String),
}

/// The back split into pieces, if the split finds at least two items.
pub fn split(back: &str, how: Split) -> Option<Vec<Piece>> {
    let back = &markdown::tidy(back);
    let items = match how {
        Split::List => list_items(back),
        Split::Sections => sections(back),
        Split::Parts => parts(back),
    };
    if items.len() < 2 {
        return None;
    }
    let mut pieces = Vec::new();
    let mut at = 0;
    for (range, text) in items {
        let gap = back[at..range.start].trim();
        if !gap.is_empty() {
            pieces.push(Piece::Text(gap.to_string()));
        }
        pieces.push(Piece::Item(text));
        at = range.end;
    }
    let rest = back[at..].trim();
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest.to_string()));
    }
    Some(pieces)
}

/// The items alone.
pub fn items(back: &str, how: Split) -> Option<Vec<String>> {
    split(back, how).map(|p| {
        p.into_iter()
            .filter_map(|p| match p {
                Piece::Item(t) => Some(t),
                Piece::Text(_) => None,
            })
            .collect()
    })
}

/// The splits this back allows.
pub fn splits(back: &str) -> Vec<Split> {
    Split::ALL.into_iter().filter(|&k| split(back, k).is_some()).collect()
}

/// Every event of the back with its source range and how deep it sits: 0
/// for the blocks at the top, 1 for what they hold directly, and so on. An
/// end tag has the depth of its start.
fn walk(src: &str, mut f: impl FnMut(usize, &Event, Range<usize>)) {
    let mut depth = 0usize;
    for (event, range) in Parser::new_ext(src, markdown::options()).into_offset_iter() {
        match &event {
            Event::Start(_) => {
                f(depth, &event, range);
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                f(depth, &event, range);
            }
            _ => f(depth, &event, range),
        }
    }
}

/// The items of the lists at the top of the back, without their markers and
/// with their own lines unindented.
fn list_items(src: &str) -> Vec<(Range<usize>, String)> {
    let mut in_list = false;
    let mut out = Vec::new();
    walk(src, |depth, e, range| match (depth, e) {
        (0, Event::Start(tag)) => in_list = matches!(tag, Tag::List(_)),
        (1, Event::Start(Tag::Item)) if in_list => {
            let text = item_text(src, range.clone());
            if !text.is_empty() {
                out.push((range, text));
            }
        }
        _ => {}
    });
    out
}

/// A list item's text: the marker is dropped, and the lines after the first
/// lose the indent that put them inside the item.
fn item_text(src: &str, range: Range<usize>) -> String {
    let line_start = src[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let col = src[line_start..range.start].chars().count();
    let raw = &src[range];
    // "-", "*", "+", or digits and "." or ")".
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    let marker = if digits > 0 { digits + 1 } else { 1 };
    let after = &raw[marker.min(raw.len())..];
    let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
    let indent = col + marker + if (1..=4).contains(&gap) { gap } else { 1 };
    let mut lines = after.lines();
    let mut out = lines.next().unwrap_or("").trim().to_string();
    for line in lines {
        let lead = line.len() - line.trim_start_matches(' ').len();
        out.push('\n');
        out.push_str(&line[lead.min(indent)..]);
    }
    out.trim().to_string()
}

/// One item per heading of the highest level used at the top of the back,
/// each running to the next such heading.
fn sections(src: &str) -> Vec<(Range<usize>, String)> {
    let mut heads: Vec<(HeadingLevel, usize)> = Vec::new();
    walk(src, |depth, e, range| {
        if let (0, Event::Start(Tag::Heading { level, .. })) = (depth, e) {
            heads.push((*level, range.start));
        }
    });
    let Some(top) = heads.iter().map(|h| h.0).min() else { return vec![] };
    let starts: Vec<usize> = heads.iter().filter(|h| h.0 == top).map(|h| h.1).collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &s)| {
            let e = starts.get(n + 1).copied().unwrap_or(src.len());
            (s..e, src[s..e].trim().to_string())
        })
        .collect()
}

/// The parts between thematic breaks (`---`, `***`) at the top of the back.
fn parts(src: &str) -> Vec<(Range<usize>, String)> {
    let mut rules: Vec<Range<usize>> = Vec::new();
    walk(src, |depth, e, range| {
        if depth == 0 && matches!(e, Event::Rule) {
            rules.push(range);
        }
    });
    if rules.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut at = 0;
    for r in rules.iter().cloned().chain(std::iter::once(src.len()..src.len())) {
        // The rule after a part goes with it, so it isn't shown as text.
        let text = src[at..r.start].trim();
        if !text.is_empty() {
            out.push((at..r.end, text.to_string()));
        }
        at = r.end;
    }
    out
}

/// A short name for an item: its first line, without a heading's `#`s.
pub fn title(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim_start_matches('#').trim()
}

/// Items match across edits by their words, ignoring case and spacing.
fn key(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct CardItem {
    pub id: i64,
    pub card_id: i64,
    pub position: i64,
    pub text: String,
    pub state: i64,
    pub step: i64,
    pub stability: Option<f64>,
    pub difficulty: Option<f64>,
    pub due: i64,
    pub last_review: Option<i64>,
    pub reps: i64,
    pub lapses: i64,
}

impl CardItem {
    pub fn card_state(&self) -> CardState {
        CardState::from_i64(self.state)
    }

    pub fn sched(&self) -> CardSched {
        CardSched {
            state: self.card_state(),
            step: self.step,
            stability: self.stability.map(|v| v as f32),
            difficulty: self.difficulty.map(|v| v as f32),
            due: self.due,
            last_review: self.last_review,
            reps: self.reps,
            lapses: self.lapses,
        }
    }

    fn set(&mut self, s: &CardSched) {
        self.state = s.state as i64;
        self.step = s.step;
        self.stability = s.stability.map(|v| v as f64);
        self.difficulty = s.difficulty.map(|v| v as f64);
        self.due = s.due;
        self.last_review = s.last_review;
        self.reps = s.reps;
        self.lapses = s.lapses;
    }

    /// Still being learned: new, learning or relearning.
    fn learning(&self) -> bool {
        self.card_state() != CardState::Review
    }
}

/// Which items a study turn asks for. A new card and a card in review ask
/// for the whole list. While (re)learning, only the items still being
/// learned that are due are asked; the others are shown as known. A card
/// shown early asks for the items that fall due first (items answered the
/// same way in one turn are due at the same moment).
pub fn asked(card: &Card, items: &[CardItem], now: i64) -> Vec<bool> {
    if matches!(card.card_state(), CardState::New | CardState::Review) {
        return vec![true; items.len()];
    }
    let Some(first) = items.iter().filter(|i| i.learning()).map(|i| i.due).min() else {
        return vec![true; items.len()];
    };
    let until = now.max(first + 60);
    items.iter().map(|i| i.learning() && i.due <= until).collect()
}

/// The card's scheduling fields for its items: due when the first item is,
/// (re)learning while any item is, and as strong as its weakest item.
/// Reviews, lapses and the last review stay the card's own.
fn summarize(items: &[CardItem], card: &CardSched) -> CardSched {
    let mut out = card.clone();
    out.step = 0;
    if items.iter().all(|i| i.card_state() == CardState::New) {
        out.state = CardState::New;
        out.stability = None;
        out.difficulty = None;
        out.due = 0;
        return out;
    }
    let learning: Vec<&CardItem> = items.iter().filter(|i| i.learning()).collect();
    out.state = if learning.is_empty() {
        CardState::Review
    } else if learning.iter().any(|i| i.card_state() == CardState::Relearning) {
        CardState::Relearning
    } else {
        CardState::Learning
    };
    let pool = if learning.is_empty() { items.iter().collect() } else { learning };
    out.due = pool.iter().map(|i| i.due).min().unwrap_or(0);
    let weakest = items
        .iter()
        .filter(|i| i.stability.is_some())
        .min_by(|a, b| a.stability.partial_cmp(&b.stability).unwrap_or(Ordering::Equal));
    out.stability = weakest.and_then(|i| i.stability).map(|v| v as f32);
    out.difficulty = weakest.and_then(|i| i.difficulty).map(|v| v as f32);
    out
}

/// What was missed in a list answer.
pub enum Missed {
    /// Every asked item ("Again" on a list card).
    All,
    /// Positions in the list, from 0.
    These(Vec<usize>),
}

impl App {
    /// A list card's items in list order (empty for other cards). The caller
    /// has already checked that the card is visible.
    pub async fn card_items(&self, card: &Card) -> Result<Vec<CardItem>> {
        if card.multi.is_none() {
            return Ok(vec![]);
        }
        Ok(sqlx::query_as("SELECT * FROM card_items WHERE card_id = ? ORDER BY position, id")
            .bind(card.id)
            .fetch_all(&self.db)
            .await?)
    }

    /// Make the card a multicard split as `multi`, or a plain card again,
    /// and match its items to the back. Items whose words stay the same keep
    /// their memory, and so does an item that only gained or lost words at
    /// its end; a studied card that becomes a multicard starts every item
    /// from its memory.
    pub(crate) async fn sync_items(&self, card: &Card, multi: Option<Split>) -> Result<()> {
        let mut tx = self.db.begin().await?;
        let Some(how) = multi else {
            sqlx::query("DELETE FROM card_items WHERE card_id = ?").bind(card.id).execute(&mut *tx).await?;
            sqlx::query("UPDATE cards SET multi = NULL WHERE id = ?").bind(card.id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        };
        let texts = items(&card.back, how).ok_or_else(|| no_items(how))?;
        let old: Vec<CardItem> = sqlx::query_as("SELECT * FROM card_items WHERE card_id = ? ORDER BY position, id")
            .bind(card.id)
            .fetch_all(&mut *tx)
            .await?;
        let first_time = old.is_empty();
        let mut used = vec![false; old.len()];
        // Same words first, then the same start at the same place.
        let keys: Vec<String> = texts.iter().map(|t| key(t)).collect();
        let mut pick: Vec<Option<usize>> = keys
            .iter()
            .map(|k| {
                let n = (0..old.len()).find(|&n| !used[n] && key(&old[n].text) == *k)?;
                used[n] = true;
                Some(n)
            })
            .collect();
        for (pos, k) in keys.iter().enumerate() {
            if pick[pos].is_none() && pos < old.len() && !used[pos] {
                let o = key(&old[pos].text);
                if !k.is_empty() && !o.is_empty() && (o.starts_with(k.as_str()) || k.starts_with(o.as_str())) {
                    used[pos] = true;
                    pick[pos] = Some(pos);
                }
            }
        }
        for (pos, text) in texts.iter().enumerate() {
            match pick[pos] {
                Some(n) => {
                    sqlx::query("UPDATE card_items SET position = ?, text = ? WHERE id = ?")
                        .bind(pos as i64)
                        .bind(text)
                        .bind(old[n].id)
                        .execute(&mut *tx)
                        .await?;
                }
                None => {
                    let s = if first_time { card.sched() } else { CardSched::new_card() };
                    sqlx::query(
                        "INSERT INTO card_items (card_id, position, text, state, step, stability, difficulty,
                            due, last_review, reps, lapses)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(card.id)
                    .bind(pos as i64)
                    .bind(text)
                    .bind(s.state as i64)
                    .bind(s.step)
                    .bind(s.stability.map(|v| v as f64))
                    .bind(s.difficulty.map(|v| v as f64))
                    .bind(s.due)
                    .bind(s.last_review)
                    .bind(s.reps)
                    .bind(s.lapses)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
        for (n, item) in old.iter().enumerate() {
            if !used[n] {
                sqlx::query("DELETE FROM card_items WHERE id = ?").bind(item.id).execute(&mut *tx).await?;
            }
        }
        let items: Vec<CardItem> = sqlx::query_as("SELECT * FROM card_items WHERE card_id = ? ORDER BY position, id")
            .bind(card.id)
            .fetch_all(&mut *tx)
            .await?;
        let s = summarize(&items, &card.sched());
        sqlx::query("UPDATE cards SET multi = ?, state = ?, step = ?, stability = ?, difficulty = ?, due = ? WHERE id = ?")
            .bind(how)
            .bind(s.state as i64)
            .bind(s.step)
            .bind(s.stability.map(|v| v as f64))
            .bind(s.difficulty.map(|v| v as f64))
            .bind(s.due)
            .bind(card.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Record a list answer: missed items count as Again, the other asked
    /// items as Good, each on its own memory. The review log gets one row
    /// for the card (Again if anything was missed) plus one per item.
    pub(crate) async fn answer_items(
        &self,
        access: &Access,
        card: &Card,
        missed: Missed,
        duration_ms: Option<i64>,
    ) -> Result<AnswerResult> {
        let mut items = self.card_items(card).await?;
        if items.is_empty() {
            return Err(AppError::bad("this multicard has no items"));
        }
        let clock = access.user.clock();
        let asked = asked(card, &items, clock.now);
        let missed: Vec<bool> = match missed {
            Missed::All => asked.clone(),
            Missed::These(positions) => {
                let mut m = vec![false; items.len()];
                for p in positions {
                    if p >= items.len() || !asked[p] {
                        return Err(AppError::bad(format!("item {} was not asked this time", p + 1)));
                    }
                    m[p] = true;
                }
                m
            }
        };
        let deck = self.deck(access, card.deck_id).await?;
        let cfg = self.sched_config(access, &deck).await?;
        let fsrs = scheduler::fsrs_for(&access.user.params());
        let before_items = serde_json::to_string(&items).ok();
        let mut item_logs = Vec::new();
        for (n, item) in items.iter_mut().enumerate() {
            if !asked[n] {
                continue;
            }
            let rating = if missed[n] { Rating::Again } else { Rating::Good };
            let out = scheduler::answer(&fsrs, &cfg, item.id, &item.sched(), rating, &clock)?;
            item_logs.push((item.id, rating, item.state));
            item.set(&out.card);
        }

        let before = card.sched();
        let n_asked = asked.iter().filter(|a| **a).count() as i64;
        let n_missed = missed.iter().filter(|m| **m).count() as i64;
        let mut c = summarize(&items, &before);
        c.last_review = Some(clock.now);
        c.reps = before.reps + 1;
        if n_missed > 0 && before.state == CardState::Review {
            c.lapses += 1;
        }
        let elapsed_days = before
            .last_review
            .map(|last| clock.days_between(last, clock.now).max(0))
            .unwrap_or(0);
        let interval_secs = (c.due - clock.now).max(0);

        let mut tx = self.db.begin().await?;
        let updated = sqlx::query(
            "UPDATE cards SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                last_review = ?, reps = ?, lapses = ?, updated_at = ?
             WHERE id = ? AND user_id = ? AND reps = ?",
        )
        .bind(c.state as i64)
        .bind(c.step)
        .bind(c.stability.map(|v| v as f64))
        .bind(c.difficulty.map(|v| v as f64))
        .bind(c.due)
        .bind(c.last_review)
        .bind(c.reps)
        .bind(c.lapses)
        .bind(clock.now)
        .bind(card.id)
        .bind(access.user_id())
        .bind(card.reps)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(AppError::Conflict("this card was already answered".into()));
        }
        for (n, item) in items.iter().enumerate() {
            if !asked[n] {
                continue;
            }
            sqlx::query(
                "UPDATE card_items SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                    last_review = ?, reps = ?, lapses = ? WHERE id = ?",
            )
            .bind(item.state)
            .bind(item.step)
            .bind(item.stability)
            .bind(item.difficulty)
            .bind(item.due)
            .bind(item.last_review)
            .bind(item.reps)
            .bind(item.lapses)
            .bind(item.id)
            .execute(&mut *tx)
            .await?;
        }
        let rating = if n_missed > 0 { Rating::Again } else { Rating::Good };
        let log_id: i64 = sqlx::query_scalar(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, stability, difficulty, duration_ms, source, prev_card,
                items_asked, items_recalled, prev_items)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(access.user_id())
        .bind(card.id)
        .bind(card.deck_id)
        .bind(rating as i64)
        .bind(clock.now)
        .bind(card.state)
        .bind(elapsed_days)
        .bind(interval_secs)
        .bind(c.stability.map(|v| v as f64))
        .bind(c.difficulty.map(|v| v as f64))
        .bind(duration_ms.map(|d| d.clamp(0, 3_600_000)))
        .bind(access.source())
        .bind(serde_json::to_string(&before).ok())
        .bind(n_asked)
        .bind(n_asked - n_missed)
        .bind(before_items)
        .fetch_one(&mut *tx)
        .await?;
        for (item_id, rating, state_before) in item_logs {
            sqlx::query(
                "INSERT INTO item_review_log (review_log_id, item_id, rating, state_before, reviewed_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(log_id)
            .bind(item_id)
            .bind(rating as i64)
            .bind(state_before)
            .bind(clock.now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        let interval = if c.state == CardState::Review {
            format::days(clock.days_between(clock.now, c.due).max(1))
        } else {
            format::interval(interval_secs)
        };
        Ok(AnswerResult {
            card: self.card(access, card.id).await?,
            interval_secs,
            interval,
        })
    }

    /// Split every multicard's back again, once after an update that
    /// changed how backs are split (`SPLIT_VERSION`), so items match the
    /// backs as they are now shown. Items keep their memory (see
    /// [`App::sync_items`]).
    pub async fn resplit_multicards(&self) -> Result<()> {
        let version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&self.db).await?;
        if version >= SPLIT_VERSION {
            return Ok(());
        }
        let cards: Vec<Card> = sqlx::query_as("SELECT * FROM cards WHERE multi IS NOT NULL").fetch_all(&self.db).await?;
        for card in &cards {
            let how = card.multi.filter(|&h| split(&card.back, h).is_some());
            if let Err(e) = self.sync_items(card, how).await {
                tracing::warn!("card {}: could not split its back again: {e}", card.id);
            }
        }
        sqlx::query(&format!("PRAGMA user_version = {SPLIT_VERSION}")).execute(&self.db).await?;
        Ok(())
    }

    /// Put items back as they were before an answer (undo).
    pub(crate) async fn restore_items(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        card_id: i64,
        prev_items: &str,
    ) -> Result<()> {
        let Ok(items) = serde_json::from_str::<Vec<CardItem>>(prev_items) else { return Ok(()) };
        for i in items {
            sqlx::query(
                "UPDATE card_items SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                    last_review = ?, reps = ?, lapses = ? WHERE id = ? AND card_id = ?",
            )
            .bind(i.state)
            .bind(i.step)
            .bind(i.stability)
            .bind(i.difficulty)
            .bind(i.due)
            .bind(i.last_review)
            .bind(i.reps)
            .bind(i.lapses)
            .bind(i.id)
            .bind(card_id)
            .execute(&mut **tx)
            .await?;
        }
        Ok(())
    }
}

pub fn no_items(how: Split) -> AppError {
    let what = match how {
        Split::List => "list items",
        Split::Sections => "sections with headings",
        Split::Parts => "parts between --- lines",
    };
    AppError::bad(format!("learning item by item this way needs at least two {what} on the back"))
}

/// Items as the MCP API shows them: numbered from 1, with recall chances,
/// and for a study turn whether each one is asked.
pub fn items_json(items: &[CardItem], asked: Option<&[bool]>, clock: &Clock, params: &[f32]) -> Vec<Value> {
    items
        .iter()
        .enumerate()
        .map(|(n, i)| {
            let mut v = json!({
                "number": n + 1,
                "text": i.text,
                "state": i.card_state().as_str(),
                "retrievability": scheduler::retrievability(&i.sched(), params, clock)
                    .map(|r| (r as f64 * 1000.0).round() / 1000.0),
            });
            if let Some(a) = asked {
                v["asked"] = json!(a[n]);
            }
            v
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(back: &str, how: Split) -> Vec<String> {
        items(back, how).unwrap_or_default()
    }

    #[test]
    fn finds_list_items() {
        let back = "Cohen says:\r\n1. One\r\n2) Two\r\n   still two\r\n\r\n3. Three";
        assert_eq!(texts(back, Split::List), ["One", "Two\nstill two", "Three"]);
        assert_eq!(texts("- a\n- b\n  - b1\n  - b2", Split::List), ["a", "b\n- b1\n- b2"]);
        assert!(split("1. Only one", Split::List).is_none());
        assert!(split("2026. was a year\nno list", Split::List).is_none());
        assert!(split("1.5 is a number\n2.5 too", Split::List).is_none());
        // A list inside a quote is text.
        assert!(split("> - a\n> - b", Split::List).is_none());
    }

    #[test]
    fn keeps_what_is_not_an_item() {
        let back = "will\n    - decisions made now: I'll give you a hand.\n    - guesses: I think I'll make it.\n\n\
            be going to\n    - intentions: We're going to have a barbecue.\n\n\
            ⚠️ Remark: will is for spontaneous decisions.\nOther future forms: the present simple.";
        let pieces = split(back, Split::List).unwrap();
        assert_eq!(
            pieces,
            [
                Piece::Text("will".into()),
                Piece::Item("decisions made now: I'll give you a hand.".into()),
                Piece::Item("guesses: I think I'll make it.".into()),
                Piece::Text("be going to".into()),
                Piece::Item("intentions: We're going to have a barbecue.".into()),
                Piece::Text("⚠️ Remark: will is for spontaneous decisions.\nOther future forms: the present simple.".into()),
            ]
        );
    }

    #[test]
    fn finds_sections_and_parts() {
        let back = "Intro\n\n## will\nnow\n### detail\nx\n\n## going to\nplanned";
        assert_eq!(
            split(back, Split::Sections).unwrap(),
            [
                Piece::Text("Intro".into()),
                Piece::Item("## will\nnow\n### detail\nx".into()),
                Piece::Item("## going to\nplanned".into()),
            ]
        );
        assert!(split("# only one\ntext", Split::Sections).is_none());
        let back = "one\n\n---\n\ntwo\nlines\n\n***\n\nthree";
        assert_eq!(texts(back, Split::Parts), ["one", "two\nlines", "three"]);
        assert!(split(back, Split::Parts).unwrap().iter().all(|p| matches!(p, Piece::Item(_))));
        // Text right above --- is a heading, not a part.
        assert!(split("one\n---\ntwo", Split::Parts).is_none());
        assert_eq!(splits("- a\n- b"), [Split::List]);
    }

    #[test]
    fn keys_ignore_case_and_spacing() {
        assert_eq!(key("The  Monster Always\nEscapes"), key("the monster always escapes"));
    }
}
