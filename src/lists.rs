//! List cards. A card whose back is a numbered list can be learned item by
//! item: after the reveal you mark the items you missed, and every item keeps
//! its own FSRS memory. The card's own scheduling fields always hold a
//! summary of its items (see [`summarize`]), so queues, counts and stats work
//! as for any other card.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    format,
    scheduler::{self, CardSched, CardState, Clock, Rating},
    service::{AnswerResult, Card},
};

/// A back split into the text before its numbered list and the list's items.
#[derive(Debug, Clone, PartialEq)]
pub struct ListBack {
    pub intro: String,
    pub items: Vec<String>,
}

/// The text after "3." or "3)" when a line starts a numbered item.
fn item_start(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if line.len() - t.len() > 3 {
        return None;
    }
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = &t[digits..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some(rest.trim()).filter(|text| !text.is_empty())
}

/// The back's numbered list, if it has one with at least two items. Lines
/// after an item that don't start a new one belong to that item.
pub fn parse(back: &str) -> Option<ListBack> {
    let mut intro = Vec::new();
    let mut items: Vec<String> = Vec::new();
    for line in back.lines() {
        match (item_start(line), items.last_mut()) {
            (Some(text), _) => items.push(text.to_string()),
            (None, Some(item)) if !line.trim().is_empty() => {
                item.push('\n');
                item.push_str(line.trim());
            }
            (None, Some(_)) => {}
            (None, None) => intro.push(line),
        }
    }
    (items.len() >= 2).then(|| ListBack {
        intro: intro.join("\n").trim().to_string(),
        items,
    })
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
        if !card.is_list {
            return Ok(vec![]);
        }
        Ok(sqlx::query_as("SELECT * FROM card_items WHERE card_id = ? ORDER BY position, id")
            .bind(card.id)
            .fetch_all(&self.db)
            .await?)
    }

    /// Make the card a list card, or a plain card again, and match its items
    /// to the back. Items whose words stay the same keep their memory; a
    /// studied card that becomes a list starts every item from its memory.
    pub(crate) async fn sync_items(&self, card: &Card, is_list: bool) -> Result<()> {
        let mut tx = self.db.begin().await?;
        if !is_list {
            sqlx::query("DELETE FROM card_items WHERE card_id = ?").bind(card.id).execute(&mut *tx).await?;
            sqlx::query("UPDATE cards SET is_list = 0 WHERE id = ?").bind(card.id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
        let list = parse(&card.back).ok_or_else(no_list)?;
        let old: Vec<CardItem> = sqlx::query_as("SELECT * FROM card_items WHERE card_id = ? ORDER BY position, id")
            .bind(card.id)
            .fetch_all(&mut *tx)
            .await?;
        let first_time = old.is_empty();
        let mut used = vec![false; old.len()];
        for (pos, text) in list.items.iter().enumerate() {
            let k = key(text);
            match (0..old.len()).find(|&n| !used[n] && key(&old[n].text) == k) {
                Some(n) => {
                    used[n] = true;
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
        sqlx::query("UPDATE cards SET is_list = 1, state = ?, step = ?, stability = ?, difficulty = ?, due = ? WHERE id = ?")
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
            return Err(AppError::bad("this list card has no items"));
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

pub fn no_list() -> AppError {
    AppError::bad("learning item by item needs a numbered list with at least two items on the back")
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

    #[test]
    fn finds_numbered_lists() {
        let l = parse("Cohen says:\r\n1. One\r\n2) Two\r\n   still two\r\n\r\n3. Three").unwrap();
        assert_eq!(l.intro, "Cohen says:");
        assert_eq!(l.items, vec!["One", "Two\nstill two", "Three"]);
        assert!(parse("1. Only one").is_none());
        assert!(parse("2026. was a year\nno list").is_none());
        assert!(parse("- a\n- b").is_none());
        assert!(parse("1.5 is a number\n2.5 too").is_none());
    }

    #[test]
    fn keys_ignore_case_and_spacing() {
        assert_eq!(key("The  Monster Always\nEscapes"), key("the monster always escapes"));
    }
}
