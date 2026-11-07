//! Merged decks: a new deck holding copies of the cards of two or more
//! decks. The decks it is made from are only read, never changed.

use std::collections::HashSet;

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
}

#[derive(Debug, Clone, Serialize)]
pub struct Merged {
    pub deck: Deck,
    /// Cards copied into the new deck.
    pub cards: i64,
    /// Cards left out because an identical one was already copied.
    pub duplicates: i64,
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
}

impl Source {
    fn content(&self) -> (String, String, Option<String>, Option<String>, Option<Split>) {
        (self.front.clone(), self.back.clone(), self.front_image.clone(), self.back_image.clone(), self.multi)
    }
}

impl App {
    /// Make a new deck from copies of the cards of `input.sources`.
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
        match self.copy_into(access, &deck, &sources, input.keep_progress).await {
            Ok((cards, duplicates)) => Ok(Merged { deck, cards, duplicates }),
            Err(e) => {
                // No half-filled deck: the copies went in one transaction,
                // so only the empty deck is left to take away.
                if let Err(cleanup) = self.delete_deck(access, deck.id).await {
                    tracing::error!("merge: removing the empty deck {} failed: {cleanup}", deck.id);
                }
                Err(e)
            }
        }
    }

    /// Copy the cards of `sources` into `deck`, all or nothing.
    /// Returns (copied, duplicates left out).
    async fn copy_into(&self, access: &Access, deck: &Deck, sources: &[i64], keep_progress: bool) -> Result<(i64, i64)> {
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        let mut copied_content = HashSet::new();
        let (mut cards, mut duplicates) = (0, 0);
        let t = now();
        // Cards and items share these columns: copied, or those of a new card.
        let schedule = if keep_progress {
            "state, step, stability, difficulty, due, last_review, reps, lapses"
        } else {
            "0, 0, NULL, NULL, 0, NULL, 0, 0"
        };
        for source in sources {
            let originals: Vec<Source> = sqlx::query_as(
                "SELECT id, front, back, front_image, back_image, multi, suspended, copy_of
                 FROM cards WHERE deck_id = ? AND user_id = ? ORDER BY position, id",
            )
            .bind(source)
            .bind(access.user_id())
            .fetch_all(&mut *tx)
            .await?;
            for card in originals {
                if !copied_content.insert(card.content()) {
                    duplicates += 1;
                    continue;
                }
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
                cards += 1;
            }
        }
        tx.commit().await?;
        Ok((cards, duplicates))
    }
}
