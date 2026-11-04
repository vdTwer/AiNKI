//! Fitting FSRS to the user and showing what settings cost: per-preset
//! parameters, the optimizer (which only keeps parameters that fit better),
//! monthly re-optimization, the workload simulator and the calibration view.
//!
//! Following the FSRS project's and Anki's guidance (2025): re-optimize about
//! once a month; give material that behaves differently its own preset and
//! parameters once it has a few hundred reviews; and show the workload of a
//! desired retention by simulation rather than suggest one "optimal" value
//! (Anki 25.07 removed its "compute minimum recommended retention").

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    App,
    auth::{Access, User},
    error::{AppError, Result},
    now,
    scheduler::{self, CardState},
    service::placeholders,
};

/// A preset gets parameters of its own from this many reviews on.
pub const PRESET_MIN_REVIEWS: usize = 400;
/// Automatic optimization: at most this often...
const AUTO_EVERY_SECS: i64 = 30 * 86_400;
/// ...once there are this many reviews in all...
const AUTO_MIN_REVIEWS: i64 = 400;
/// ...and this many new ones since the last run.
const AUTO_MIN_NEW_REVIEWS: i64 = 100;

/// Which FSRS parameters apply to which deck: its preset's own, or the
/// user's (which may be the defaults, an empty list).
pub struct ParamBook {
    user: Vec<f32>,
    by_deck: HashMap<i64, Vec<f32>>,
}

impl ParamBook {
    pub fn for_deck(&self, deck_id: i64) -> &[f32] {
        self.by_deck.get(&deck_id).map(Vec::as_slice).unwrap_or(&self.user)
    }
}

fn parse_params(s: &str) -> Vec<f32> {
    serde_json::from_str(s).unwrap_or_default()
}

/// One review in the optimizer's input, keyed by card (negative: an item
/// of a multicard).
#[derive(sqlx::FromRow)]
struct Row {
    card_id: i64,
    deck_id: i64,
    rating: i64,
    reviewed_at: i64,
    state_before: i64,
}

/// What the optimizer learns from, per card: the reviews since the card
/// was last reset, when they start from the card being new.
struct History {
    card_id: i64,
    deck_id: i64,
    reviews: Vec<fsrs::FSRSReview>,
}

fn training_set(histories: &[&History]) -> (Vec<fsrs::FSRSItem>, Vec<i64>, usize) {
    let mut items = Vec::new();
    let mut card_ids = Vec::new();
    let mut used = 0;
    for h in histories {
        used += h.reviews.len();
        for i in 1..h.reviews.len() {
            if h.reviews[i].delta_t > 0 {
                items.push(fsrs::FSRSItem { reviews: h.reviews[..=i].to_vec() });
                card_ids.push(h.card_id);
            }
        }
    }
    (items, card_ids, used)
}

/// The result of fitting one set of parameters.
#[derive(Debug, Clone, Serialize)]
pub struct Fit {
    /// The parameters now in use for this scope.
    pub parameters: Vec<f32>,
    /// New parameters were found and kept (they predict better).
    pub changed: bool,
    /// How much better the new ones predict, as a share of the old error
    /// (log loss); `None` when nothing was fitted.
    pub improvement: Option<f64>,
    pub reviews: usize,
}

/// Fit parameters and keep them only if they predict this history better
/// than `current` (empty: the defaults). Runs on a blocking thread.
fn fit(items: Vec<fsrs::FSRSItem>, card_ids: Vec<i64>, current: Vec<f32>, reviews: usize) -> std::result::Result<Fit, fsrs::FSRSError> {
    let new = fsrs::compute_parameters(fsrs::ComputeParametersInput {
        train_set: items.clone(),
        card_ids: Some(card_ids),
        ..Default::default()
    })?;
    let loss = |p: &[f32]| -> Option<f32> {
        let model = fsrs::FSRS::new(p).ok()?;
        model.evaluate(items.clone(), |_| true).ok().map(|e| e.log_loss)
    };
    let (old_loss, new_loss) = (loss(&current), loss(&new));
    Ok(match (old_loss, new_loss) {
        (Some(o), Some(n)) if n < o => Fit {
            parameters: new,
            changed: true,
            improvement: Some(((o - n) / o) as f64),
            reviews,
        },
        // A fit that could not be scored at all is kept, as before.
        (None, Some(_)) => Fit { parameters: new, changed: true, improvement: None, reviews },
        _ => Fit { parameters: current, changed: false, improvement: Some(0.0), reviews },
    })
}

/// A desired retention and what it would cost.
#[derive(Debug, Clone, Serialize)]
pub struct Workload {
    pub retention: f64,
    /// Average study time per day over the period.
    pub minutes_per_day: f64,
    /// Average reviews per day (new cards not counted).
    pub reviews_per_day: f64,
    /// Cards you would remember at the end, if asked about all of them.
    pub remembered: f64,
    /// This is the preset's own setting.
    pub current: bool,
}

/// One band of predicted recall: what FSRS said and what happened.
#[derive(Debug, Clone, Serialize)]
pub struct CalibrationBin {
    /// The band, as whole percents ("85" to "90").
    pub from: u32,
    pub to: u32,
    pub predicted: f64,
    pub actual: f64,
    pub reviews: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Calibration {
    pub bins: Vec<CalibrationBin>,
    pub predicted: f64,
    pub actual: f64,
    pub reviews: i64,
}

const BANDS: [(u32, u32); 7] = [(0, 70), (70, 75), (75, 80), (80, 85), (85, 90), (90, 95), (95, 100)];

impl App {
    /// The parameters for every visible deck, read once.
    pub async fn param_book(&self, access: &Access) -> Result<ParamBook> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT d.id, p.fsrs_params FROM decks d JOIN presets p ON p.id = d.preset_id WHERE d.user_id = ?",
        )
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        Ok(ParamBook {
            user: access.user.params(),
            by_deck: rows
                .into_iter()
                .map(|(deck, p)| (deck, parse_params(&p)))
                .filter(|(_, p)| !p.is_empty())
                .collect(),
        })
    }

    /// The parameters that schedule one deck.
    pub async fn deck_params(&self, access: &Access, deck_id: i64) -> Result<Vec<f32>> {
        let own: Option<String> = sqlx::query_scalar(
            "SELECT p.fsrs_params FROM decks d JOIN presets p ON p.id = d.preset_id WHERE d.id = ? AND d.user_id = ?",
        )
        .bind(deck_id)
        .bind(access.user_id())
        .fetch_optional(&self.db)
        .await?;
        Ok(own.map(|p| parse_params(&p)).filter(|p| !p.is_empty()).unwrap_or_else(|| access.user.params()))
    }

    async fn histories(&self, user: &User) -> Result<Vec<History>> {
        // A multicard's own rows only summarize its items, so the optimizer
        // learns from the items instead: each item is a card of its own
        // (keyed by its negative id), reset along with its card.
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT r.card_id, c.deck_id, r.rating, r.reviewed_at, r.state_before
             FROM review_log r JOIN cards c ON c.id = r.card_id
             WHERE r.user_id = ?1 AND (r.source != 'agent' OR ?2) AND r.items_asked IS NULL
             UNION ALL
             SELECT -i.item_id, c.deck_id, i.rating, i.reviewed_at, i.state_before
             FROM item_review_log i JOIN review_log r ON r.id = i.review_log_id JOIN cards c ON c.id = r.card_id
             WHERE r.user_id = ?1 AND (r.source != 'agent' OR ?2)
               AND i.reviewed_at > COALESCE((SELECT MAX(x.reviewed_at) FROM review_log x
                   WHERE x.card_id = r.card_id AND x.rating = 0), -1)
             ORDER BY 1, 4",
        )
        .bind(user.id)
        .bind(user.optimize_include_agent)
        .fetch_all(&self.db)
        .await?;
        let clock = user.clock();
        let mut by_card: Vec<(i64, i64, Vec<&Row>)> = Vec::new();
        for r in &rows {
            match by_card.last_mut() {
                Some((id, _, v)) if *id == r.card_id => v.push(r),
                _ => by_card.push((r.card_id, r.deck_id, vec![r])),
            }
        }
        let mut out = Vec::new();
        for (card_id, deck_id, logs) in by_card {
            // Only history after the last reset, and only if it starts from
            // the card being new (otherwise the memory state is unknown).
            let start = logs.iter().rposition(|l| l.rating == 0).map(|i| i + 1).unwrap_or(0);
            let logs = &logs[start..];
            if logs.first().is_none_or(|l| l.state_before != CardState::New as i64) {
                continue;
            }
            let mut reviews = Vec::new();
            let mut prev: Option<i64> = None;
            for l in logs {
                let delta = prev.map(|p| clock.days_between(p, l.reviewed_at).max(0)).unwrap_or(0);
                reviews.push(fsrs::FSRSReview { rating: l.rating.clamp(1, 4) as u32, delta_t: delta as u32 });
                prev = Some(l.reviewed_at);
            }
            out.push(History { card_id, deck_id, reviews });
        }
        Ok(out)
    }

    /// Fit FSRS to the user's history: once for everything, and once more
    /// for each preset with enough reviews of its own. New parameters are
    /// only kept where they predict the history better than the ones in use.
    pub async fn optimize_parameters(&self, access: &Access) -> Result<Value> {
        access.require_write()?;
        access.require_all_decks()?;
        self.optimize_user(&access.user).await
    }

    async fn optimize_user(&self, user: &User) -> Result<Value> {
        let histories = self.histories(user).await?;
        let all: Vec<&History> = histories.iter().collect();
        let (items, card_ids, used) = training_set(&all);
        if items.is_empty() {
            return Err(AppError::bad(
                "not enough review history yet (cards need reviews on at least two different days)",
            ));
        }
        let n_items = items.len();
        let current = user.params();
        let global = tokio::task::spawn_blocking(move || fit(items, card_ids, current, used))
            .await
            .map_err(|e| AppError::Internal(e.into()))?
            .map_err(|e| match e {
                fsrs::FSRSError::NotEnoughData => AppError::bad("not enough review history yet to optimize; keep studying"),
                e => e.into(),
            })?;
        let t = now();
        if global.changed {
            sqlx::query("UPDATE users SET fsrs_params = ?, fsrs_params_updated_at = ? WHERE id = ?")
                .bind(serde_json::to_string(&global.parameters).unwrap_or_else(|_| "[]".into()))
                .bind(t)
                .bind(user.id)
                .execute(&self.db)
                .await?;
        }

        // Presets with enough reviews get their own; the others use the
        // user's.
        #[derive(sqlx::FromRow)]
        struct P {
            id: i64,
            name: String,
            fsrs_params: String,
        }
        let presets: Vec<P> = sqlx::query_as("SELECT id, name, fsrs_params FROM presets WHERE user_id = ?")
            .bind(user.id)
            .fetch_all(&self.db)
            .await?;
        let deck_preset: HashMap<i64, i64> = sqlx::query_as::<_, (i64, i64)>("SELECT id, preset_id FROM decks WHERE user_id = ?")
            .bind(user.id)
            .fetch_all(&self.db)
            .await?
            .into_iter()
            .collect();
        let mut preset_results = Vec::new();
        for p in presets {
            let mine: Vec<&History> = histories
                .iter()
                .filter(|h| deck_preset.get(&h.deck_id) == Some(&p.id))
                .collect();
            let reviews: usize = mine.iter().map(|h| h.reviews.len()).sum();
            let own = parse_params(&p.fsrs_params);
            if reviews < PRESET_MIN_REVIEWS {
                if !own.is_empty() {
                    sqlx::query("UPDATE presets SET fsrs_params = '[]', fsrs_params_updated_at = NULL WHERE id = ?")
                        .bind(p.id)
                        .execute(&self.db)
                        .await?;
                }
                continue;
            }
            let (items, card_ids, used) = training_set(&mine);
            if items.is_empty() {
                continue;
            }
            // A preset starts from the user's parameters: its own must beat those.
            let current = if own.is_empty() { global.parameters.clone() } else { own };
            let result = tokio::task::spawn_blocking(move || fit(items, card_ids, current, used))
                .await
                .map_err(|e| AppError::Internal(e.into()))?;
            let Ok(f) = result else { continue };
            if f.changed {
                sqlx::query("UPDATE presets SET fsrs_params = ?, fsrs_params_updated_at = ? WHERE id = ?")
                    .bind(serde_json::to_string(&f.parameters).unwrap_or_else(|_| "[]".into()))
                    .bind(t)
                    .bind(p.id)
                    .execute(&self.db)
                    .await?;
            }
            preset_results.push(json!({ "preset": p.name, "reviews": f.reviews, "changed": f.changed, "improvement": f.improvement }));
        }

        let note = match (global.changed, global.improvement) {
            (true, Some(i)) => format!("Fitted to {} reviews; predicts {:.0}% better than before.", used, i * 100.0),
            (true, None) => format!("Fitted to {used} reviews."),
            (false, _) => "Kept the parameters in use: new ones would not predict your reviews better.".to_string(),
        };
        let reviews_now = self.optimizable_reviews_of(user).await?;
        sqlx::query("UPDATE users SET optimized_at = ?, optimized_reviews = ?, optimize_note = ? WHERE id = ?")
            .bind(t)
            .bind(reviews_now)
            .bind(&note)
            .bind(user.id)
            .execute(&self.db)
            .await?;
        Ok(json!({
            "parameters": global.parameters,
            "changed": global.changed,
            "improvement": global.improvement,
            "note": note,
            "reviews_used": used,
            "training_items": n_items,
            "presets": preset_results,
        }))
    }

    /// Count of reviews the optimizer would learn from.
    pub async fn optimizable_reviews(&self, access: &Access) -> Result<i64> {
        self.optimizable_reviews_of(&access.user).await
    }

    async fn optimizable_reviews_of(&self, user: &User) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM review_log WHERE user_id = ?1 AND rating > 0
                    AND items_asked IS NULL AND (source != 'agent' OR ?2))
                  + (SELECT COUNT(*) FROM item_review_log i JOIN review_log r ON r.id = i.review_log_id
                    WHERE r.user_id = ?1 AND (r.source != 'agent' OR ?2))",
        )
        .bind(user.id)
        .bind(user.optimize_include_agent)
        .fetch_one(&self.db)
        .await?)
    }

    /// Re-optimize everyone who wants it: about once a month, when there
    /// are enough new reviews. Called from the hourly cleanup.
    pub async fn auto_optimize(&self) -> anyhow::Result<()> {
        let users: Vec<User> = sqlx::query_as(
            "SELECT * FROM users WHERE auto_optimize = 1 AND (optimized_at IS NULL OR optimized_at < ?)",
        )
        .bind(now() - AUTO_EVERY_SECS)
        .fetch_all(&self.db)
        .await?;
        for user in users {
            let reviews = self.optimizable_reviews_of(&user).await?;
            if reviews < AUTO_MIN_REVIEWS || reviews - user.optimized_reviews < AUTO_MIN_NEW_REVIEWS {
                continue;
            }
            if let Err(e) = self.optimize_user(&user).await {
                tracing::info!("automatic optimization skipped for user {}: {e:?}", user.id);
            }
        }
        Ok(())
    }

    /// Simulate a preset's decks for `days` at several desired retentions:
    /// study time, reviews, and how much you would remember at the end. It
    /// assumes you keep learning the preset's new cards every day.
    pub async fn workload(&self, access: &Access, preset_id: i64, days: usize) -> Result<Vec<Workload>> {
        let preset = self.preset(access, preset_id).await?;
        let deck_ids: Vec<i64> = self
            .decks(access)
            .await?
            .into_iter()
            .filter(|d| d.preset_id == preset_id)
            .map(|d| d.id)
            .collect();
        let own = parse_params(&preset_params(self, preset_id).await?);
        let params = if own.is_empty() { access.user.params() } else { own };
        let clock = access.user.clock();

        // Cards already learned, as the simulator sees them (days from today).
        let mut existing = Vec::new();
        let mut new_cards = 0usize;
        if !deck_ids.is_empty() {
            let sql = format!(
                "SELECT state, stability, difficulty, due, last_review, lapses FROM cards
                 WHERE user_id = ? AND suspended = 0 AND deck_id IN ({})",
                placeholders(deck_ids.len())
            );
            let mut q = sqlx::query_as::<_, (i64, Option<f64>, Option<f64>, i64, Option<i64>, i64)>(&sql).bind(access.user_id());
            for id in &deck_ids {
                q = q.bind(id);
            }
            for (state, s, d, due, last, lapses) in q.fetch_all(&self.db).await? {
                match (CardState::from_i64(state), s, d, last) {
                    (CardState::New, ..) => new_cards += 1,
                    (_, Some(s), Some(d), Some(last)) => {
                        let since = (clock.now - last) as f32 / 86_400.0;
                        let until = ((due - clock.now) as f32 / 86_400.0).max(0.0);
                        existing.push((s as f32, d as f32, since, until, lapses as u32));
                    }
                    _ => {}
                }
            }
        }
        let to_learn = new_cards.max(preset.new_per_day.max(0) as usize * days);
        // Thinking time: the simulator's defaults, scaled to the user's own
        // typical time per review once there is enough history.
        let scale = self
            .stats(access, None)
            .await?
            .seconds_per_review
            .map(|s| (s / 12.32).clamp(0.3, 4.0) as f32)
            .unwrap_or(1.0);
        let current = preset.desired_retention;
        let mut retentions = vec![0.80, 0.85, 0.90, 0.95];
        if !retentions.iter().any(|r: &f64| (r - current).abs() < 0.005) {
            retentions.push(current);
            retentions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }
        let learning_steps = scheduler::parse_steps(&preset.learning_steps).unwrap_or_default().len();
        let relearning_steps = scheduler::parse_steps(&preset.relearning_steps).unwrap_or_default().len();
        let (new_per_day, reviews_per_day, max_ivl) = (preset.new_per_day, preset.reviews_per_day, preset.max_interval);
        let out = tokio::task::spawn_blocking(move || -> std::result::Result<Vec<Workload>, fsrs::FSRSError> {
            let filled = Arc::new(fsrs::check_and_fill_parameters(&params)?);
            let mut out = Vec::new();
            for r in retentions {
                let mut config = fsrs::SimulatorConfig {
                    deck_size: existing.len() + to_learn,
                    learn_span: days,
                    max_cost_perday: f32::MAX,
                    max_ivl: max_ivl as f32,
                    learn_limit: new_per_day.max(0) as usize,
                    review_limit: reviews_per_day.max(0) as usize,
                    learning_step_count: learning_steps,
                    relearning_step_count: relearning_steps,
                    ..Default::default()
                };
                for row in config.state_rating_costs.iter_mut() {
                    for c in row.iter_mut() {
                        *c *= scale;
                    }
                }
                if config.deck_size == 0 {
                    out.push(Workload { retention: r, minutes_per_day: 0.0, reviews_per_day: 0.0, remembered: 0.0, current: (r - current).abs() < 0.005 });
                    continue;
                }
                let cards: Vec<fsrs::Card> = existing
                    .iter()
                    .enumerate()
                    .map(|(i, &(s, d, since, until, lapses))| fsrs::Card {
                        id: i as i64 + 1,
                        difficulty: d,
                        stability: s,
                        last_date: -since,
                        due: until,
                        interval: since + until,
                        lapses,
                        desired_retention: r as f32,
                        parameters: filled.clone(),
                    })
                    .collect();
                let sim = fsrs::simulate(&config, &filled, r as f32, Some(42), Some(cards))?;
                let n = days.max(1) as f64;
                out.push(Workload {
                    retention: r,
                    minutes_per_day: sim.cost_per_day.iter().map(|c| *c as f64).sum::<f64>() / 60.0 / n,
                    reviews_per_day: sim.review_cnt_per_day.iter().sum::<usize>() as f64 / n,
                    remembered: sim.memorized_cnt_per_day.last().copied().unwrap_or(0.0) as f64,
                    current: (r - current).abs() < 0.005,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| AppError::Internal(e.into()))??;
        Ok(out)
    }

    /// How well FSRS's predictions match what happened, over the last year:
    /// reviews of learned cards, grouped by the recall chance FSRS gave
    /// them, against the share actually remembered.
    pub async fn calibration(&self, access: &Access, deck_id: Option<i64>) -> Result<Calibration> {
        let book = self.param_book(access).await?;
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => vec![self.deck(access, id).await?.id],
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        let mut out = Calibration { bins: vec![], predicted: 0.0, actual: 0.0, reviews: 0 };
        if deck_ids.is_empty() {
            return Ok(out);
        }
        let sql = format!(
            "SELECT deck_id, rating, reviewed_at,
                json_extract(prev_card, '$.stability'), json_extract(prev_card, '$.last_review')
             FROM review_log
             WHERE user_id = ? AND reviewed_at >= ? AND rating > 0 AND state_before = 2
               AND items_asked IS NULL AND prev_card IS NOT NULL AND source != 'import'
               AND deck_id IN ({})",
            placeholders(deck_ids.len())
        );
        let mut q = sqlx::query_as::<_, (i64, i64, i64, Option<f64>, Option<i64>)>(&sql)
            .bind(access.user_id())
            .bind(now() - 365 * 86_400);
        for id in &deck_ids {
            q = q.bind(id);
        }
        let mut sums = [(0.0f64, 0i64, 0i64); BANDS.len()];
        for (deck, rating, at, s, last) in q.fetch_all(&self.db).await? {
            let (Some(s), Some(last)) = (s, last) else { continue };
            let elapsed = ((at - last).max(0) as f32) / 86_400.0;
            let memory = fsrs::MemoryState { stability: s as f32, difficulty: 5.0 };
            let r = fsrs::current_retrievability(memory, elapsed, scheduler::decay_for(book.for_deck(deck))) as f64;
            let pct = (r * 100.0).clamp(0.0, 99.999) as u32;
            let band = BANDS.iter().position(|(from, to)| pct >= *from && pct < *to).unwrap_or(0);
            sums[band].0 += r;
            sums[band].1 += (rating > 1) as i64;
            sums[band].2 += 1;
        }
        let total: i64 = sums.iter().map(|s| s.2).sum();
        if total == 0 {
            return Ok(out);
        }
        out.reviews = total;
        out.predicted = sums.iter().map(|s| s.0).sum::<f64>() / total as f64;
        out.actual = sums.iter().map(|s| s.1).sum::<i64>() as f64 / total as f64;
        out.bins = BANDS
            .iter()
            .zip(sums)
            .filter(|(_, s)| s.2 > 0)
            .map(|(&(from, to), (p, ok, n))| CalibrationBin {
                from,
                to,
                predicted: p / n as f64,
                actual: ok as f64 / n as f64,
                reviews: n,
            })
            .collect();
        Ok(out)
    }

    pub async fn set_auto_optimize(&self, user_id: i64, on: bool) -> Result<()> {
        sqlx::query("UPDATE users SET auto_optimize = ? WHERE id = ?")
            .bind(on)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

async fn preset_params(app: &App, preset_id: i64) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT fsrs_params FROM presets WHERE id = ?")
        .bind(preset_id)
        .fetch_one(&app.db)
        .await?)
}
