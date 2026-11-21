//! Stats: counts, forecast, recall and streaks.

#[allow(clippy::wildcard_imports, reason = "an area of the service sees the service's shared imports and types")]
use super::*;

#[derive(Debug, Default, Serialize)]
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

impl Stats {
    /// Thinking time when there's too little history to measure it.
    const GUESS_SECONDS_PER_REVIEW: f64 = 12.0;

    /// Whole minutes `cards` answers take, rounded up: measured thinking
    /// time once there's enough history, a rough guess before.
    pub fn minutes_for(&self, cards: i64) -> i64 {
        let seconds = self.seconds_per_review.unwrap_or(Self::GUESS_SECONDS_PER_REVIEW);
        num::f64_i64((num::i64_f64(cards) * seconds / 60.0).ceil())
    }
}

/// Days of history the desktop stats charts show: twelve whole weeks.
pub const HISTORY_DAYS: i64 = 84;

#[derive(Debug, Serialize)]
pub struct History {
    /// Reviews on each day, oldest first, by date ("2026-10-21").
    pub days: Vec<(String, i64)>,
    /// Per week, oldest first: review-state answers, and how many were
    /// recalled.
    pub weeks: Vec<(i64, i64)>,
}

impl App {
    /// # Errors
    ///
    /// - `NotFound` if a deck is given that the access doesn't see.
    /// - `Internal` if the database fails. Also `Internal` if stored FSRS parameters are broken.
    pub async fn stats(&self, access: &Access, deck_id: Option<i64>) -> Result<Stats> {
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => {
                self.deck(access, id).await?;
                vec![id]
            }
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        let mut stats = Stats::default();
        if deck_ids.is_empty() {
            return Ok(stats);
        }
        let clock = access.user.clock();
        self.count_cards(access, &deck_ids, &clock, &mut stats).await?;
        self.forecast_and_recall(access, &deck_ids, &clock, &mut stats).await?;
        self.count_answers(access, &deck_ids, &clock, &mut stats).await?;
        stats.streak_days = self.streak(access, &deck_ids, &clock).await?;
        Ok(stats)
    }

    async fn count_cards(&self, access: &Access, deck_ids: &[i64], clock: &Clock, stats: &mut Stats) -> Result<()> {
        // Everything is counted in the database: a long-time Anki user has
        // tens of thousands of cards and hundreds of thousands of reviews,
        // and loading them all made the home page take seconds.
        let rows: Vec<(CardState, bool, i64, i64)> = {
            let sql = "SELECT state, suspended, COUNT(*), SUM(due <= ?) FROM cards
                 WHERE user_id = ? AND deck_id IN (SELECT value FROM json_each(?)) GROUP BY state, suspended";
            let q = sqlx::query_as::<_, (CardState, bool, i64, i64)>(sql)
                .bind(clock.now)
                .bind(access.user_id())
                .bind(id_list(deck_ids));
            q.fetch_all(&self.db).await?
        };
        for (state, suspended, n, due_now) in rows {
            stats.total += n;
            if suspended {
                stats.suspended += n;
                continue;
            }
            match state {
                CardState::New => stats.new += n,
                CardState::Learning | CardState::Relearning => stats.learning += n,
                CardState::Review => stats.review += n,
            }
            if state != CardState::New {
                stats.due_now += due_now;
            }
        }

        Ok(())
    }

    async fn forecast_and_recall(
        &self,
        access: &Access,
        deck_ids: &[i64],
        clock: &Clock,
        stats: &mut Stats,
    ) -> Result<()> {
        let today = clock.today();
        // Forecast and recall chance: only the few numbers they need.
        let horizon = 14;
        let horizon_end = clock.day_start(today + Duration::days(num::usize_i64(horizon)));
        let sql = "SELECT json_group_array(json_array(deck_id, due, stability, last_review)) FROM cards
             WHERE user_id = ? AND deck_id IN (SELECT value FROM json_each(?)) AND state != 0 AND suspended = 0";
        let q = sqlx::query_scalar::<_, Json<Vec<(i64, i64, Option<f64>, Option<i64>)>>>(sql)
            .bind(access.user_id())
            .bind(id_list(deck_ids));
        let Json(learned) = q.fetch_one(&self.db).await?;
        let book = self.param_book(access).await?;
        let decays: HashMap<i64, f32> = deck_ids
            .iter()
            .map(|&d| scheduler::decay_for(book.for_deck(d)).map(|decay| (d, decay)))
            .collect::<std::result::Result<_, _>>()?;
        let mut forecast = vec![0i64; horizon];
        let (mut r_sum, mut r_n) = (0.0, 0);
        for (deck, due, stability, last_review) in learned {
            let decay = decays.get(&deck).copied().unwrap_or(fsrs::FSRS6_DEFAULT_DECAY);
            if due < horizon_end {
                let day = num::i64_usize((clock.date_of(due) - today).num_days().max(0));
                if let Some(n) = forecast.get_mut(day) {
                    *n += 1;
                }
            }
            if let (Some(s), Some(last)) = (stability, last_review) {
                let elapsed = num::i64_f32((clock.now - last).max(0)) / 86_400.0;
                let memory = fsrs::MemoryState { stability: num::f64_f32(s), difficulty: 5.0 };
                r_sum += f64::from(fsrs::current_retrievability(memory, elapsed, decay));
                r_n += 1;
            }
        }
        stats.forecast = forecast
            .into_iter()
            .enumerate()
            .map(|(i, n)| ((today + Duration::days(num::usize_i64(i))).format("%Y-%m-%d").to_string(), n))
            .collect();
        if r_n > 0 {
            stats.average_retrievability = Some(r_sum / f64::from(r_n));
        }

        Ok(())
    }

    /// Answers today and in the last 30 days: counts, minutes, recall.
    async fn count_answers(&self, access: &Access, deck_ids: &[i64], clock: &Clock, stats: &mut Stats) -> Result<()> {
        let today = clock.today();
        let since = |from: i64| {
            let sql = "SELECT COUNT(*), COUNT(DISTINCT card_id), COALESCE(SUM(duration_ms), 0),
                    COALESCE(SUM(state_before = 2), 0), COALESCE(SUM(state_before = 2 AND rating > 1), 0),
                    COALESCE(SUM(CASE WHEN duration_ms > 0 THEN duration_ms END), 0),
                    COALESCE(SUM(duration_ms > 0), 0)
                 FROM review_log
                 WHERE user_id = ? AND rating > 0 AND reviewed_at >= ? AND deck_id IN (SELECT value FROM json_each(?))";
            let user_id = access.user_id();
            let deck_ids = deck_ids.to_vec();
            async move {
                let q = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(sql)
                    .bind(user_id)
                    .bind(from)
                    .bind(id_list(&deck_ids));
                q.fetch_one(&self.db).await
            }
        };
        let (n, cards, ms, ..) = since(clock.start_of_today()).await?;
        stats.reviews_today = n;
        stats.cards_today = cards;
        stats.minutes_today = (num::i64_f64(ms) / 60_000.0 * 10.0).round() / 10.0;
        let (n, _, _, review_total, passed, timed_ms, timed) =
            since(clock.day_start(today - Duration::days(30))).await?;
        stats.reviews_30d = n;
        stats.retention_sample_30d = review_total;
        if review_total > 0 {
            stats.true_retention_30d = Some(num::i64_f64(passed) / num::i64_f64(review_total));
        }
        if timed >= 10 {
            stats.seconds_per_review = Some(num::i64_f64(timed_ms) / num::i64_f64(timed) / 1000.0);
        }

        Ok(())
    }

    /// Days in a row with a review, up to a year back; today counts once it
    /// has one, and doesn't break the streak before it has.
    async fn streak(&self, access: &Access, deck_ids: &[i64], clock: &Clock) -> Result<i64> {
        let today = clock.today();
        // One query asks every day of the last year at once: each pair is a
        // day's start and end in the user's zone, so DST is the clock's job.
        let days = day_bounds(clock, (0..=STREAK_DAYS).map(|back| today - Duration::days(back)));
        let sql = "SELECT CAST(d.key AS INTEGER) FROM json_each(?) d WHERE EXISTS (SELECT 1 FROM review_log
                WHERE user_id = ? AND rating > 0 AND reviewed_at >= d.value ->> 0 AND reviewed_at < d.value ->> 1
                AND deck_id IN (SELECT value FROM json_each(?)))";
        let studied: Vec<i64> = sqlx::query_scalar(sql)
            .bind(Json(&days))
            .bind(access.user_id())
            .bind(id_list(deck_ids))
            .fetch_all(&self.db)
            .await?;
        Ok(streak_days(&studied))
    }

    /// The last twelve weeks, for the charts on the desktop stats page:
    /// reviews on each day, and per week how many review-state answers
    /// there were and how many were recalled. Oldest first.
    ///
    /// # Errors
    ///
    /// - `NotFound` if a deck is given that the access doesn't see.
    /// - `Internal` if the database fails.
    pub async fn history(&self, access: &Access, deck_id: Option<i64>) -> Result<History> {
        let deck_ids: Vec<i64> = match deck_id {
            Some(id) => {
                self.deck(access, id).await?;
                vec![id]
            }
            None => self.decks(access).await?.into_iter().map(|d| d.id).collect(),
        };
        let clock = access.user.clock();
        let today = clock.today();
        let first = today - Duration::days(HISTORY_DAYS - 1);
        let mut days = vec![0i64; num::i64_usize(HISTORY_DAYS)];
        let mut weeks = vec![(0i64, 0i64); num::i64_usize(HISTORY_DAYS / 7)];
        if !deck_ids.is_empty() {
            // Counted per day in the database: twelve weeks are tens of
            // thousands of answers for a heavy user, too many to ship.
            let sql = "SELECT CAST(d.key AS INTEGER), COUNT(*), SUM(r.state_before = 2), SUM(r.state_before = 2 AND r.rating > 1)
                 FROM json_each(?) d JOIN review_log r ON r.user_id = ? AND r.reviewed_at >= d.value ->> 0
                     AND r.reviewed_at < d.value ->> 1
                 WHERE r.rating > 0 AND r.deck_id IN (SELECT value FROM json_each(?))
                 GROUP BY d.key";
            let q = sqlx::query_as::<_, (i64, i64, i64, i64)>(sql)
                .bind(Json(day_bounds(&clock, (0..HISTORY_DAYS).map(|i| first + Duration::days(i)))))
                .bind(access.user_id())
                .bind(id_list(&deck_ids));
            for (i, n, reviews, passed) in q.fetch_all(&self.db).await? {
                let i = num::i64_usize(i);
                if let Some(day) = days.get_mut(i) {
                    *day = n;
                }
                if let Some(w) = weeks.get_mut(i / 7) {
                    w.0 += reviews;
                    w.1 += passed;
                }
            }
        }
        Ok(History {
            days: days
                .into_iter()
                .enumerate()
                .map(|(i, n)| ((first + Duration::days(num::usize_i64(i))).format("%Y-%m-%d").to_string(), n))
                .collect(),
            weeks,
        })
    }
}

/// One answer as the coach and the weekly review read it.
pub(crate) struct Answer {
    pub deck_id: i64,
    pub card_id: i64,
    pub rating: i64,
    pub state_before: CardState,
    pub reviewed_at: i64,
    pub duration_ms: Option<i64>,
}

impl App {
    /// The user's own answers from `since` on, oldest first, imports left
    /// out. Read as one JSON array: six weeks are tens of thousands of
    /// answers for a heavy user, and sqlx hands rows over one at a time.
    pub(crate) async fn answers_since(&self, access: &Access, since: i64) -> Result<Vec<Answer>> {
        type Row = (i64, i64, i64, i64, i64, Option<i64>);
        let Json(rows): Json<Vec<Row>> = sqlx::query_scalar(
            "SELECT json_group_array(json_array(deck_id, card_id, rating, state_before, reviewed_at, duration_ms))
             FROM (SELECT * FROM review_log WHERE user_id = ? AND rating > 0 AND source != 'import' AND reviewed_at >= ?
                   ORDER BY reviewed_at, id)",
        )
        .bind(access.user_id())
        .bind(since)
        .fetch_one(&self.db)
        .await?;
        rows.into_iter()
            .map(|(deck_id, card_id, rating, state, reviewed_at, duration_ms)| {
                let state_before = CardState::from_stored(state)
                    .ok_or_else(|| AppError::Internal(anyhow::anyhow!("an answer has the unknown state {state}")))?;
                Ok(Answer { deck_id, card_id, rating, state_before, reviewed_at, duration_ms })
            })
            .collect()
    }
}

/// Each day's start and end in the user's zone, for queries that count
/// per day with `json_each`: the clock knows DST and rollover, SQL doesn't.
fn day_bounds(clock: &Clock, days: impl Iterator<Item = NaiveDate>) -> Vec<(i64, i64)> {
    days.map(|day| (clock.day_start(day), clock.day_start(day + Duration::days(1)))).collect()
}

/// How far back the streak looks, in days.
const STREAK_DAYS: i64 = 365;

/// The streak from the days studied, counted back from today (0 = today):
/// consecutive days up to yesterday, plus today once it is studied, so an
/// unfinished today doesn't break it.
fn streak_days(studied: &[i64]) -> i64 {
    let set: std::collections::HashSet<i64> = studied.iter().copied().collect();
    let first = i64::from(!set.contains(&0));
    (first..=STREAK_DAYS).take_while(|d| set.contains(d)).fold(0, |n, _| n + 1)
}

#[cfg(test)]
mod streak_tests {
    use super::streak_days;

    #[test]
    fn today_counts_once_studied_and_never_breaks_the_streak() {
        assert_eq!(streak_days(&[]), 0);
        assert_eq!(streak_days(&[0]), 1);
        assert_eq!(streak_days(&[1, 2, 3]), 3, "not yet studied today");
        assert_eq!(streak_days(&[0, 1, 2, 4]), 3, "a gap ends it");
        assert_eq!(streak_days(&[2, 3]), 0, "missed yesterday");
        assert_eq!(streak_days(&(0..400).collect::<Vec<_>>()), 366, "capped at a year");
    }
}
