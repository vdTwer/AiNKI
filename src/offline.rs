//! Offline studying.
//!
//! While online, the browser downloads today's study queue (cards, rendered
//! HTML, interval previews) and caches images through a service worker.
//! Offline, a small JavaScript client studies from that queue and keeps the
//! answers. When the connection is back it sends them here; each answer is
//! replayed with the time it was actually given, so FSRS sees the real history.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    markdown,
    scheduler::{self, CardState, Rating},
    service::Card,
};

/// Don't accept offline answers older than this.
const MAX_OFFLINE_AGE_SECS: i64 = 30 * 86_400;

#[derive(Debug, Serialize)]
pub struct QueueItem {
    pub id: i64,
    pub deck_id: i64,
    pub deck: String,
    pub front: String,
    pub back: String,
    pub front_image: Option<String>,
    pub back_image: Option<String>,
    pub state: &'static str,
    pub reps: i64,
    /// Unix seconds; 0 for new cards.
    pub due: i64,
    /// rating -> {label, secs}
    pub previews: HashMap<&'static str, Value>,
}

#[derive(Debug, Deserialize)]
pub struct OfflineAnswer {
    pub client_id: String,
    pub card_id: i64,
    /// again | hard | good | easy (or 1-4)
    pub rating: String,
    /// The card's review count when it was shown; detects cards that were
    /// already answered elsewhere in the meantime.
    pub reps: i64,
    /// Unix seconds.
    pub answered_at: i64,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub client_id: String,
    /// ok | duplicate | stale | error
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn rating_key(r: Rating) -> &'static str {
    match r {
        Rating::Again => "again",
        Rating::Hard => "hard",
        Rating::Good => "good",
        Rating::Easy => "easy",
    }
}

impl App {
    /// Everything studyable for the rest of today, within daily limits.
    pub async fn offline_queue(&self, access: &Access) -> Result<Value> {
        let summaries = self.deck_summaries(access).await?;
        let presets: HashMap<i64, crate::service::Preset> = self
            .list_presets(access)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
        let clock = access.user.clock();
        let end_of_day = clock.end_of_today();
        let params = access.user.params();
        let fsrs = scheduler::fsrs_for(&params);

        let mut learning: Vec<(Card, usize)> = Vec::new();
        let mut reviews: Vec<(Card, usize)> = Vec::new();
        let mut new: Vec<(Card, usize)> = Vec::new();
        for (i, s) in summaries.iter().enumerate() {
            let fetch = |cond: &'static str, order: &'static str, limit: i64| {
                let sql = format!(
                    "SELECT * FROM cards WHERE user_id = ? AND deck_id = ? AND suspended = 0 AND {cond}
                     ORDER BY {order} LIMIT ?"
                );
                let user_id = access.user_id();
                let deck_id = s.deck.id;
                async move {
                    Ok::<Vec<Card>, AppError>(
                        sqlx::query_as::<_, Card>(&sql)
                            .bind(user_id)
                            .bind(deck_id)
                            .bind(end_of_day)
                            .bind(limit.max(0))
                            .fetch_all(&self.db)
                            .await?,
                    )
                }
            };
            let reviews_left = presets
                .get(&s.deck.preset_id)
                .map(|p| p.reviews_per_day - s.review_done_today)
                .unwrap_or(0);
            learning.extend(
                fetch("state IN (1, 3) AND due < ?", "due", 1000).await?.into_iter().map(|c| (c, i)),
            );
            reviews.extend(
                fetch("state = 2 AND due < ?", "due, id", reviews_left).await?.into_iter().map(|c| (c, i)),
            );
            new.extend(
                fetch("state = 0 AND ? IS NOT NULL", "position, id", s.new_today)
                    .await?
                    .into_iter()
                    .map(|c| (c, i)),
            );
        }
        learning.sort_by_key(|(c, _)| c.due);
        reviews.sort_by_key(|(c, _)| (c.due, c.id));

        // Interleave new cards evenly among reviews, like the online queue.
        let mut main = Vec::with_capacity(reviews.len() + new.len());
        let (nr, nn) = (reviews.len(), new.len());
        let (mut ri, mut ni) = (reviews.into_iter(), new.into_iter());
        let (mut r_done, mut n_done) = (0usize, 0usize);
        while r_done < nr || n_done < nn {
            let take_new = n_done < nn && (r_done >= nr || n_done * nr <= r_done * nn);
            if take_new {
                main.extend(ni.next());
                n_done += 1;
            } else {
                main.extend(ri.next());
                r_done += 1;
            }
        }

        let mut configs = HashMap::new();
        let mut items = Vec::new();
        for (card, deck_idx) in learning.into_iter().chain(main) {
            let deck = &summaries[deck_idx].deck;
            if !configs.contains_key(&deck.id) {
                configs.insert(deck.id, self.sched_config(access, deck).await?);
            }
            let cfg = &configs[&deck.id];
            let sched = card.sched();
            let mut previews = HashMap::new();
            for r in Rating::ALL {
                let out = scheduler::answer(&fsrs, cfg, card.id, &sched, r, &clock)?;
                previews.insert(rating_key(r), json!({ "label": out.label(), "secs": out.interval_secs }));
            }
            items.push(QueueItem {
                id: card.id,
                deck_id: deck.id,
                deck: deck.name.clone(),
                front: markdown::render(&card.front),
                back: markdown::render(&card.back),
                front_image: card.front_image.as_ref().map(|i| format!("/media/{i}")),
                back_image: card.back_image.as_ref().map(|i| format!("/media/{i}")),
                state: match card.card_state() {
                    CardState::New => "new",
                    CardState::Learning | CardState::Relearning => "learning",
                    CardState::Review => "review",
                },
                reps: card.reps,
                due: if card.card_state() == CardState::New { 0 } else { card.due },
                previews,
            });
        }
        Ok(json!({
            "user": access.user.handle(),
            "user_id": access.user_id(),
            "generated_at": clock.now,
            "valid_until": end_of_day,
            "four_buttons": access.user.four_buttons,
            "items": items,
        }))
    }

    /// Apply answers given offline, oldest first.
    pub async fn offline_sync(&self, access: &Access, mut answers: Vec<OfflineAnswer>) -> Result<Vec<SyncResult>> {
        access.require_review()?;
        if answers.len() > 5000 {
            return Err(AppError::bad("too many answers in one sync"));
        }
        answers.sort_by_key(|a| a.answered_at);
        let now = crate::now();
        let mut results = Vec::with_capacity(answers.len());
        for a in answers {
            let result = |status: &'static str, message: Option<String>| SyncResult {
                client_id: a.client_id.clone(),
                status,
                message,
            };
            if a.client_id.is_empty() || a.client_id.len() > 100 {
                results.push(result("error", Some("bad client_id".into())));
                continue;
            }
            let seen: Option<i64> = sqlx::query_scalar("SELECT id FROM review_log WHERE user_id = ? AND client_id = ?")
                .bind(access.user_id())
                .bind(&a.client_id)
                .fetch_optional(&self.db)
                .await?;
            if seen.is_some() {
                results.push(result("duplicate", None));
                continue;
            }
            let Some(rating) = Rating::parse(&a.rating) else {
                results.push(result("error", Some("bad rating".into())));
                continue;
            };
            if a.answered_at < now - MAX_OFFLINE_AGE_SECS {
                results.push(result("stale", Some("answer is too old".into())));
                continue;
            }
            let duration = a.duration_ms.map(|d| d.clamp(0, 60_000));
            match self
                .answer_card_at(access, a.card_id, rating, duration, Some(a.reps), Some(a.answered_at), Some(&a.client_id))
                .await
            {
                Ok(_) => results.push(result("ok", None)),
                // Answered elsewhere since the queue was downloaded.
                Err(AppError::Conflict(m)) => results.push(result("stale", Some(m))),
                Err(AppError::NotFound) => results.push(result("stale", Some("card no longer exists".into()))),
                Err(AppError::Internal(e)) => return Err(AppError::Internal(e)),
                Err(e) => results.push(result("error", Some(e.to_string()))),
            }
        }
        Ok(results)
    }
}
