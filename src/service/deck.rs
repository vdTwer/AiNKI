//! Decks: creating, listing and counting them.

#[allow(clippy::wildcard_imports, reason = "an area of the service sees the service's shared imports and types")]
use super::*;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Deck {
    pub id: i64,
    #[serde(skip)]
    pub user_id: i64,
    pub name: String,
    pub description: String,
    pub preset_id: i64,
    pub exam_date: Option<NaiveDate>,
    pub created_at: i64,
    pub status: DeckStatus,
}

/// Whether a deck is studied. Paused and archived decks rest: their cards
/// are suspended until the deck is resumed. An archived deck is also put
/// away, out of the deck list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum DeckStatus {
    #[default]
    Active,
    Paused,
    Archived,
}

impl DeckStatus {
    pub fn is_active(self) -> bool {
        self == Self::Active
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeckSummary {
    #[serde(flatten)]
    pub deck: Deck,
    pub preset_name: String,
    pub total: i64,
    pub new: i64,
    pub suspended: i64,
    /// Learning cards due now, or soon enough that studying shows them
    /// already (see `LEARN_AHEAD_SECS`).
    pub learning_due: i64,
    /// Review cards due now, capped by the daily limit.
    pub review_due: i64,
    /// New cards still allowed today.
    pub new_today: i64,
    #[serde(skip)]
    pub new_done_today: i64,
    #[serde(skip)]
    pub review_done_today: i64,
}

impl DeckSummary {
    pub const fn due_total(&self) -> i64 {
        self.learning_due + self.review_due + self.new_today
    }
}

/// Cards of one deck in one state, counted in the database.
#[derive(sqlx::FromRow)]
struct StateCount {
    deck_id: i64,
    state: CardState,
    suspended: bool,
    cnt: i64,
    due_now: i64,
    /// Due within the learn-ahead window.
    due_soon: i64,
}

/// Cards of one deck answered today, by the state they were in.
#[derive(sqlx::FromRow)]
struct DoneToday {
    deck_id: i64,
    state_before: CardState,
    cnt: i64,
}

impl DeckSummary {
    /// A deck's counts from the per-state rows, capped by its preset's
    /// daily limits.
    fn tally(deck: Deck, preset: Option<&Preset>, rows: &[StateCount], done: &[DoneToday]) -> Self {
        let mut s = Self {
            preset_name: preset.map_or_else(Default::default, |p| p.name.clone()),
            deck,
            total: 0,
            new: 0,
            suspended: 0,
            learning_due: 0,
            review_due: 0,
            new_today: 0,
            new_done_today: 0,
            review_done_today: 0,
        };
        for r in rows.iter().filter(|r| r.deck_id == s.deck.id) {
            s.total += r.cnt;
            if r.suspended {
                s.suspended += r.cnt;
                continue;
            }
            match r.state {
                CardState::New => s.new += r.cnt,
                // Studying learns ahead, so these count as due too;
                // otherwise a left session reads "all done" while
                // studying still has cards to show.
                CardState::Learning | CardState::Relearning => s.learning_due += r.due_soon,
                CardState::Review => s.review_due += r.due_now,
            }
        }
        for d in done.iter().filter(|d| d.deck_id == s.deck.id) {
            match d.state_before {
                CardState::New => s.new_done_today += d.cnt,
                CardState::Review => s.review_done_today += d.cnt,
                _ => {}
            }
        }
        if let Some(p) = preset {
            s.new_today = (p.new_per_day - s.new_done_today).clamp(0, s.new);
            s.review_due = (p.reviews_per_day - s.review_done_today).clamp(0, s.review_due);
        }
        s
    }
}

impl App {
    /// # Errors
    ///
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn deck(&self, access: &Access, id: i64) -> Result<Deck> {
        access.require_deck(id)?;
        Ok(sqlx::query_as("SELECT * FROM decks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?)
    }

    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn decks(&self, access: &Access) -> Result<Vec<Deck>> {
        let all: Vec<Deck> = sqlx::query_as("SELECT * FROM decks WHERE user_id = ? ORDER BY name COLLATE NOCASE")
            .bind(access.user_id())
            .fetch_all(&self.db)
            .await?;
        Ok(all.into_iter().filter(|d| access.can_see_deck(d.id)).collect())
    }

    /// The visible deck a card was most recently added to.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn last_used_deck(&self, access: &Access) -> Result<Option<i64>> {
        let recent: Vec<i64> = sqlx::query_scalar(
            "SELECT deck_id FROM cards WHERE user_id = ? ORDER BY created_at DESC, id DESC LIMIT 50",
        )
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        Ok(recent.into_iter().find(|d| access.can_see_deck(*d)))
    }

    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn deck_by_name(&self, access: &Access, name: &str) -> Result<Option<Deck>> {
        let deck: Option<Deck> = sqlx::query_as("SELECT * FROM decks WHERE user_id = ? AND name = ? COLLATE NOCASE")
            .bind(access.user_id())
            .bind(name.trim())
            .fetch_optional(&self.db)
            .await?;
        Ok(deck.filter(|d| access.can_see_deck(d.id)))
    }

    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn deck_summaries(&self, access: &Access) -> Result<Vec<DeckSummary>> {
        let decks = self.decks(access).await?;
        let presets: HashMap<i64, Preset> = self.list_presets(access).await?.into_iter().map(|p| (p.id, p)).collect();
        let clock = access.user.clock();
        let rows: Vec<StateCount> = sqlx::query_as(
            "SELECT deck_id, state, suspended, COUNT(*) AS cnt, SUM(due <= ?) AS due_now,
                    SUM(due <= ?) AS due_soon
             FROM cards WHERE user_id = ? GROUP BY deck_id, state, suspended",
        )
        .bind(clock.now)
        .bind(clock.now + LEARN_AHEAD_SECS)
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        let done: Vec<DoneToday> = sqlx::query_as(
            "SELECT deck_id, state_before, COUNT(DISTINCT card_id) AS cnt FROM review_log
             WHERE user_id = ? AND reviewed_at >= ? AND rating > 0 AND source != 'import'
             GROUP BY deck_id, state_before",
        )
        .bind(access.user_id())
        .bind(clock.start_of_today())
        .fetch_all(&self.db)
        .await?;
        let mut out: Vec<DeckSummary> = decks
            .into_iter()
            .map(|deck| {
                let preset = presets.get(&deck.preset_id);
                DeckSummary::tally(deck, preset, &rows, &done)
            })
            .collect();
        // Catch-up mode: no new cards, and one review budget for all decks.
        let user = &access.user;
        if user.catch_up {
            let done: i64 = out.iter().map(|s| s.review_done_today).sum();
            let mut budget = (user.catch_up_reviews - done).max(0);
            for s in &mut out {
                s.new_today = 0;
                s.review_due = s.review_due.min(budget);
                budget -= s.review_due;
            }
        }
        Ok(out)
    }

    /// How far into today's session, in half cards: (done, total cards).
    /// A card studied today counts fully once it has left learning, half
    /// while it is still being learned and the last answer was right, and
    /// not at all after a miss. So the line only fills when the last card
    /// has really been learned, and a miss takes its share back.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn session_progress(&self, access: &Access, deck_id: Option<i64>) -> Result<(i64, i64)> {
        let summaries: Vec<DeckSummary> = self
            .deck_summaries(access)
            .await?
            .into_iter()
            .filter(|s| deck_id.is_none_or(|id| s.deck.id == id))
            .collect();
        let clock = access.user.clock();

        #[derive(sqlx::FromRow)]
        struct Touched {
            deck_id: i64,
            state: CardState,
            due: i64,
            last_rating: i64,
        }
        let touched: Vec<Touched> = sqlx::query_as(
            "SELECT c.deck_id, c.state, c.due,
                (SELECT r.rating FROM review_log r
                 WHERE r.card_id = c.id AND r.reviewed_at >= ? AND r.rating > 0 AND r.source != 'import'
                 ORDER BY r.reviewed_at DESC, r.id DESC LIMIT 1) AS last_rating
             FROM cards c
             WHERE c.user_id = ? AND c.suspended = 0 AND c.id IN (
                SELECT card_id FROM review_log
                WHERE user_id = ? AND reviewed_at >= ? AND rating > 0 AND source != 'import')",
        )
        .bind(clock.start_of_today())
        .bind(access.user_id())
        .bind(access.user_id())
        .bind(clock.start_of_today())
        .fetch_all(&self.db)
        .await?;

        let (mut done, mut total, mut touched_due_now) = (0, 0, 0);
        for t in touched.iter().filter(|t| summaries.iter().any(|s| s.deck.id == t.deck_id)) {
            total += 1;
            match t.state {
                CardState::Learning | CardState::Relearning => {
                    if t.due <= clock.now + LEARN_AHEAD_SECS {
                        touched_due_now += 1;
                    }
                    if t.last_rating != Rating::Again as i64 {
                        done += 1;
                    }
                }
                _ => done += 2,
            }
        }
        let waiting: i64 = summaries.iter().map(DeckSummary::due_total).sum();
        Ok((done, total + (waiting - touched_due_now).max(0)))
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only or limited to some decks.
    /// - `NotFound` if the preset isn't the user's (when one is given).
    /// - `BadRequest` if the name is empty or too long.
    /// - `Conflict` if another deck already has the name.
    /// - `Internal` if the database fails.
    pub async fn create_deck(
        &self,
        access: &Access,
        name: &str,
        description: &str,
        preset_id: Option<i64>,
    ) -> Result<Deck> {
        access.require_write()?;
        access.require_all_decks()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::bad("deck name is empty"));
        }
        check_len("deck name", name, MAX_NAME)?;
        check_len("description", description, MAX_DESCRIPTION)?;
        if self.deck_by_name(access, name).await?.is_some() {
            return Err(AppError::Conflict(format!("a deck called '{name}' already exists")));
        }
        let preset_id = match preset_id {
            Some(id) => self.preset(access, id).await?.id,
            None => self.default_preset_id(access.user_id()).await?,
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO decks (user_id, name, description, preset_id, created_at)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(access.user_id())
        .bind(name)
        .bind(description.trim())
        .bind(preset_id)
        .bind(now())
        .fetch_one(&self.db)
        .await?;
        self.deck(access, id).await
    }

    /// `exam_date`: `Some("")` clears it.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `NotFound` if the preset isn't the user's (when one is given).
    /// - `BadRequest` if the name is empty or too long, or the exam date isn't `YYYY-MM-DD`.
    /// - `Conflict` if another deck already has the name.
    /// - `Internal` if the database fails.
    pub async fn update_deck(
        &self,
        access: &Access,
        id: i64,
        name: Option<&str>,
        description: Option<&str>,
        preset_id: Option<i64>,
        exam_date: Option<&str>,
    ) -> Result<Deck> {
        access.require_write()?;
        let mut deck = self.deck(access, id).await?;
        if let Some(n) = name {
            let n = n.trim();
            if n.is_empty() {
                return Err(AppError::bad("deck name is empty"));
            }
            check_len("deck name", n, MAX_NAME)?;
            if let Some(other) = self.deck_by_name(access, n).await?
                && other.id != id
            {
                return Err(AppError::Conflict(format!("a deck called '{n}' already exists")));
            }
            deck.name = n.to_string();
        }
        if let Some(d) = description {
            check_len("description", d, MAX_DESCRIPTION)?;
            deck.description = d.trim().to_string();
        }
        if let Some(p) = preset_id {
            deck.preset_id = self.preset(access, p).await?.id;
        }
        if let Some(e) = exam_date {
            let e = e.trim();
            deck.exam_date = if e.is_empty() {
                None
            } else {
                Some(
                    NaiveDate::parse_from_str(e, "%Y-%m-%d")
                        .map_err(|_| AppError::bad("exam date must look like 2026-12-31"))?,
                )
            };
        }
        sqlx::query(
            "UPDATE decks SET name = ?, description = ?, preset_id = ?, exam_date = ? WHERE id = ? AND user_id = ?",
        )
        .bind(&deck.name)
        .bind(&deck.description)
        .bind(deck.preset_id)
        .bind(deck.exam_date)
        .bind(id)
        .bind(access.user_id())
        .execute(&self.db)
        .await?;
        Ok(deck)
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only or limited to some decks.
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn delete_deck(&self, access: &Access, id: i64) -> Result<i64> {
        access.require_write()?;
        access.require_all_decks()?;
        self.deck(access, id).await?;
        let cards: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM cards WHERE deck_id = ?").bind(id).fetch_one(&self.db).await?;
        sqlx::query("DELETE FROM decks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(cards)
    }

    /// Pause, archive or resume a deck. A deck at rest has all its cards
    /// suspended; resuming wakes the ones it suspended and leaves cards that
    /// were suspended by hand as they are.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn set_deck_status(&self, access: &Access, id: i64, status: DeckStatus) -> Result<Deck> {
        access.require_write()?;
        self.deck(access, id).await?;
        let t = now();
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("UPDATE decks SET status = ? WHERE id = ? AND user_id = ?")
            .bind(status)
            .bind(id)
            .bind(access.user_id())
            .execute(&mut *tx)
            .await?;
        let cards = if status.is_active() {
            "UPDATE cards SET suspended = 0, held = 0, updated_at = ?
             WHERE deck_id = ? AND user_id = ? AND held = 1"
        } else {
            "UPDATE cards SET suspended = 1, held = 1, updated_at = ?
             WHERE deck_id = ? AND user_id = ? AND suspended = 0"
        };
        sqlx::query(cards).bind(t).bind(id).bind(access.user_id()).execute(&mut *tx).await?;
        tx.commit().await?;
        self.deck(access, id).await
    }
}
