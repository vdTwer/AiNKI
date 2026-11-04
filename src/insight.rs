//! Looking back: the week in a few numbers with one tip, and how much of
//! each deck you really remembered against what its preset aims for.

use std::collections::{HashMap, HashSet};

use chrono::Duration;
use serde::Serialize;

use crate::App;
use crate::auth::Access;
use crate::error::Result;

/// Below this many reviews a deck's retention says little.
pub const MIN_SAMPLE: i64 = 20;
/// Retention this far under the target is worth a word.
const SHORT_BY: f64 = 0.05;

#[derive(Debug, Serialize)]
pub struct DeckRetention {
    pub deck_id: i64,
    pub deck_name: String,
    pub target: f64,
    /// Share of reviews of learned cards that were remembered.
    pub actual: Option<f64>,
    pub reviews: i64,
}

impl DeckRetention {
    pub fn short(&self) -> bool {
        self.reviews >= MIN_SAMPLE && self.actual.is_some_and(|a| a < self.target - SHORT_BY)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tip {
    /// Some cards keep slipping; fixing them beats reviewing them.
    Attention,
    /// Remembered clearly less than aimed for.
    BelowTarget,
    /// Remembered clearly more; optimizing lets FSRS space cards further.
    AboveTarget,
    /// Studied on few days; little and often works better.
    Spread,
    Nothing,
}

#[derive(Debug, Serialize)]
pub struct Week {
    pub reviews: i64,
    pub minutes: f64,
    pub days_studied: i64,
    pub new_cards: i64,
    pub retention: Option<f64>,
    /// Desired retention over the decks reviewed, weighted by reviews.
    pub target: Option<f64>,
    pub retention_sample: i64,
    pub attention: i64,
    pub tip: Tip,
}

impl App {
    /// How much of each deck was remembered when due over the last 30 days.
    pub async fn deck_retention(&self, access: &Access) -> Result<Vec<DeckRetention>> {
        let decks = self.decks(access).await?;
        let presets: HashMap<i64, f64> =
            self.list_presets(access).await?.into_iter().map(|p| (p.id, p.desired_retention)).collect();
        let since = access.user.clock().start_of_today() - 30 * 86_400;
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT deck_id, COUNT(*), COALESCE(SUM(rating > 1), 0) FROM review_log
             WHERE user_id = ? AND rating > 0 AND state_before = 2 AND source != 'import' AND reviewed_at >= ?
             GROUP BY deck_id",
        )
        .bind(access.user_id())
        .bind(since)
        .fetch_all(&self.db)
        .await?;
        let by_deck: HashMap<i64, (i64, i64)> = rows.into_iter().map(|(d, n, ok)| (d, (n, ok))).collect();
        Ok(decks
            .into_iter()
            .map(|d| {
                let (n, ok) = by_deck.get(&d.id).copied().unwrap_or((0, 0));
                DeckRetention {
                    deck_id: d.id,
                    deck_name: d.name,
                    target: presets.get(&d.preset_id).copied().unwrap_or(0.9),
                    actual: (n > 0).then(|| ok as f64 / n as f64),
                    reviews: n,
                }
            })
            .collect())
    }

    /// The last seven days, today included.
    pub async fn week(&self, access: &Access) -> Result<Week> {
        let clock = access.user.clock();
        let today = clock.today();
        let start = clock.day_start(today - Duration::days(6));
        let presets: HashMap<i64, f64> =
            self.list_presets(access).await?.into_iter().map(|p| (p.id, p.desired_retention)).collect();
        // The decks this access sees, with the retention each aims for.
        let targets: HashMap<i64, f64> = self
            .decks(access)
            .await?
            .into_iter()
            .map(|d| (d.id, presets.get(&d.preset_id).copied().unwrap_or(0.9)))
            .collect();

        #[derive(sqlx::FromRow)]
        struct Row {
            deck_id: i64,
            card_id: i64,
            rating: i64,
            state_before: i64,
            reviewed_at: i64,
            duration_ms: Option<i64>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT deck_id, card_id, rating, state_before, reviewed_at, duration_ms FROM review_log
             WHERE user_id = ? AND rating > 0 AND source != 'import' AND reviewed_at >= ?",
        )
        .bind(access.user_id())
        .bind(start)
        .fetch_all(&self.db)
        .await?;

        let (mut reviews, mut ms, mut sample, mut passed, mut target_sum) = (0, 0, 0, 0, 0.0);
        let mut days = HashSet::new();
        let mut new_cards = HashSet::new();
        for r in rows.iter().filter(|r| targets.contains_key(&r.deck_id)) {
            reviews += 1;
            ms += r.duration_ms.unwrap_or(0);
            days.insert((r.reviewed_at - start).div_euclid(86_400));
            if r.state_before == 0 {
                new_cards.insert(r.card_id);
            }
            if r.state_before == 2 {
                sample += 1;
                passed += (r.rating > 1) as i64;
                target_sum += targets.get(&r.deck_id).copied().unwrap_or(0.9);
            }
        }
        let attention = self.attention(access, None).await?.len() as i64;
        let retention = (sample > 0).then(|| passed as f64 / sample as f64);
        let target = (sample > 0).then(|| target_sum / sample as f64);
        let tip = if attention > 0 {
            Tip::Attention
        } else if let (true, Some(r), Some(t)) = (sample >= MIN_SAMPLE, retention, target) {
            if r < t - SHORT_BY {
                Tip::BelowTarget
            } else if r > t + 0.04 && sample >= 100 {
                Tip::AboveTarget
            } else if reviews > 0 && days.len() < 4 {
                Tip::Spread
            } else {
                Tip::Nothing
            }
        } else if reviews > 0 && days.len() < 4 {
            Tip::Spread
        } else {
            Tip::Nothing
        };
        Ok(Week {
            reviews,
            minutes: ms as f64 / 60_000.0,
            days_studied: days.len() as i64,
            new_cards: new_cards.len() as i64,
            retention,
            target,
            retention_sample: sample,
            attention,
            tip,
        })
    }
}
