//! Merged decks: a new deck holding copies of the cards of two or more
//! decks. The decks it is made from are only read, never changed, unless
//! the person asks to delete them: then their cards move instead.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    multi::Split,
    now,
    service::Deck,
};

#[derive(Debug, Clone)]
pub struct MergeInput {
    /// The decks to copy, in the order their cards go into the new deck
    /// (on the website: the order of the deck list).
    pub sources: Vec<i64>,
    pub name: String,
    pub description: String,
    /// `None` takes the preset of the first deck.
    pub preset_id: Option<i64>,
    /// Copy each card's schedule (and its items'), so known cards stay known.
    /// Without it the copies are new cards. Review history is never copied:
    /// those reviews happened once, and stats would count them twice.
    pub keep_progress: bool,
    /// Delete the decks it is made from. Their cards then move into the new
    /// deck instead of being copied, so their review history moves along.
    pub delete_sources: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Merged {
    pub deck: Deck,
    /// Cards copied into the new deck.
    pub cards: i64,
    /// Cards left out because an identical one was already copied.
    pub duplicates: i64,
    /// The decks it was made from, deleted.
    pub deleted_decks: i64,
}

/// One card to copy, with what identifies its content.
#[derive(sqlx::FromRow)]
struct Source {
    id: i64,
    front: String,
    back: String,
    front_image: Option<String>,
    back_image: Option<String>,
    multi: Option<Split>,
    suspended: bool,
    copy_of: Option<i64>,
    state: i64,
}

/// What makes two cards the same card.
type Content = (String, String, Option<String>, Option<String>, Option<Split>);

impl Source {
    fn content(&self) -> Content {
        (self.front.clone(), self.back.clone(), self.front_image.clone(), self.back_image.clone(), self.multi)
    }
}

impl App {
    /// Make a new deck from the cards of `input.sources`.
    pub async fn merge_decks(&self, access: &Access, input: MergeInput) -> Result<Merged> {
        access.require_write()?;
        access.require_all_decks()?;
        let mut seen = HashSet::new();
        let sources: Vec<i64> = input.sources.into_iter().filter(|id| seen.insert(*id)).collect();
        if sources.len() < 2 {
            return Err(AppError::bad("pick at least two decks to merge"));
        }
        let mut decks = Vec::with_capacity(sources.len());
        for id in &sources {
            decks.push(self.deck(access, *id).await?);
        }
        let preset_id = input.preset_id.unwrap_or(decks[0].preset_id);
        let deck = self.create_deck(access, &input.name, &input.description, Some(preset_id)).await?;
        let how = Fill { keep_progress: input.keep_progress, delete_sources: input.delete_sources };
        match self.fill(access, &deck, &sources, how).await {
            Ok((cards, duplicates)) => {
                let deleted_decks = if how.delete_sources { sources.len() as i64 } else { 0 };
                Ok(Merged { deck, cards, duplicates, deleted_decks })
            }
            Err(e) => {
                // No half-filled deck: the cards went in one transaction,
                // so only the empty deck is left to take away.
                if let Err(cleanup) = self.delete_deck(access, deck.id).await {
                    tracing::error!("merge: removing the empty deck {} failed: {cleanup}", deck.id);
                }
                Err(e)
            }
        }
    }

    /// Copy (or move) the cards of `sources` into `deck`, all or nothing.
    /// Returns (cards in the new deck, duplicates left out).
    async fn fill(&self, access: &Access, deck: &Deck, sources: &[i64], how: Fill) -> Result<(i64, i64)> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        // Each content once: the card in the new deck that holds it.
        let mut kept: HashMap<Content, i64> = HashMap::new();
        let (mut cards, mut duplicates) = (0, 0);
        let t = now();
        // Cards and items share these columns: copied, or those of a new card.
        let schedule = if how.keep_progress {
            "state, step, stability, difficulty, due, last_review, reps, lapses"
        } else {
            "0, 0, NULL, NULL, 0, NULL, 0, 0"
        };
        for source in sources {
            let originals: Vec<Source> = sqlx::query_as(
                "SELECT id, front, back, front_image, back_image, multi, suspended, copy_of, state
                 FROM cards WHERE deck_id = ? AND user_id = ? ORDER BY position, id",
            )
            .bind(source)
            .bind(access.user_id())
            .fetch_all(&mut *tx)
            .await?;
            for card in originals {
                if let Some(&into) = kept.get(&card.content()) {
                    duplicates += 1;
                    if how.delete_sources {
                        // The duplicate goes with its deck; its reviews
                        // happened, so they stay, on the card that is kept.
                        sqlx::query("UPDATE review_log SET card_id = ?, deck_id = ? WHERE card_id = ?")
                            .bind(into)
                            .bind(deck.id)
                            .bind(card.id)
                            .execute(&mut *tx)
                            .await?;
                    }
                    continue;
                }
                let id = if how.delete_sources {
                    self.move_into(&mut tx, access, deck, &card, how.keep_progress, t).await?
                } else {
                    self.copy_card(&mut tx, access, deck, &card, schedule, t).await?
                };
                kept.insert(card.content(), id);
                cards += 1;
            }
        }
        if how.delete_sources {
            for source in sources {
                sqlx::query("DELETE FROM decks WHERE id = ? AND user_id = ?")
                    .bind(source)
                    .bind(access.user_id())
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok((cards, duplicates))
    }

    async fn copy_card(
        &self,
        tx: &mut sqlx::SqliteConnection,
        access: &Access,
        deck: &Deck,
        card: &Source,
        schedule: &str,
        t: i64,
    ) -> Result<i64> {
        let id: i64 = sqlx::query_scalar(&format!(
            "INSERT INTO cards (user_id, deck_id, front, back, front_image, back_image,
                state, step, stability, difficulty, due, last_review, reps, lapses,
                suspended, multi, copy_of, position, created_by, created_at, updated_at)
             SELECT user_id, ?, front, back, front_image, back_image, {schedule},
                ?, multi, ?,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM cards WHERE user_id = ?),
                'merged', ?, ?
             FROM cards WHERE id = ?
             RETURNING id"
        ))
        .bind(deck.id)
        .bind(card.suspended)
        .bind(card.copy_of.unwrap_or(card.id))
        .bind(access.user_id())
        .bind(t)
        .bind(t)
        .bind(card.id)
        .fetch_one(&mut *tx)
        .await?;
        if card.multi.is_some() {
            sqlx::query(&format!(
                "INSERT INTO card_items (card_id, position, text,
                    state, step, stability, difficulty, due, last_review, reps, lapses)
                 SELECT ?, position, text, {schedule}
                 FROM card_items WHERE card_id = ? ORDER BY position, id"
            ))
            .bind(id)
            .bind(card.id)
            .execute(&mut *tx)
            .await?;
        }
        Ok(id)
    }

    /// Move a card into the new deck: same card, same history. Without
    /// progress it starts over the way "Forget" does it.
    async fn move_into(
        &self,
        tx: &mut sqlx::SqliteConnection,
        access: &Access,
        deck: &Deck,
        card: &Source,
        keep_progress: bool,
        t: i64,
    ) -> Result<i64> {
        sqlx::query(
            "UPDATE cards SET deck_id = ?,
                position = (SELECT COALESCE(MAX(position), 0) + 1 FROM cards WHERE user_id = ?)
             WHERE id = ? AND user_id = ?",
        )
        .bind(deck.id)
        .bind(access.user_id())
        .bind(card.id)
        .bind(access.user_id())
        .execute(&mut *tx)
        .await?;
        // Stats count reviews by deck: the old deck is going away.
        sqlx::query("UPDATE review_log SET deck_id = ? WHERE card_id = ?")
            .bind(deck.id)
            .bind(card.id)
            .execute(&mut *tx)
            .await?;
        if !keep_progress {
            sqlx::query(
                "UPDATE cards SET state = 0, step = 0, stability = NULL, difficulty = NULL, due = 0,
                    last_review = NULL, reps = 0, lapses = 0, updated_at = ? WHERE id = ?",
            )
            .bind(t)
            .bind(card.id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE card_items SET state = 0, step = 0, stability = NULL, difficulty = NULL, due = 0,
                    last_review = NULL, reps = 0, lapses = 0 WHERE card_id = ?",
            )
            .bind(card.id)
            .execute(&mut *tx)
            .await?;
            // The same mark a reset leaves, so the optimizer sees the restart.
            sqlx::query(
                "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                    elapsed_days, scheduled_secs, source)
                 VALUES (?, ?, ?, 0, ?, ?, 0, 0, 'manual')",
            )
            .bind(access.user_id())
            .bind(card.id)
            .bind(deck.id)
            .bind(t)
            .bind(card.state)
            .execute(&mut *tx)
            .await?;
        }
        Ok(card.id)
    }
}

#[derive(Clone, Copy)]
struct Fill {
    keep_progress: bool,
    delete_sources: bool,
}
