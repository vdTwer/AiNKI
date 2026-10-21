//! Everything a user can do, in one place. The website and the MCP server are
//! both thin layers over these functions, so agents can do whatever humans can
//! (within the limits of their token).

use std::collections::HashMap;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    App,
    auth::{Access, Actor},
    error::{AppError, Result},
    now, presets,
    scheduler::{self, CardSched, CardState, Clock, Rating, SchedConfig},
};

/// Show learning cards up to this many seconds early when nothing else is due.
const LEARN_AHEAD_SECS: i64 = 20 * 60;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Preset {
    pub id: i64,
    #[serde(skip)]
    pub user_id: i64,
    pub name: String,
    pub description: String,
    pub desired_retention: f64,
    pub new_per_day: i64,
    pub reviews_per_day: i64,
    pub learning_steps: String,
    pub relearning_steps: String,
    pub max_interval: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Deck {
    pub id: i64,
    #[serde(skip)]
    pub user_id: i64,
    pub name: String,
    pub description: String,
    pub preset_id: i64,
    pub exam_date: Option<String>,
    pub created_at: i64,
}

impl Deck {
    pub fn exam(&self) -> Option<NaiveDate> {
        self.exam_date
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Card {
    pub id: i64,
    #[serde(skip)]
    pub user_id: i64,
    pub deck_id: i64,
    pub front: String,
    pub back: String,
    pub front_image: Option<String>,
    pub back_image: Option<String>,
    pub state: i64,
    pub step: i64,
    pub stability: Option<f64>,
    pub difficulty: Option<f64>,
    pub due: i64,
    pub last_review: Option<i64>,
    pub reps: i64,
    pub lapses: i64,
    pub suspended: bool,
    pub position: i64,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Card {
    pub fn sched(&self) -> CardSched {
        CardSched {
            state: CardState::from_i64(self.state),
            step: self.step,
            stability: self.stability.map(|v| v as f32),
            difficulty: self.difficulty.map(|v| v as f32),
            due: self.due,
            last_review: self.last_review,
            reps: self.reps,
            lapses: self.lapses,
        }
    }

    pub fn card_state(&self) -> CardState {
        CardState::from_i64(self.state)
    }

    /// JSON shape used by the MCP API.
    pub fn to_json(&self, clock: &Clock, params: &[f32]) -> Value {
        let state = self.card_state();
        let due = if state == CardState::New {
            Value::Null
        } else {
            json!(crate::format::iso(self.due, clock))
        };
        json!({
            "id": self.id,
            "deck_id": self.deck_id,
            "front": self.front,
            "back": self.back,
            "front_image": self.front_image,
            "back_image": self.back_image,
            "state": state.as_str(),
            "suspended": self.suspended,
            "due": due,
            "is_due": state != CardState::New && self.due <= clock.now,
            "stability_days": self.stability.map(|s| (s * 100.0).round() / 100.0),
            "difficulty": self.difficulty.map(|d| (d * 100.0).round() / 100.0),
            "retrievability": scheduler::retrievability(&self.sched(), params, clock)
                .map(|r| (r as f64 * 1000.0).round() / 1000.0),
            "reps": self.reps,
            "lapses": self.lapses,
            "created_by": self.created_by,
        })
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
    /// Learning cards due now.
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
    pub fn due_total(&self) -> i64 {
        self.learning_due + self.review_due + self.new_today
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct PresetInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub desired_retention: Option<f64>,
    pub new_per_day: Option<i64>,
    pub reviews_per_day: Option<i64>,
    pub learning_steps: Option<String>,
    pub relearning_steps: Option<String>,
    pub max_interval: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CardInput {
    pub front: Option<String>,
    pub back: Option<String>,
    /// `Some(None)` clears the image.
    pub front_image: Option<Option<String>>,
    pub back_image: Option<Option<String>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CardFilter {
    pub deck_id: Option<i64>,
    pub query: Option<String>,
    /// new | learning | review | relearning | due | suspended
    pub state: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

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

#[derive(Debug, Serialize)]
pub struct Stats {
    pub total: i64,
    pub new: i64,
    pub learning: i64,
    pub review: i64,
    pub suspended: i64,
    pub due_now: i64,
    pub reviews_today: i64,
    /// Different cards answered today (a card can be answered more than once).
    pub cards_today: i64,
    pub minutes_today: f64,
    /// Share of mature-ish (review-state) recalls that succeeded, last 30 days.
    pub true_retention_30d: Option<f64>,
    /// How many review-state answers the retention figure is based on.
    pub retention_sample_30d: i64,
    pub reviews_30d: i64,
    pub streak_days: i64,
    /// Number of cards falling due on each of the next 14 days, by date
    /// ("2026-10-21").
    pub forecast: Vec<(String, i64)>,
    pub average_retrievability: Option<f64>,
    /// Typical thinking time per answer over the last 30 days, in seconds.
    pub seconds_per_review: Option<f64>,
}

pub const MAX_CARD_TEXT: usize = 20_000;
pub const MAX_NAME: usize = 200;
pub const MAX_DESCRIPTION: usize = 2_000;

/// Reject oversized text so one request can't bloat the database or pages.
fn check_len(what: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max {
        Err(AppError::bad(format!("{what} is too long (at most {max} characters)")))
    } else {
        Ok(())
    }
}

pub(crate) fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

impl App {
    // ----- presets -------------------------------------------------------

    pub async fn list_presets(&self, access: &Access) -> Result<Vec<Preset>> {
        Ok(sqlx::query_as("SELECT * FROM presets WHERE user_id = ? ORDER BY id")
            .bind(access.user_id())
            .fetch_all(&self.db)
            .await?)
    }

    pub async fn preset(&self, access: &Access, id: i64) -> Result<Preset> {
        Ok(sqlx::query_as("SELECT * FROM presets WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?)
    }

    /// Validate a preset and normalize its step lists (e.g. "1m,10m" -> "1m 10m").
    fn validate_preset(p: &mut Preset) -> Result<()> {
        if p.name.trim().is_empty() {
            return Err(AppError::bad("preset name is empty"));
        }
        check_len("preset name", &p.name, MAX_NAME)?;
        check_len("description", &p.description, MAX_DESCRIPTION)?;
        if !(0.70..=0.99).contains(&p.desired_retention) {
            return Err(AppError::bad("desired retention must be between 0.70 and 0.99"));
        }
        if p.new_per_day < 0 || p.reviews_per_day < 0 {
            return Err(AppError::bad("daily limits can't be negative"));
        }
        if p.max_interval < 1 {
            return Err(AppError::bad("maximum interval must be at least 1 day"));
        }
        let learning = scheduler::parse_steps(&p.learning_steps).map_err(AppError::BadRequest)?;
        let relearning = scheduler::parse_steps(&p.relearning_steps).map_err(AppError::BadRequest)?;
        p.learning_steps = scheduler::format_steps(&learning);
        p.relearning_steps = scheduler::format_steps(&relearning);
        Ok(())
    }

    fn apply_preset_input(p: &mut Preset, input: PresetInput) {
        if let Some(v) = input.name {
            p.name = v.trim().to_string();
        }
        if let Some(v) = input.description {
            p.description = v;
        }
        if let Some(v) = input.desired_retention {
            p.desired_retention = v;
        }
        if let Some(v) = input.new_per_day {
            p.new_per_day = v;
        }
        if let Some(v) = input.reviews_per_day {
            p.reviews_per_day = v;
        }
        if let Some(v) = input.learning_steps {
            p.learning_steps = v;
        }
        if let Some(v) = input.relearning_steps {
            p.relearning_steps = v;
        }
        if let Some(v) = input.max_interval {
            p.max_interval = v;
        }
    }

    /// Create a preset, starting from a built-in template (default: standard).
    pub async fn create_preset(
        &self,
        access: &Access,
        template: Option<&str>,
        input: PresetInput,
    ) -> Result<Preset> {
        access.require_write()?;
        access.require_all_decks()?;
        let t = presets::template(template.unwrap_or("standard"))
            .ok_or_else(|| AppError::bad("unknown preset template"))?;
        let mut p = Preset {
            id: 0,
            user_id: access.user_id(),
            name: format!("{} (custom)", t.name),
            description: t.description.to_string(),
            desired_retention: t.desired_retention,
            new_per_day: t.new_per_day,
            reviews_per_day: t.reviews_per_day,
            learning_steps: t.learning_steps.to_string(),
            relearning_steps: t.relearning_steps.to_string(),
            max_interval: t.max_interval,
            created_at: now(),
        };
        Self::apply_preset_input(&mut p, input);
        Self::validate_preset(&mut p)?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO presets (user_id, name, description, desired_retention, new_per_day,
                reviews_per_day, learning_steps, relearning_steps, max_interval, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(p.user_id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.desired_retention)
        .bind(p.new_per_day)
        .bind(p.reviews_per_day)
        .bind(&p.learning_steps)
        .bind(&p.relearning_steps)
        .bind(p.max_interval)
        .bind(p.created_at)
        .fetch_one(&self.db)
        .await?;
        self.preset(access, id).await
    }

    pub async fn update_preset(&self, access: &Access, id: i64, input: PresetInput) -> Result<Preset> {
        access.require_write()?;
        access.require_all_decks()?;
        let mut p = self.preset(access, id).await?;
        Self::apply_preset_input(&mut p, input);
        Self::validate_preset(&mut p)?;
        sqlx::query(
            "UPDATE presets SET name = ?, description = ?, desired_retention = ?, new_per_day = ?,
                reviews_per_day = ?, learning_steps = ?, relearning_steps = ?, max_interval = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.desired_retention)
        .bind(p.new_per_day)
        .bind(p.reviews_per_day)
        .bind(&p.learning_steps)
        .bind(&p.relearning_steps)
        .bind(p.max_interval)
        .bind(id)
        .bind(access.user_id())
        .execute(&self.db)
        .await?;
        Ok(p)
    }

    pub async fn delete_preset(&self, access: &Access, id: i64) -> Result<()> {
        access.require_write()?;
        access.require_all_decks()?;
        self.preset(access, id).await?;
        let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM decks WHERE preset_id = ?")
            .bind(id)
            .fetch_one(&self.db)
            .await?;
        if used > 0 {
            return Err(AppError::bad(format!(
                "{} still {} this preset; switch {} to another preset first",
                crate::format::count(used, "deck"),
                if used == 1 { "uses" } else { "use" },
                if used == 1 { "it" } else { "them" }
            )));
        }
        sqlx::query("DELETE FROM presets WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn default_preset_id(&self, user_id: i64) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT id FROM presets WHERE user_id = ? ORDER BY id LIMIT 1")
                .bind(user_id)
                .fetch_optional(&self.db)
                .await?
                .ok_or_else(|| AppError::bad("create a preset first"))?,
        )
    }

    // ----- decks ---------------------------------------------------------

    pub async fn deck(&self, access: &Access, id: i64) -> Result<Deck> {
        access.require_deck(id)?;
        Ok(sqlx::query_as("SELECT * FROM decks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?)
    }

    pub async fn decks(&self, access: &Access) -> Result<Vec<Deck>> {
        let all: Vec<Deck> =
            sqlx::query_as("SELECT * FROM decks WHERE user_id = ? ORDER BY name COLLATE NOCASE")
                .bind(access.user_id())
                .fetch_all(&self.db)
                .await?;
        Ok(all.into_iter().filter(|d| access.can_see_deck(d.id)).collect())
    }

    /// The visible deck a card was most recently added to.
    pub async fn last_used_deck(&self, access: &Access) -> Result<Option<i64>> {
        let recent: Vec<i64> = sqlx::query_scalar(
            "SELECT deck_id FROM cards WHERE user_id = ? ORDER BY created_at DESC, id DESC LIMIT 50",
        )
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        Ok(recent.into_iter().find(|d| access.can_see_deck(*d)))
    }

    pub async fn deck_by_name(&self, access: &Access, name: &str) -> Result<Option<Deck>> {
        let deck: Option<Deck> =
            sqlx::query_as("SELECT * FROM decks WHERE user_id = ? AND name = ? COLLATE NOCASE")
                .bind(access.user_id())
                .bind(name.trim())
                .fetch_optional(&self.db)
                .await?;
        Ok(deck.filter(|d| access.can_see_deck(d.id)))
    }

    pub async fn deck_summaries(&self, access: &Access) -> Result<Vec<DeckSummary>> {
        let decks = self.decks(access).await?;
        let presets: HashMap<i64, Preset> = self
            .list_presets(access)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
        let clock = access.user.clock();
        let n = clock.now;

        #[derive(sqlx::FromRow)]
        struct Row {
            deck_id: i64,
            state: i64,
            suspended: bool,
            cnt: i64,
            due_now: i64,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT deck_id, state, suspended, COUNT(*) AS cnt, SUM(due <= ?) AS due_now
             FROM cards WHERE user_id = ? GROUP BY deck_id, state, suspended",
        )
        .bind(n)
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;

        #[derive(sqlx::FromRow)]
        struct Done {
            deck_id: i64,
            state_before: i64,
            cnt: i64,
        }
        let done: Vec<Done> = sqlx::query_as(
            "SELECT deck_id, state_before, COUNT(DISTINCT card_id) AS cnt FROM review_log
             WHERE user_id = ? AND reviewed_at >= ? AND rating > 0 AND source != 'import'
             GROUP BY deck_id, state_before",
        )
        .bind(access.user_id())
        .bind(clock.start_of_today())
        .fetch_all(&self.db)
        .await?;

        let mut out = Vec::new();
        for deck in decks {
            let preset = presets.get(&deck.preset_id);
            let mut s = DeckSummary {
                preset_name: preset.map(|p| p.name.clone()).unwrap_or_default(),
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
                match CardState::from_i64(r.state) {
                    CardState::New => s.new += r.cnt,
                    CardState::Learning | CardState::Relearning => s.learning_due += r.due_now,
                    CardState::Review => s.review_due += r.due_now,
                }
            }
            for d in done.iter().filter(|d| d.deck_id == s.deck.id) {
                match CardState::from_i64(d.state_before) {
                    CardState::New => s.new_done_today += d.cnt,
                    CardState::Review => s.review_done_today += d.cnt,
                    _ => {}
                }
            }
            if let Some(p) = preset {
                s.new_today = (p.new_per_day - s.new_done_today).clamp(0, s.new);
                s.review_due = (p.reviews_per_day - s.review_done_today).clamp(0, s.review_due);
            }
            out.push(s);
        }
        Ok(out)
    }

    /// How far into today's session, in half cards: (done, total cards).
    /// A card studied today counts fully once it has left learning, half
    /// while it is still being learned and the last answer was right, and
    /// not at all after a miss. So the line only fills when the last card
    /// has really been learned, and a miss takes its share back.
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
            state: i64,
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
            match CardState::from_i64(t.state) {
                CardState::Learning | CardState::Relearning => {
                    if t.due <= clock.now {
                        touched_due_now += 1;
                    }
                    if t.last_rating != Rating::Again as i64 {
                        done += 1;
                    }
                }
                _ => done += 2,
            }
        }
        let waiting: i64 = summaries.iter().map(|s| s.due_total()).sum();
        Ok((done, total + (waiting - touched_due_now).max(0)))
    }

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
            if let Some(other) = self.deck_by_name(access, n).await? {
                if other.id != id {
                    return Err(AppError::Conflict(format!("a deck called '{n}' already exists")));
                }
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
                NaiveDate::parse_from_str(e, "%Y-%m-%d")
                    .map_err(|_| AppError::bad("exam date must look like 2026-12-31"))?;
                Some(e.to_string())
            };
        }
        sqlx::query(
            "UPDATE decks SET name = ?, description = ?, preset_id = ?, exam_date = ? WHERE id = ?",
        )
        .bind(&deck.name)
        .bind(&deck.description)
        .bind(deck.preset_id)
        .bind(&deck.exam_date)
        .bind(id)
        .execute(&self.db)
        .await?;
        Ok(deck)
    }

    pub async fn delete_deck(&self, access: &Access, id: i64) -> Result<i64> {
        access.require_write()?;
        access.require_all_decks()?;
        self.deck(access, id).await?;
        let cards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cards WHERE deck_id = ?")
            .bind(id)
            .fetch_one(&self.db)
            .await?;
        sqlx::query("DELETE FROM decks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(cards)
    }

    // ----- cards ---------------------------------------------------------

    pub async fn card(&self, access: &Access, id: i64) -> Result<Card> {
        let card: Card = sqlx::query_as("SELECT * FROM cards WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?;
        access.require_deck(card.deck_id)?;
        Ok(card)
    }

    async fn check_media(&self, access: &Access, id: &Option<String>) -> Result<()> {
        if let Some(id) = id {
            let found: Option<String> =
                sqlx::query_scalar("SELECT id FROM media WHERE id = ? AND user_id = ?")
                    .bind(id)
                    .bind(access.user_id())
                    .fetch_optional(&self.db)
                    .await?;
            if found.is_none() {
                return Err(AppError::bad(format!("unknown image id '{id}'")));
            }
        }
        Ok(())
    }

    pub async fn create_card(
        &self,
        access: &Access,
        deck_id: i64,
        front: &str,
        back: &str,
        front_image: Option<String>,
        back_image: Option<String>,
    ) -> Result<Card> {
        access.require_write()?;
        self.deck(access, deck_id).await?;
        if front.trim().is_empty() && front_image.is_none() {
            return Err(AppError::bad("the front needs text or an image"));
        }
        check_len("front", front, MAX_CARD_TEXT)?;
        check_len("back", back, MAX_CARD_TEXT)?;
        self.check_media(access, &front_image).await?;
        self.check_media(access, &back_image).await?;
        let source = match access.actor {
            Actor::Web => "web",
            Actor::Agent { .. } => "agent",
        };
        let id = self
            .insert_card(access.user_id(), deck_id, front, back, front_image, back_image, source)
            .await?;
        self.card(access, id).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_card(
        &self,
        user_id: i64,
        deck_id: i64,
        front: &str,
        back: &str,
        front_image: Option<String>,
        back_image: Option<String>,
        created_by: &str,
    ) -> Result<i64> {
        let t = now();
        // Imports and copies are not validated field by field; cap them here.
        let cap = |x: &str| -> String { x.trim().chars().take(MAX_CARD_TEXT).collect() };
        let (front, back) = (cap(front), cap(back));
        Ok(sqlx::query_scalar(
            "INSERT INTO cards (user_id, deck_id, front, back, front_image, back_image, position,
                created_by, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM cards WHERE user_id = ?), ?, ?, ?)
             RETURNING id",
        )
        .bind(user_id)
        .bind(deck_id)
        .bind(front)
        .bind(back)
        .bind(front_image)
        .bind(back_image)
        .bind(user_id)
        .bind(created_by)
        .bind(t)
        .bind(t)
        .fetch_one(&self.db)
        .await?)
    }

    pub async fn update_card(&self, access: &Access, id: i64, input: CardInput) -> Result<Card> {
        access.require_write()?;
        let mut card = self.card(access, id).await?;
        if let Some(f) = input.front {
            card.front = f.trim().to_string();
        }
        if let Some(b) = input.back {
            card.back = b.trim().to_string();
        }
        if let Some(img) = input.front_image {
            self.check_media(access, &img).await?;
            card.front_image = img;
        }
        if let Some(img) = input.back_image {
            self.check_media(access, &img).await?;
            card.back_image = img;
        }
        if card.front.is_empty() && card.front_image.is_none() {
            return Err(AppError::bad("the front needs text or an image"));
        }
        check_len("front", &card.front, MAX_CARD_TEXT)?;
        check_len("back", &card.back, MAX_CARD_TEXT)?;
        sqlx::query(
            "UPDATE cards SET front = ?, back = ?, front_image = ?, back_image = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(&card.front)
        .bind(&card.back)
        .bind(&card.front_image)
        .bind(&card.back_image)
        .bind(now())
        .bind(id)
        .execute(&self.db)
        .await?;
        self.card(access, id).await
    }

    pub async fn delete_card(&self, access: &Access, id: i64) -> Result<()> {
        access.require_write()?;
        self.card(access, id).await?;
        sqlx::query("DELETE FROM cards WHERE id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn move_card(&self, access: &Access, id: i64, deck_id: i64) -> Result<Card> {
        access.require_write()?;
        self.card(access, id).await?;
        self.deck(access, deck_id).await?;
        sqlx::query("UPDATE cards SET deck_id = ?, updated_at = ? WHERE id = ?")
            .bind(deck_id)
            .bind(now())
            .bind(id)
            .execute(&self.db)
            .await?;
        self.card(access, id).await
    }

    pub async fn set_suspended(&self, access: &Access, id: i64, suspended: bool) -> Result<Card> {
        access.require_write()?;
        self.card(access, id).await?;
        sqlx::query("UPDATE cards SET suspended = ?, updated_at = ? WHERE id = ?")
            .bind(suspended)
            .bind(now())
            .bind(id)
            .execute(&self.db)
            .await?;
        self.card(access, id).await
    }

    /// Forget all progress; the card becomes new again. Review history is
    /// kept but the optimizer ignores everything before the reset.
    pub async fn reset_card(&self, access: &Access, id: i64) -> Result<Card> {
        access.require_write()?;
        let card = self.card(access, id).await?;
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "UPDATE cards SET state = 0, step = 0, stability = NULL, difficulty = NULL, due = 0,
                last_review = NULL, reps = 0, lapses = 0, updated_at = ? WHERE id = ?",
        )
        .bind(now())
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, source)
             VALUES (?, ?, ?, 0, ?, ?, 0, 0, 'manual')",
        )
        .bind(access.user_id())
        .bind(id)
        .bind(card.deck_id)
        .bind(now())
        .bind(card.state)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.card(access, id).await
    }

    pub async fn list_cards(&self, access: &Access, filter: &CardFilter) -> Result<(Vec<Card>, i64)> {
        let deck_ids: Vec<i64> = match filter.deck_id {
            Some(id) => {
                self.deck(access, id).await?;
                vec![id]
            }
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        if deck_ids.is_empty() {
            return Ok((vec![], 0));
        }
        let mut cond = format!("user_id = ? AND deck_id IN ({})", placeholders(deck_ids.len()));
        let query = filter.query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        if query.is_some() {
            cond.push_str(" AND (front LIKE ? ESCAPE '\\' OR back LIKE ? ESCAPE '\\')");
        }
        match filter.state.as_deref().unwrap_or("") {
            "" | "all" => {}
            "new" => cond.push_str(" AND state = 0 AND suspended = 0"),
            "learning" => cond.push_str(" AND state IN (1, 3) AND suspended = 0"),
            "review" => cond.push_str(" AND state = 2 AND suspended = 0"),
            "relearning" => cond.push_str(" AND state = 3 AND suspended = 0"),
            "due" => cond.push_str(" AND state != 0 AND suspended = 0 AND due <= ?"),
            "suspended" => cond.push_str(" AND suspended = 1"),
            other => return Err(AppError::bad(format!("unknown state filter '{other}'"))),
        }
        let like = query.map(|q| {
            format!(
                "%{}%",
                q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
            )
        });
        let due_bound = filter.state.as_deref() == Some("due");
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let offset = filter.offset.unwrap_or(0).max(0);

        let count_sql = format!("SELECT COUNT(*) FROM cards WHERE {cond}");
        let list_sql = format!(
            "SELECT * FROM cards WHERE {cond} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        );
        let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql).bind(access.user_id());
        let mut list_q = sqlx::query_as::<_, Card>(&list_sql).bind(access.user_id());
        for id in &deck_ids {
            count_q = count_q.bind(id);
            list_q = list_q.bind(id);
        }
        if let Some(l) = &like {
            count_q = count_q.bind(l.clone()).bind(l.clone());
            list_q = list_q.bind(l.clone()).bind(l.clone());
        }
        if due_bound {
            count_q = count_q.bind(now());
            list_q = list_q.bind(now());
        }
        let total = count_q.fetch_one(&self.db).await?;
        let cards = list_q.bind(limit).bind(offset).fetch_all(&self.db).await?;
        Ok((cards, total))
    }

    // ----- studying ------------------------------------------------------

    pub(crate) async fn sched_config(&self, access: &Access, deck: &Deck) -> Result<SchedConfig> {
        let p = self.preset(access, deck.preset_id).await?;
        Ok(SchedConfig {
            desired_retention: p.desired_retention as f32,
            learning_steps: scheduler::parse_steps(&p.learning_steps).unwrap_or_default(),
            relearning_steps: scheduler::parse_steps(&p.relearning_steps).unwrap_or_default(),
            max_interval: p.max_interval,
            exam_date: deck.exam(),
        })
    }

    /// Pick the next card to study, from one deck or all visible decks.
    pub async fn next_card(&self, access: &Access, deck_id: Option<i64>) -> Result<Option<StudyCard>> {
        let summaries: Vec<DeckSummary> = self
            .deck_summaries(access)
            .await?
            .into_iter()
            .filter(|s| deck_id.is_none_or(|id| s.deck.id == id))
            .collect();
        if let Some(id) = deck_id {
            if summaries.is_empty() {
                self.deck(access, id).await?;
            }
        }
        let remaining: i64 = summaries.iter().map(|s| s.due_total()).sum();
        let n = now();
        let all_ids: Vec<i64> = summaries.iter().map(|s| s.deck.id).collect();
        let review_ids: Vec<i64> =
            summaries.iter().filter(|s| s.review_due > 0).map(|s| s.deck.id).collect();
        let new_ids: Vec<i64> =
            summaries.iter().filter(|s| s.new_today > 0).map(|s| s.deck.id).collect();

        let pick = |ids: &[i64], cond: &str, order: &str| {
            let sql = format!(
                "SELECT * FROM cards WHERE user_id = ? AND suspended = 0 AND deck_id IN ({}) AND {cond}
                 ORDER BY {order} LIMIT 1",
                placeholders(ids.len())
            );
            let ids = ids.to_vec();
            let user_id = access.user_id();
            async move {
                if ids.is_empty() {
                    return Ok::<Option<Card>, AppError>(None);
                }
                let mut q = sqlx::query_as::<_, Card>(&sql).bind(user_id);
                for id in ids {
                    q = q.bind(id);
                }
                Ok(q.bind(n).fetch_optional(&self.db).await?)
            }
        };

        let learning = pick(&all_ids, "state IN (1, 3) AND due <= ?", "due").await?;
        let review = pick(&review_ids, "state = 2 AND due <= ?", "due, id").await?;
        let new = pick(&new_ids, "state = 0 AND ? IS NOT NULL", "position, id").await?;

        // Interleave new cards with reviews so both progress at the same rate.
        let (new_done, new_left, rev_done, rev_left) = summaries.iter().fold((0, 0, 0, 0), |a, s| {
            (
                a.0 + s.new_done_today,
                a.1 + s.new_today,
                a.2 + s.review_done_today,
                a.3 + s.review_due,
            )
        });
        let prefer_new = new_left > 0
            && (rev_left == 0
                || (new_done as f64 / (new_done + new_left) as f64)
                    <= (rev_done as f64 / (rev_done + rev_left).max(1) as f64));

        let chosen = match (learning, review, new) {
            (Some(c), _, _) => Some(c),
            (None, Some(r), Some(nw)) => Some(if prefer_new { nw } else { r }),
            (None, Some(r), None) => Some(r),
            (None, None, Some(nw)) => Some(nw),
            (None, None, None) => {
                let ahead = format!("state IN (1, 3) AND due <= ? + {LEARN_AHEAD_SECS}");
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

    pub async fn previews(&self, access: &Access, card: &Card, deck: &Deck) -> Result<Vec<(Rating, String)>> {
        let cfg = self.sched_config(access, deck).await?;
        let params = access.user.params();
        let fsrs = scheduler::fsrs_for(&params);
        let clock = access.user.clock();
        let sched = card.sched();
        Rating::ALL
            .iter()
            .map(|&r| {
                let out = scheduler::answer(&fsrs, &cfg, card.id, &sched, r, &clock)?;
                Ok((r, out.label()))
            })
            .collect()
    }

    /// Record an answer. `expected_reps` guards against double submissions.
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
        if let Some(reps) = expected_reps {
            if reps != card.reps {
                return Err(AppError::Conflict("this card was already answered".into()));
            }
        }
        let deck = self.deck(access, card.deck_id).await?;
        let cfg = self.sched_config(access, &deck).await?;
        let params = access.user.params();
        let fsrs = scheduler::fsrs_for(&params);
        let clock = access.user.clock();
        let before = card.sched();
        let out = scheduler::answer(&fsrs, &cfg, card.id, &before, rating, &clock)?;
        let c = &out.card;

        let mut tx = self.db.begin().await?;
        let updated = sqlx::query(
            "UPDATE cards SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                last_review = ?, reps = ?, lapses = ?, updated_at = ?
             WHERE id = ? AND reps = ?",
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
        .bind(card.reps)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(AppError::Conflict("this card was already answered".into()));
        }
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
        .bind(c.stability.map(|v| v as f64))
        .bind(c.difficulty.map(|v| v as f64))
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

    /// Undo the most recent answer made through the same channel (web or
    /// this agent). Returns the restored card.
    pub async fn undo_last(&self, access: &Access) -> Result<Option<Card>> {
        access.require_review()?;
        #[derive(sqlx::FromRow)]
        struct Last {
            id: i64,
            card_id: i64,
            prev_card: Option<String>,
        }
        let last: Option<Last> = sqlx::query_as(
            "SELECT id, card_id, prev_card FROM review_log
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
        let newest: i64 = sqlx::query_scalar("SELECT MAX(id) FROM review_log WHERE card_id = ?")
            .bind(card.id)
            .fetch_one(&self.db)
            .await?;
        let Some(prev) = last
            .prev_card
            .as_deref()
            .and_then(|p| serde_json::from_str::<CardSched>(p).ok())
        else {
            return Ok(None);
        };
        if newest != last.id {
            return Ok(None);
        }
        let mut tx = self.db.begin().await?;
        sqlx::query(
            "UPDATE cards SET state = ?, step = ?, stability = ?, difficulty = ?, due = ?,
                last_review = ?, reps = ?, lapses = ?, updated_at = ? WHERE id = ?",
        )
        .bind(prev.state as i64)
        .bind(prev.step)
        .bind(prev.stability.map(|v| v as f64))
        .bind(prev.difficulty.map(|v| v as f64))
        // Make it due right away so it comes up next.
        .bind(if prev.state == CardState::New { 0 } else { prev.due.min(now()) })
        .bind(prev.last_review)
        .bind(prev.reps)
        .bind(prev.lapses)
        .bind(now())
        .bind(card.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM review_log WHERE id = ?")
            .bind(last.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(self.card(access, card.id).await?))
    }

    /// When the next already-learned card falls due (after now), in the
    /// given deck or all visible decks.
    pub async fn next_review_at(&self, access: &Access, deck_id: Option<i64>) -> Result<Option<i64>> {
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => vec![self.deck(access, id).await?.id],
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        if deck_ids.is_empty() {
            return Ok(None);
        }
        let sql = format!(
            "SELECT MIN(due) FROM cards WHERE user_id = ? AND suspended = 0 AND state != 0
             AND due > ? AND deck_id IN ({})",
            placeholders(deck_ids.len())
        );
        let mut q = sqlx::query_scalar::<_, Option<i64>>(&sql).bind(access.user_id()).bind(now());
        for id in &deck_ids {
            q = q.bind(id);
        }
        Ok(q.fetch_one(&self.db).await?)
    }

    pub async fn card_history(&self, access: &Access, card_id: i64) -> Result<Vec<Value>> {
        self.card(access, card_id).await?;
        #[derive(sqlx::FromRow)]
        struct Row {
            rating: i64,
            reviewed_at: i64,
            state_before: i64,
            scheduled_secs: i64,
            stability: Option<f64>,
            source: String,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT rating, reviewed_at, state_before, scheduled_secs, stability, source
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
                    "state_before": CardState::from_i64(r.state_before).as_str(),
                    "next_interval": (r.source != "import" && r.rating > 0)
                        .then(|| crate::format::interval(r.scheduled_secs)),
                    "stability_days": r.stability.map(|s| (s * 100.0).round() / 100.0),
                    "source": r.source,
                })
            })
            .collect())
    }

    // ----- stats ---------------------------------------------------------

    pub async fn stats(&self, access: &Access, deck_id: Option<i64>) -> Result<Stats> {
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => {
                self.deck(access, id).await?;
                vec![id]
            }
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        let clock = access.user.clock();
        let mut stats = Stats {
            total: 0,
            new: 0,
            learning: 0,
            review: 0,
            suspended: 0,
            due_now: 0,
            reviews_today: 0,
            cards_today: 0,
            minutes_today: 0.0,
            true_retention_30d: None,
            retention_sample_30d: 0,
            reviews_30d: 0,
            streak_days: 0,
            forecast: vec![],
            average_retrievability: None,
            seconds_per_review: None,
        };
        if deck_ids.is_empty() {
            return Ok(stats);
        }
        let ph = placeholders(deck_ids.len());

        let sql = format!("SELECT * FROM cards WHERE user_id = ? AND deck_id IN ({ph})");
        let mut q = sqlx::query_as::<_, Card>(&sql).bind(access.user_id());
        for id in &deck_ids {
            q = q.bind(id);
        }
        let cards = q.fetch_all(&self.db).await?;
        let params = access.user.params();
        let horizon = 14;
        let mut forecast = vec![0i64; horizon];
        let mut r_sum = 0.0;
        let mut r_n = 0;
        let today = clock.today();
        for c in &cards {
            stats.total += 1;
            if c.suspended {
                stats.suspended += 1;
                continue;
            }
            match c.card_state() {
                CardState::New => stats.new += 1,
                CardState::Learning | CardState::Relearning => stats.learning += 1,
                CardState::Review => stats.review += 1,
            }
            if c.card_state() != CardState::New {
                if c.due <= clock.now {
                    stats.due_now += 1;
                }
                let day = (clock.date_of(c.due) - today).num_days().max(0) as usize;
                if day < horizon {
                    forecast[day] += 1;
                }
                if let Some(r) = scheduler::retrievability(&c.sched(), &params, &clock) {
                    r_sum += r as f64;
                    r_n += 1;
                }
            }
        }
        stats.forecast = forecast
            .into_iter()
            .enumerate()
            .map(|(i, n)| ((today + Duration::days(i as i64)).format("%Y-%m-%d").to_string(), n))
            .collect();
        if r_n > 0 {
            stats.average_retrievability = Some(r_sum / r_n as f64);
        }

        #[derive(sqlx::FromRow)]
        struct Log {
            card_id: i64,
            rating: i64,
            reviewed_at: i64,
            state_before: i64,
            duration_ms: Option<i64>,
        }
        let sql = format!(
            "SELECT card_id, rating, reviewed_at, state_before, duration_ms FROM review_log
             WHERE user_id = ? AND rating > 0 AND deck_id IN ({ph}) AND reviewed_at >= ?"
        );
        let mut q = sqlx::query_as::<_, Log>(&sql).bind(access.user_id());
        for id in &deck_ids {
            q = q.bind(id);
        }
        let logs = q
            .bind(clock.day_start(today - Duration::days(365)))
            .fetch_all(&self.db)
            .await?;
        let start_today = clock.start_of_today();
        let start_30 = clock.day_start(today - Duration::days(30));
        let mut passed = 0;
        let (mut timed_ms, mut timed) = (0i64, 0i64);
        let mut review_total = 0;
        let mut days = std::collections::HashSet::new();
        let mut cards_today = std::collections::HashSet::new();
        for l in &logs {
            days.insert(clock.date_of(l.reviewed_at));
            if l.reviewed_at >= start_today {
                stats.reviews_today += 1;
                cards_today.insert(l.card_id);
                stats.minutes_today += l.duration_ms.unwrap_or(0) as f64 / 60_000.0;
            }
            if l.reviewed_at >= start_30 {
                stats.reviews_30d += 1;
                if let Some(ms) = l.duration_ms.filter(|ms| *ms > 0) {
                    timed_ms += ms;
                    timed += 1;
                }
                if l.state_before == CardState::Review as i64 {
                    review_total += 1;
                    if l.rating > 1 {
                        passed += 1;
                    }
                }
            }
        }
        if review_total > 0 {
            stats.true_retention_30d = Some(passed as f64 / review_total as f64);
        }
        stats.retention_sample_30d = review_total;
        stats.cards_today = cards_today.len() as i64;
        let mut day = if days.contains(&today) { today } else { today - Duration::days(1) };
        while days.contains(&day) {
            stats.streak_days += 1;
            day -= Duration::days(1);
        }
        stats.minutes_today = (stats.minutes_today * 10.0).round() / 10.0;
        if timed >= 10 {
            stats.seconds_per_review = Some(timed_ms as f64 / timed as f64 / 1000.0);
        }
        Ok(stats)
    }

    // ----- settings & FSRS ----------------------------------------------

    pub async fn update_settings(
        &self,
        user_id: i64,
        timezone: &str,
        rollover_hour: i64,
        four_buttons: bool,
        font_scale: i64,
        optimize_include_agent: bool,
        desktop_look: &str,
    ) -> Result<()> {
        if !matches!(desktop_look, "calm" | "simple") {
            return Err(AppError::bad("unknown look"));
        }
        timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| AppError::bad(format!("unknown timezone '{timezone}'")))?;
        sqlx::query(
            "UPDATE users SET timezone = ?, rollover_hour = ?, four_buttons = ?, font_scale = ?,
                optimize_include_agent = ?, desktop_look = ? WHERE id = ?",
        )
        .bind(timezone)
        .bind(rollover_hour.clamp(0, 23))
        .bind(four_buttons)
        .bind(font_scale.clamp(80, 200))
        .bind(optimize_include_agent)
        .bind(desktop_look)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn reset_parameters(&self, access: &Access) -> Result<()> {
        access.require_write()?;
        access.require_all_decks()?;
        sqlx::query("UPDATE users SET fsrs_params = '[]', fsrs_params_updated_at = NULL WHERE id = ?")
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Count of reviews the optimizer would learn from.
    pub async fn optimizable_reviews(&self, access: &Access) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM review_log WHERE user_id = ? AND rating > 0
             AND (source != 'agent' OR ?)",
        )
        .bind(access.user_id())
        .bind(access.user.optimize_include_agent)
        .fetch_one(&self.db)
        .await?)
    }

    /// Fit FSRS parameters to the user's own review history.
    pub async fn optimize_parameters(&self, access: &Access) -> Result<Value> {
        access.require_write()?;
        access.require_all_decks()?;
        #[derive(sqlx::FromRow)]
        struct Row {
            card_id: i64,
            rating: i64,
            reviewed_at: i64,
            state_before: i64,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT card_id, rating, reviewed_at, state_before FROM review_log
             WHERE user_id = ? AND (source != 'agent' OR ?)
             ORDER BY card_id, reviewed_at, id",
        )
        .bind(access.user_id())
        .bind(access.user.optimize_include_agent)
        .fetch_all(&self.db)
        .await?;
        let clock = access.user.clock();

        let mut by_card: Vec<(i64, Vec<&Row>)> = Vec::new();
        for r in &rows {
            match by_card.last_mut() {
                Some((id, v)) if *id == r.card_id => v.push(r),
                _ => by_card.push((r.card_id, vec![r])),
            }
        }
        let mut items = Vec::new();
        let mut card_ids = Vec::new();
        let mut reviews_used = 0;
        for (card_id, logs) in by_card {
            // Only use history after the last reset, and only if it starts
            // from the card being new (otherwise the memory state is unknown).
            let start = logs.iter().rposition(|l| l.rating == 0).map(|i| i + 1).unwrap_or(0);
            let logs = &logs[start..];
            if logs.first().is_none_or(|l| l.state_before != CardState::New as i64) {
                continue;
            }
            let mut reviews = Vec::new();
            let mut prev: Option<i64> = None;
            for l in logs {
                let delta = prev.map(|p| clock.days_between(p, l.reviewed_at).max(0)).unwrap_or(0);
                reviews.push(fsrs::FSRSReview {
                    rating: l.rating.clamp(1, 4) as u32,
                    delta_t: delta as u32,
                });
                prev = Some(l.reviewed_at);
            }
            reviews_used += reviews.len();
            for i in 1..reviews.len() {
                if reviews[i].delta_t > 0 {
                    items.push(fsrs::FSRSItem {
                        reviews: reviews[..=i].to_vec(),
                    });
                    card_ids.push(card_id);
                }
            }
        }
        if items.is_empty() {
            return Err(AppError::bad(
                "not enough review history yet (cards need reviews on at least two different days)",
            ));
        }
        let n_items = items.len();
        let params = tokio::task::spawn_blocking(move || {
            fsrs::compute_parameters(fsrs::ComputeParametersInput {
                train_set: items,
                card_ids: Some(card_ids),
                ..Default::default()
            })
        })
        .await
        .map_err(|e| AppError::Internal(e.into()))?
        .map_err(|e| match e {
            fsrs::FSRSError::NotEnoughData => {
                AppError::bad("not enough review history yet to optimize; keep studying")
            }
            e => e.into(),
        })?;
        sqlx::query("UPDATE users SET fsrs_params = ?, fsrs_params_updated_at = ? WHERE id = ?")
            .bind(serde_json::to_string(&params).unwrap_or_else(|_| "[]".into()))
            .bind(now())
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(json!({
            "parameters": params,
            "reviews_used": reviews_used,
            "training_items": n_items,
        }))
    }
}
