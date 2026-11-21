//! FSRS-based scheduling.
//!
//! Memory-state math comes from the `fsrs` crate (FSRS-6). This module adds the
//! card lifecycle around it: learning/relearning steps, day boundaries in the
//! user's timezone, interval fuzz, maximum intervals and exam-date caps.

use chrono::{Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use fsrs::{FSRS, MemoryState};
use serde::{Deserialize, Serialize};

use crate::num;

/// Stored as its number; a row with any other number fails to load
/// instead of passing for a new card.
///
/// The numbers are part of the database format, like a column: the SQL
/// writes them as literals (`state = 2` is review, `state IN (1, 3)` is
/// learning or relearning), and so do Anki's files and the migrations.
/// They never change; `the_stored_numbers_never_change` holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[repr(i64)]
pub enum CardState {
    New = 0,
    Learning = 1,
    Review = 2,
    Relearning = 3,
}

impl CardState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Learning => "learning",
            Self::Review => "review",
            Self::Relearning => "relearning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
}

impl Rating {
    pub const fn from_i64(v: i64) -> Option<Self> {
        match v {
            1 => Some(Self::Again),
            2 => Some(Self::Hard),
            3 => Some(Self::Good),
            4 => Some(Self::Easy),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "again" => Some(Self::Again),
            "2" | "hard" => Some(Self::Hard),
            "3" | "good" => Some(Self::Good),
            "4" | "easy" => Some(Self::Easy),
            _ => None,
        }
    }

    pub const ALL: [Self; 4] = [Self::Again, Self::Hard, Self::Good, Self::Easy];
}

/// The scheduling-relevant part of a card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardSched {
    pub state: CardState,
    pub step: i64,
    pub stability: Option<f32>,
    pub difficulty: Option<f32>,
    pub due: i64,
    pub last_review: Option<i64>,
    pub reps: i64,
    pub lapses: i64,
}

impl CardSched {
    pub const fn new_card() -> Self {
        Self {
            state: CardState::New,
            step: 0,
            stability: None,
            difficulty: None,
            due: 0,
            last_review: None,
            reps: 0,
            lapses: 0,
        }
    }

    pub const fn memory(&self) -> Option<MemoryState> {
        match (self.stability, self.difficulty) {
            (Some(stability), Some(difficulty)) => Some(MemoryState { stability, difficulty }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SchedConfig {
    pub desired_retention: f32,
    /// Learning steps in seconds.
    pub learning_steps: Vec<i64>,
    /// Relearning steps in seconds.
    pub relearning_steps: Vec<i64>,
    pub max_interval: i64,
    pub exam_date: Option<NaiveDate>,
}

/// User-local day arithmetic. A "day" starts at `rollover_hour` local time,
/// like Anki's "next day starts at".
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    pub now: i64,
    pub tz: Tz,
    pub rollover_hour: u32,
}

impl Clock {
    pub fn new(now: i64, tz: Tz, rollover_hour: u32) -> Self {
        Self { now, tz, rollover_hour: rollover_hour.min(23) }
    }

    pub fn date_of(&self, ts: i64) -> NaiveDate {
        let utc = Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now);
        let local = utc.with_timezone(&self.tz).naive_local();
        (local - Duration::hours(i64::from(self.rollover_hour))).date()
    }

    pub fn today(&self) -> NaiveDate {
        self.date_of(self.now)
    }

    /// Unix timestamp at which the given (user-local) day starts.
    pub fn day_start(&self, date: NaiveDate) -> i64 {
        let naive = date.and_time(NaiveTime::MIN) + Duration::hours(i64::from(self.rollover_hour));
        match self.tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => t.timestamp(),
            chrono::LocalResult::Ambiguous(a, _) => a.timestamp(),
            // DST gap: fall forward an hour.
            chrono::LocalResult::None => self
                .tz
                .from_local_datetime(&(naive + Duration::hours(1)))
                .earliest()
                .map_or_else(|| naive.and_utc().timestamp(), |t| t.timestamp()),
        }
    }

    pub fn end_of_today(&self) -> i64 {
        self.day_start(self.today() + Duration::days(1))
    }

    pub fn start_of_today(&self) -> i64 {
        self.day_start(self.today())
    }

    pub fn days_between(&self, from_ts: i64, to_ts: i64) -> i64 {
        (self.date_of(to_ts) - self.date_of(from_ts)).num_days()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Outcome {
    pub card: CardSched,
    /// Seconds until the card is due again.
    pub interval_secs: i64,
    /// Whole days until due, for cards scheduled by day (review state).
    pub interval_days: Option<i64>,
    pub elapsed_days: i64,
}

/// The model for these parameters; empty means the defaults.
///
/// # Errors
/// Parameters that are not a valid FSRS set (wrong count, out of range).
pub fn fsrs_for(params: &[f32]) -> Result<FSRS, fsrs::FSRSError> {
    FSRS::new(params)
}

/// How fast memories fade under these parameters (FSRS-6's last one).
///
/// # Errors
/// Parameters that are not a valid FSRS set.
pub fn decay_for(params: &[f32]) -> Result<f32, fsrs::FSRSError> {
    Ok(fsrs::check_and_fill_parameters(params)?.get(20).copied().unwrap_or(fsrs::FSRS6_DEFAULT_DECAY))
}

/// Probability of recall right now.
pub fn retrievability(card: &CardSched, decay: f32, clock: &Clock) -> Option<f32> {
    let memory = card.memory()?;
    let last = card.last_review?;
    let elapsed = num::i64_f32((clock.now - last).max(0)) / 86_400.0;
    Some(fsrs::current_retrievability(memory, elapsed, decay))
}

/// Deterministic pseudo-random number in [0, 1) so previews match answers.
fn fuzz_seed(card_id: i64, reps: i64) -> f64 {
    let mut x = num::i64_u64(card_id)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(num::i64_u64(reps).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    x ^= x >> 31;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 29;
    num::u64_f64(x >> 11) / num::u64_f64(1u64 << 53)
}

/// Interval fuzz as used by Anki and the FSRS reference implementations:
/// spreads reviews so cards added together don't stay clumped together.
fn fuzzed_interval(interval: f64, max_interval: i64, seed: f64) -> i64 {
    if interval < 2.5 {
        return num::f64_i64(interval.round()).clamp(1, max_interval.max(1));
    }
    const RANGES: [(f64, f64, f64); 3] = [(2.5, 7.0, 0.15), (7.0, 20.0, 0.1), (20.0, f64::MAX, 0.05)];
    let mut delta = 1.0;
    for (start, end, factor) in RANGES {
        delta = factor.mul_add((interval.min(end) - start).max(0.0), delta);
    }
    let mut min_ivl = num::f64_i64((interval - delta).round()).max(2);
    let max_ivl = num::f64_i64((interval + delta).round());
    min_ivl = min_ivl.min(max_ivl);
    let ivl = min_ivl + num::f64_i64((seed * num::i64_f64(max_ivl - min_ivl + 1)).floor());
    ivl.clamp(1, max_interval.max(1))
}

fn effective_max_interval(cfg: &SchedConfig, clock: &Clock) -> i64 {
    let mut max = cfg.max_interval.max(1);
    if let Some(exam) = cfg.exam_date {
        let days_left = (exam - clock.today()).num_days();
        if days_left >= 1 {
            max = max.min(days_left);
        }
    }
    max
}

/// Where a card in its short steps goes next: `Some((step, delay))` to
/// stay in the steps, `None` to graduate to FSRS. Again starts over, Hard
/// repeats the step (the first one a bit longer), Good moves on, Easy and
/// running out of steps graduate.
fn next_step(rating: Rating, steps: &[i64], step: usize) -> Option<(usize, i64)> {
    match (rating, steps) {
        (_, []) | (Rating::Easy, _) => None,
        (Rating::Again, [first, ..]) => Some((0, *first)),
        (Rating::Hard, [first, rest @ ..]) => {
            let idx = step.min(rest.len());
            let delay = match (idx, rest) {
                // One step: half as long again, but at most a day longer.
                (0, []) => num::f64_i64(num::i64_f64(*first) * 1.5).min(first + 86_400),
                (0, [second, ..]) => i64::midpoint(*first, *second),
                _ => steps.get(idx).copied().unwrap_or(*first),
            };
            Some((idx, delay))
        }
        (Rating::Good, _) => steps.get(step + 1).map(|&delay| (step + 1, delay)),
    }
}

/// Compute the card's new state after answering with `rating`.
pub fn answer(
    fsrs: &FSRS,
    cfg: &SchedConfig,
    card_id: i64,
    card: &CardSched,
    rating: Rating,
    clock: &Clock,
) -> anyhow::Result<Outcome> {
    let elapsed_days = card.last_review.map_or(0, |last| clock.days_between(last, clock.now).max(0));
    let next =
        fsrs.next_states(card.memory(), cfg.desired_retention, num::i64_u32(elapsed_days.min(i64::from(u32::MAX))))?;
    let chosen = match rating {
        Rating::Again => &next.again,
        Rating::Hard => &next.hard,
        Rating::Good => &next.good,
        Rating::Easy => &next.easy,
    };

    let max_interval = effective_max_interval(cfg, clock);
    let seed = fuzz_seed(card_id, card.reps);
    let mut out = card.clone();
    out.stability = Some(chosen.memory.stability);
    out.difficulty = Some(chosen.memory.difficulty);
    out.last_review = Some(clock.now);
    out.reps += 1;

    let mut graduated_days = None;
    let mut graduate = |out: &mut CardSched| -> i64 {
        let mut days = fuzzed_interval(f64::from(chosen.interval), max_interval, seed);
        // Keep the button order sensible: Easy should never be sooner than Good.
        if rating == Rating::Easy {
            let good_days = num::f64_i64(f64::from(next.good.interval).round().max(1.0));
            days = days.max((good_days + 1).min(max_interval));
        }
        out.state = CardState::Review;
        out.step = 0;
        out.due = clock.day_start(clock.today() + Duration::days(days));
        graduated_days = Some(days);
        out.due - clock.now
    };

    let interval_secs = match card.state {
        CardState::Review if rating == Rating::Again => {
            out.lapses += 1;
            if let Some(&first) = cfg.relearning_steps.first() {
                (out.state, out.step, out.due) = (CardState::Relearning, 0, clock.now + first);
                first
            } else {
                graduate(&mut out)
            }
        }
        CardState::Review => graduate(&mut out),
        CardState::New | CardState::Learning | CardState::Relearning => {
            let relearning = card.state == CardState::Relearning;
            let steps = if relearning { &cfg.relearning_steps } else { &cfg.learning_steps };
            let step = if card.state == CardState::New { 0 } else { num::i64_usize(card.step.max(0)) };
            match next_step(rating, steps, step) {
                None => graduate(&mut out),
                Some((idx, delay)) => {
                    let state = if relearning { CardState::Relearning } else { CardState::Learning };
                    (out.state, out.step, out.due) = (state, num::usize_i64(idx), clock.now + delay);
                    delay
                }
            }
        }
    };

    Ok(Outcome { card: out, interval_secs, interval_days: graduated_days, elapsed_days })
}

/// The longest learning step; anything longer is FSRS's job anyway.
const MAX_STEP_SECS: i64 = 365 * 86_400;

/// Parse steps like "1m 10m 1h" into seconds.
pub fn parse_steps(s: &str) -> Result<Vec<i64>, String> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(|t| {
            let (num, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(t.len()));
            let n: f64 = num.parse().map_err(|_| format!("invalid step '{t}'"))?;
            let mult = match unit {
                "s" => 1.0,
                "" | "m" => 60.0,
                "h" => 3600.0,
                "d" => 86_400.0,
                _ => return Err(format!("invalid unit in step '{t}' (use s, m, h or d)")),
            };
            let secs = (n * mult).round();
            if secs <= 0.0 {
                return Err(format!("step '{t}' must be positive"));
            }
            if secs > num::i64_f64(MAX_STEP_SECS) {
                return Err(format!("step '{t}' is too long (at most 365d)"));
            }
            let secs = num::f64_i64(secs);
            Ok(secs)
        })
        .collect()
}

pub fn format_steps(steps: &[i64]) -> String {
    steps
        .iter()
        .map(|&s| {
            if s % 86_400 == 0 {
                format!("{}d", s / 86_400)
            } else if s % 3600 == 0 {
                format!("{}h", s / 3600)
            } else if s % 60 == 0 {
                format!("{}m", s / 60)
            } else {
                format!("{s}s")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Outcome {
    pub fn label(&self) -> String {
        match self.interval_days {
            Some(d) => crate::format::days(d),
            None => crate::format::interval(self.interval_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_numbers_never_change() {
        let stored = [CardState::New, CardState::Learning, CardState::Review, CardState::Relearning].map(|s| s as i64);
        assert_eq!(stored, [0, 1, 2, 3]);
    }

    fn cfg() -> SchedConfig {
        SchedConfig {
            desired_retention: 0.9,
            learning_steps: vec![600],
            relearning_steps: vec![600],
            max_interval: 36500,
            exam_date: None,
        }
    }

    fn clock(now: i64) -> Clock {
        Clock::new(now, chrono_tz::Europe::Berlin, 4)
    }

    #[test]
    fn new_card_lifecycle() {
        let fsrs = FSRS::default();
        let c = clock(1_760_000_000);
        let card = CardSched::new_card();

        let again = answer(&fsrs, &cfg(), 1, &card, Rating::Again, &c).unwrap();
        assert_eq!(again.card.state, CardState::Learning);
        assert_eq!(again.interval_secs, 600);

        let c2 = clock(c.now + 600);
        let good = answer(&fsrs, &cfg(), 1, &again.card, Rating::Good, &c2).unwrap();
        assert_eq!(good.card.state, CardState::Review);
        assert!(good.interval_secs >= 3600);
        assert_eq!(good.card.reps, 2);

        // Lapse goes into relearning.
        let c3 = clock(good.card.due + 10);
        let lapse = answer(&fsrs, &cfg(), 1, &good.card, Rating::Again, &c3).unwrap();
        assert_eq!(lapse.card.state, CardState::Relearning);
        assert_eq!(lapse.card.lapses, 1);
        assert!(lapse.card.stability.unwrap() < good.card.stability.unwrap());
    }

    #[test]
    fn good_on_new_graduates_with_single_step() {
        let fsrs = FSRS::default();
        let c = clock(1_760_000_000);
        let out = answer(&fsrs, &cfg(), 7, &CardSched::new_card(), Rating::Good, &c).unwrap();
        assert_eq!(out.card.state, CardState::Review);
        assert_eq!(c.days_between(c.now, out.card.due), 2);
    }

    #[test]
    fn exam_date_caps_interval() {
        let fsrs = FSRS::default();
        let c = clock(1_760_000_000);
        let mut config = cfg();
        config.exam_date = Some(c.today() + Duration::days(3));
        let mut card = CardSched::new_card();
        card.state = CardState::Review;
        card.stability = Some(100.0);
        card.difficulty = Some(5.0);
        card.last_review = Some(c.now - 100 * 86_400);
        let out = answer(&fsrs, &config, 1, &card, Rating::Good, &c).unwrap();
        assert!(c.days_between(c.now, out.card.due) <= 3);
    }

    #[test]
    fn steps_roundtrip() {
        let steps = parse_steps("1m 10m, 1h 2d").unwrap();
        assert_eq!(steps, vec![60, 600, 3600, 172_800]);
        assert_eq!(format_steps(&steps), "1m 10m 1h 2d");
        assert!(parse_steps("5x").is_err());
        assert!(parse_steps("99999999999d").is_err());
        assert_eq!(parse_steps("365d").unwrap(), vec![365 * 86_400]);
        assert_eq!(parse_steps("").unwrap(), Vec::<i64>::new());
    }

    #[test]
    fn fuzz_stays_in_range() {
        for i in 0..200 {
            let ivl = fuzzed_interval(30.0, 36500, fuzz_seed(i, 3));
            assert!((27..=33).contains(&ivl), "{ivl}");
        }
        assert_eq!(fuzzed_interval(1.2, 36500, 0.5), 1);
        assert_eq!(fuzzed_interval(300.0, 10, 0.5), 10);
    }
}
