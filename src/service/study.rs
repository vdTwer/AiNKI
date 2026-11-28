//! Studying: the next card, answers and undo.

#[allow(
    clippy::wildcard_imports,
    reason = "an area of the service sees the service's shared imports and types"
)]
use super::*;

#[derive(Debug, Serialize)]
pub struct AnswerResult {
    pub card: Card,
    pub interval_secs: i64,
    pub interval: String,
}

#[derive(Debug, Serialize)]
pub struct StudyCard {
    pub card: Card,
    pub deck: Deck,
    /// Interval label per rating, in order Again, Hard, Good, Easy.
    pub previews: Vec<(Rating, String)>,
    pub remaining: i64,
}

/// Stores a card's new schedule after an answer, unless another answer got there
/// first (its `reps` moved on): then it's a conflict.
pub(crate) async fn save_answered_card(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    access: &Access,
    card: &Card,
    c: &CardSched,
    now: i64,
) -> Result<()> {
    let updated = sqlx::query(
        "UPDATE cards SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
            last_review = ?, reps = ?, lapses = ?, updated_at = ?
         WHERE id = ? AND user_id = ? AND reps = ?",
    )
    .bind(c.state)
    .bind(c.step)
    .bind(c.stability.map(f64::from))
    .bind(c.difficulty.map(f64::from))
    .bind(c.due)
    .bind(c.last_review)
    .bind(c.reps)
    .bind(c.lapses)
    .bind(now)
    .bind(card.id)
    .bind(access.user_id())
    .bind(card.reps)
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "this card was already answered".into(),
        ));
    }
    Ok(())
}

/// Whether the next card should be a new one: new cards and reviews are
/// interleaved so both progress at the same rate through the day.
fn prefer_new(summaries: &[DeckSummary]) -> bool {
    let (new_done, new_left, rev_done, rev_left) =
        summaries.iter().fold((0, 0, 0, 0), |a, s| {
            (
                a.0 + s.new_done_today,
                a.1 + s.new_today,
                a.2 + s.review_done_today,
                a.3 + s.review_due,
            )
        });
    new_left > 0
        && (rev_left == 0
            || (num::i64_f64(new_done) / num::i64_f64(new_done + new_left))
                <= (num::i64_f64(rev_done)
                    / num::i64_f64((rev_done + rev_left).max(1))))
}

impl App {
    pub(crate) async fn sched_config(
        &self,
        access: &Access,
        deck: &Deck,
    ) -> Result<SchedConfig> {
        sched_config_for(&self.preset(access, deck.preset_id).await?, deck)
    }

    /// Pick the next card to study, from one deck or all visible decks.
    ///
    /// # Errors
    ///
    /// - `NotFound` if a deck is given that the access doesn't see.
    /// - `Internal` if the database fails or a preset is broken.
    pub async fn next_card(
        &self,
        access: &Access,
        deck_id: Option<i64>,
    ) -> Result<Option<StudyCard>> {
        let summaries: Vec<DeckSummary> = self
            .deck_summaries(access)
            .await?
            .into_iter()
            .filter(|s| deck_id.is_none_or(|id| s.deck.id == id))
            .collect();
        if let Some(id) = deck_id
            && summaries.is_empty()
        {
            self.deck(access, id).await?;
        }
        let remaining: i64 = summaries.iter().map(DeckSummary::due_total).sum();
        let n = now();
        let all_ids: Vec<i64> = summaries.iter().map(|s| s.deck.id).collect();
        let review_ids: Vec<i64> = summaries
            .iter()
            .filter(|s| s.review_due > 0)
            .map(|s| s.deck.id)
            .collect();
        let new_ids: Vec<i64> = summaries
            .iter()
            .filter(|s| s.new_today > 0)
            .map(|s| s.deck.id)
            .collect();

        let pick = |ids: &[i64], cond: &str, order: &str| {
            let sql = format!(
                "SELECT * FROM cards WHERE user_id = ? AND suspended = 0 AND deck_id IN (SELECT value FROM json_each(?)) AND {cond}
                 ORDER BY {order} LIMIT 1",
            );
            let ids = ids.to_vec();
            let user_id = access.user_id();
            async move {
                if ids.is_empty() {
                    return Ok::<Option<Card>, AppError>(None);
                }
                let q = sqlx::query_as::<_, Card>(&sql)
                    .bind(user_id)
                    .bind(id_list(&ids));
                Ok(q.bind(n).fetch_optional(&self.db).await?)
            }
        };

        let learning =
            pick(&all_ids, "state IN (1, 3) AND due <= ?", "due").await?;
        // Catching up, the cards most likely still remembered come first:
        // the least time gone relative to their stability.
        let review_order = if access.user.catch_up {
            format!("({n} - last_review) / MAX(stability, 0.1), id")
        } else {
            "due, id".to_string()
        };
        let review =
            pick(&review_ids, "state = 2 AND due <= ?", &review_order).await?;
        let new = pick(&new_ids, "state = 0 AND ? IS NOT NULL", "position, id")
            .await?;

        let prefer_new = prefer_new(&summaries);

        let chosen = match (learning, review, new) {
            (Some(c), _, _) => Some(c),
            (None, Some(r), Some(nw)) => Some(if prefer_new { nw } else { r }),
            (None, Some(r), None) => Some(r),
            (None, None, Some(nw)) => Some(nw),
            (None, None, None) => {
                let ahead = format!(
                    "state IN (1, 3) AND due <= ? + {LEARN_AHEAD_SECS}"
                );
                pick(&all_ids, &ahead, "due").await?
            }
        };
        let Some(card) = chosen else {
            return Ok(None);
        };
        let deck = summaries
            .into_iter()
            .find(|s| s.deck.id == card.deck_id)
            .map(|s| s.deck)
            .ok_or(AppError::NotFound)?;
        let previews = self.previews(access, &card, &deck).await?;
        Ok(Some(StudyCard {
            card,
            deck,
            previews,
            remaining: remaining.max(1),
        }))
    }

    /// # Errors
    ///
    /// `Internal` if the database fails or the deck's preset or FSRS parameters are broken.
    pub async fn previews(
        &self,
        access: &Access,
        card: &Card,
        deck: &Deck,
    ) -> Result<Vec<(Rating, String)>> {
        let cfg = self.sched_config(access, deck).await?;
        let params = self.deck_params(access, deck.id).await?;
        let fsrs = scheduler::fsrs_for(&params)?;
        let clock = access.user.clock();
        let sched = card.sched();
        Rating::ALL
            .iter()
            .map(|&r| {
                let out =
                    scheduler::answer(&fsrs, &cfg, card.id, &sched, r, &clock)?;
                Ok((r, out.label()))
            })
            .collect()
    }

    /// Record an answer. `expected_reps` guards against double submissions.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token may not submit reviews.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Conflict` if the card was answered since it was shown (`expected_reps` differs).
    /// - `Internal` if the database fails or the FSRS parameters are broken.
    pub async fn answer_card(
        &self,
        access: &Access,
        card_id: i64,
        rating: Rating,
        duration_ms: Option<i64>,
        expected_reps: Option<i64>,
    ) -> Result<AnswerResult> {
        access.require_review()?;
        let card = self.card(access, card_id).await?;
        if let Some(reps) = expected_reps
            && reps != card.reps
        {
            return Err(AppError::Conflict(
                "this card was already answered".into(),
            ));
        }
        if card.multi.is_some() {
            // A plain rating on a multicard: Again missed every asked item,
            // anything else remembered them all.
            let missed = if rating == Rating::Again {
                Missed::All
            } else {
                Missed::These(vec![])
            };
            return self.answer_items(access, &card, missed, duration_ms).await;
        }
        let deck = self.deck(access, card.deck_id).await?;
        let cfg = self.sched_config(access, &deck).await?;
        let params = self.deck_params(access, deck.id).await?;
        let fsrs = scheduler::fsrs_for(&params)?;
        let clock = access.user.clock();
        let before = card.sched();
        let out =
            scheduler::answer(&fsrs, &cfg, card.id, &before, rating, &clock)?;
        let c = &out.card;

        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        save_answered_card(&mut tx, access, &card, c, clock.now).await?;
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, stability, difficulty, duration_ms, source, prev_card)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(access.user_id())
        .bind(card.id)
        .bind(card.deck_id)
        .bind(rating as i64)
        .bind(clock.now)
        .bind(card.state)
        .bind(out.elapsed_days)
        .bind(out.interval_secs)
        .bind(c.stability.map(f64::from))
        .bind(c.difficulty.map(f64::from))
        .bind(duration_ms.map(|d| d.clamp(0, 3_600_000)))
        .bind(access.source())
        .bind(serde_json::to_string(&before).ok())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(AnswerResult {
            card: self.card(access, card.id).await?,
            interval_secs: out.interval_secs,
            interval: out.label(),
        })
    }

    /// Answer a list card by naming the items that were missed (positions
    /// from 0); every other asked item was remembered.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token may not submit reviews.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `BadRequest` if the card is no multicard.
    /// - `Conflict` if the card was answered since it was shown.
    /// - `Internal` if the database fails or the FSRS parameters are broken.
    pub async fn answer_list(
        &self,
        access: &Access,
        card_id: i64,
        missed: Vec<usize>,
        duration_ms: Option<i64>,
        expected_reps: Option<i64>,
    ) -> Result<AnswerResult> {
        access.require_review()?;
        let card = self.card(access, card_id).await?;
        if card.multi.is_none() {
            return Err(AppError::bad(
                "this card is not learned item by item; answer it with a rating",
            ));
        }
        if let Some(reps) = expected_reps
            && reps != card.reps
        {
            return Err(AppError::Conflict(
                "this card was already answered".into(),
            ));
        }
        self.answer_items(access, &card, Missed::These(missed), duration_ms)
            .await
    }

    /// Undo the most recent answer made through the same channel (web or
    /// this agent). Returns the restored card.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token may not submit reviews.
    /// - `Internal` if the database fails. Nothing to undo is `Ok(None)`.
    pub async fn undo_last(&self, access: &Access) -> Result<Option<Card>> {
        access.require_review()?;
        #[derive(sqlx::FromRow)]
        struct Last {
            id: i64,
            card_id: i64,
            prev_card: Option<String>,
            prev_items: Option<String>,
        }
        let last: Option<Last> = sqlx::query_as(
            "SELECT id, card_id, prev_card, prev_items FROM review_log
             WHERE user_id = ? AND source = ? AND rating > 0 AND reviewed_at > ?
             ORDER BY id DESC LIMIT 1",
        )
        .bind(access.user_id())
        .bind(access.source())
        .bind(now() - 86_400)
        .fetch_optional(&self.db)
        .await?;
        let Some(last) = last else { return Ok(None) };
        let card = self.card(access, last.card_id).await?;
        let newest: i64 = sqlx::query_scalar(
            "SELECT MAX(id) FROM review_log WHERE card_id = ?",
        )
        .bind(card.id)
        .fetch_one(&self.db)
        .await?;
        // Imported history has no snapshot to go back to; a snapshot that
        // fails to parse is damage, not "nothing to undo".
        let Some(prev) = last.prev_card.as_deref() else { return Ok(None) };
        let prev: CardSched = serde_json::from_str(prev).map_err(|e| {
            AppError::Internal(anyhow::anyhow!(
                "review {} has a broken snapshot: {e}",
                last.id
            ))
        })?;
        if newest != last.id {
            return Ok(None);
        }
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE cards SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                last_review = ?, reps = ?, lapses = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(prev.state as i64)
        .bind(prev.step)
        .bind(prev.stability.map(f64::from))
        .bind(prev.difficulty.map(f64::from))
        // Make it due right away so it comes up next.
        .bind(if prev.state == CardState::New { 0 } else { prev.due.min(now()) })
        .bind(prev.last_review)
        .bind(prev.reps)
        .bind(prev.lapses)
        .bind(now())
        .bind(card.id)
        .bind(access.user_id())
        .execute(&mut *tx)
        .await?;
        if let Some(items) = &last.prev_items {
            self.restore_items(&mut tx, card.id, items).await?;
        }
        sqlx::query("DELETE FROM review_log WHERE id = ? AND user_id = ?")
            .bind(last.id)
            .bind(access.user_id())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(self.card(access, card.id).await?))
    }

    /// When the next already-learned card falls due (after now), in the
    /// given deck or all visible decks.
    ///
    /// # Errors
    ///
    /// - `NotFound` if a deck is given that the access doesn't see.
    /// - `Internal` if the database fails.
    pub async fn next_review_at(
        &self,
        access: &Access,
        deck_id: Option<i64>,
    ) -> Result<Option<i64>> {
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => vec![self.deck(access, id).await?.id],
            None => {
                self.decks(access).await?.into_iter().map(|d| d.id).collect()
            }
        };
        if deck_ids.is_empty() {
            return Ok(None);
        }
        let sql = "SELECT MIN(due) FROM cards WHERE user_id = ? AND suspended = 0 AND state != 0
             AND due > ? AND deck_id IN (SELECT value FROM json_each(?))";
        let q = sqlx::query_scalar::<_, Option<i64>>(sql)
            .bind(access.user_id())
            .bind(now())
            .bind(id_list(&deck_ids));
        Ok(q.fetch_one(&self.db).await?)
    }

    /// # Errors
    ///
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn card_history(
        &self,
        access: &Access,
        card_id: i64,
    ) -> Result<Vec<Value>> {
        self.card(access, card_id).await?;
        #[derive(sqlx::FromRow)]
        struct Row {
            rating: i64,
            reviewed_at: i64,
            state_before: CardState,
            scheduled_secs: i64,
            stability: Option<f64>,
            source: String,
            items_asked: Option<i64>,
            items_recalled: Option<i64>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT rating, reviewed_at, state_before, scheduled_secs, stability, source,
                items_asked, items_recalled
             FROM review_log WHERE card_id = ? ORDER BY reviewed_at, id",
        )
        .bind(card_id)
        .fetch_all(&self.db)
        .await?;
        let clock = access.user.clock();
        Ok(rows
            .into_iter()
            .map(|r| {
                json!({
                    "at": crate::format::iso(r.reviewed_at, &clock),
                    "timestamp": r.reviewed_at,
                    "rating": match r.rating { 0 => "reset", 1 => "again", 2 => "hard", 3 => "good", _ => "easy" },
                    "state_before": r.state_before.as_str(),
                    "next_interval": (r.source != "import" && r.rating > 0)
                        .then(|| crate::format::interval(r.scheduled_secs)),
                    "stability_days": r.stability.map(|s| (s * 100.0).round() / 100.0),
                    "source": r.source,
                    "items_asked": r.items_asked,
                    "items_recalled": r.items_recalled,
                })
            })
            .collect())
    }
}
