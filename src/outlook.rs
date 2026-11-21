//! The coming week, forecast: how many cards each day will ask for if you
//! study every day.
//!
//! Counting cards by their current due date misses every card that comes
//! back after you review it: a lapse returns the next day, and a young card
//! graduates to an interval of a few days. So each card is followed forward
//! through the real scheduler. At every review it splits in two: Good with
//! its recall chance on that day, Again with the rest. Each branch carries
//! its probability, and a branch ends once it leaves the window. Every hop
//! takes at least a day, so the tree stays small: one branch per visit and
//! at most `DAYS` visits.
//!
//! The result is the exact expected count under the two-button model, with
//! no randomness. Cards are independent, so the variances of their visit
//! counts add up, and the week's bounds come from the sum.
//!
//! Learning steps are assumed to end with Good on the same day: a card
//! counts once a day however often it is shown, so in-day misses change
//! the count very little.

use std::collections::HashMap;

use chrono::Duration;
use fsrs::FSRS;

use crate::{
    App,
    auth::Access,
    error::Result,
    scheduler::{self, CardSched, CardState, Clock, Rating, SchedConfig},
    service::{Card, DeckSummary, sched_config_for},
};

/// Days in the outlook, today included.
pub const DAYS: usize = 7;

/// A branch this unlikely is dropped. Its expected visits are too few to
/// change a count shown in whole cards.
const MIN_MASS: f64 = 1e-4;

/// Steps a card can take within one day before the forecast gives up on it.
const MAX_STEPS_PER_DAY: usize = 16;

/// One deck: its scheduler, its cards and the new cards it will start.
pub struct DeckPlan {
    pub fsrs: FSRS,
    pub cfg: SchedConfig,
    pub decay: f32,
    /// Learned cards (any state but new), with their ids for the fuzz.
    pub cards: Vec<(i64, CardSched)>,
    /// New cards started on each day; index 0 is today.
    pub new_cards: [u32; DAYS],
}

/// Expected cards per day, and the spread of the days after today.
#[derive(Debug, Clone, PartialEq)]
pub struct Outlook {
    /// Expected different cards studied on each day; index 0 is today.
    pub days: [f64; DAYS],
    /// Variance of the total over the days after today.
    pub variance: f64,
    /// Cards certain to come in the days after today: those already due
    /// then, and the new cards started then.
    pub floor: f64,
}

impl Outlook {
    /// Expected cards over the days after today.
    pub fn later(&self) -> f64 {
        self.days[1..].iter().sum()
    }

    /// A 90% range for the days after today, in whole cards: the normal
    /// approximation of a sum of many independent counts, never below the
    /// cards already certain.
    pub fn later_range(&self) -> (i64, i64) {
        let mean = self.later();
        let spread = 1.645 * self.variance.max(0.0).sqrt();
        let low = (mean - spread).max(self.floor).floor() as i64;
        let high = (mean + spread).ceil() as i64;
        (low, high.max(low))
    }
}

impl App {
    /// The coming week for these decks (as the home lists them), if you
    /// study every day. Paused and archived decks rest, so they add nothing.
    pub async fn outlook(&self, access: &Access, decks: &[DeckSummary]) -> Result<Outlook> {
        let clock = access.user.clock();
        let presets: HashMap<i64, _> = self.list_presets(access).await?.into_iter().map(|p| (p.id, p)).collect();
        let book = self.param_book(access).await?;
        let horizon = clock.day_start(clock.today() + Duration::days(DAYS as i64));
        let cards: Vec<Card> =
            sqlx::query_as("SELECT * FROM cards WHERE user_id = ? AND state != 0 AND suspended = 0 AND due < ?")
                .bind(access.user_id())
                .bind(horizon)
                .fetch_all(&self.db)
                .await?;
        let mut by_deck: HashMap<i64, Vec<(i64, CardSched)>> = HashMap::new();
        for card in cards {
            by_deck.entry(card.deck_id).or_default().push((card.id, card.sched()));
        }
        let plans: Vec<DeckPlan> = decks
            .iter()
            .filter(|s| s.deck.status.is_active())
            .filter_map(|s| {
                let preset = presets.get(&s.deck.preset_id)?;
                let params = book.for_deck(s.deck.id);
                // Today's new cards are what the home offers; later days take
                // the daily limit from what is left (none in catch-up mode).
                let mut left = (s.new - s.new_today).max(0);
                let mut new_cards = [0u32; DAYS];
                new_cards[0] = u32::try_from(s.new_today.max(0)).unwrap_or(0);
                if !access.user.catch_up {
                    for n in &mut new_cards[1..] {
                        let today = preset.new_per_day.clamp(0, left);
                        left -= today;
                        *n = u32::try_from(today).unwrap_or(0);
                    }
                }
                Some(DeckPlan {
                    fsrs: scheduler::fsrs_for(params),
                    cfg: sched_config_for(preset, &s.deck),
                    decay: scheduler::decay_for(params),
                    cards: by_deck.remove(&s.deck.id).unwrap_or_default(),
                    new_cards,
                })
            })
            .collect();
        Ok(tokio::task::spawn_blocking(move || forecast(&plans, &clock))
            .await
            .map_err(|e| crate::error::AppError::Internal(e.into()))??)
    }
}

/// What one card adds: visits per day, and the first two moments of its
/// visit count over the days after today.
#[derive(Default)]
struct Acc {
    days: [f64; DAYS],
    m1: f64,
    m2: f64,
    floor: f64,
}

pub fn forecast(plans: &[DeckPlan], clock: &Clock) -> anyhow::Result<Outlook> {
    let mut total = Acc::default();
    for plan in plans {
        for (id, card) in &plan.cards {
            let mut acc = Acc::default();
            walk(plan, clock, *id, card.clone(), card.due.max(clock.now), 1.0, 0, &mut acc)?;
            add(&mut total, &acc, 1.0);
        }
        for (day, &n) in plan.new_cards.iter().enumerate() {
            if n == 0 {
                continue;
            }
            // New cards are alike, so one walk stands for all of them.
            let at = if day == 0 { clock.now } else { clock.day_start(clock.today() + Duration::days(day as i64)) };
            let mut acc = Acc::default();
            walk(plan, clock, 0, CardSched::new_card(), at, 1.0, 0, &mut acc)?;
            add(&mut total, &acc, f64::from(n));
        }
    }
    Ok(Outlook { days: total.days, variance: total.m2, floor: total.floor })
}

/// Adds `n` independent cards like `one`. Variances add up, so `m2` of the
/// total holds the summed variance, not a second moment.
fn add(total: &mut Acc, one: &Acc, n: f64) {
    for (t, d) in total.days.iter_mut().zip(one.days) {
        *t += n * d;
    }
    total.m2 += n * (one.m2 - one.m1 * one.m1);
    total.floor += n * one.floor;
}

/// Follows one card from a review at `at`, carrying probability `p` and
/// `k` visits so far after today.
#[allow(clippy::too_many_arguments)]
fn walk(
    plan: &DeckPlan,
    clock: &Clock,
    id: i64,
    card: CardSched,
    at: i64,
    p: f64,
    k: u32,
    acc: &mut Acc,
) -> anyhow::Result<()> {
    let day = (clock.date_of(at) - clock.today()).num_days().max(0) as usize;
    if day >= DAYS {
        return leaf(acc, p, k);
    }
    acc.days[day] += p;
    let k = if day > 0 {
        if k == 0 && (p - 1.0).abs() < f64::EPSILON {
            acc.floor += 1.0;
        }
        k + 1
    } else {
        k
    };
    if p < MIN_MASS {
        return leaf(acc, p, k);
    }
    let recall = match (card.state, card.memory(), card.last_review) {
        (CardState::Review, Some(memory), Some(last)) => {
            let elapsed = ((at - last).max(0) as f32) / 86_400.0;
            f64::from(fsrs::current_retrievability(memory, elapsed, plan.decay)).clamp(0.0, 1.0)
        }
        // Learning cards are followed through their steps with Good.
        _ => 1.0,
    };
    for (q, rating) in [(recall, Rating::Good), (1.0 - recall, Rating::Again)] {
        if q <= 0.0 {
            continue;
        }
        let next = settle(plan, clock, id, &card, rating, at)?;
        let due = next.due;
        walk(plan, clock, id, next, due, p * q, k, acc)?;
    }
    Ok(())
}

fn leaf(acc: &mut Acc, p: f64, k: u32) -> anyhow::Result<()> {
    let k = f64::from(k);
    acc.m1 += p * k;
    acc.m2 += p * k * k;
    Ok(())
}

/// Answers `rating` at `at`, then Good at each step that is still due the
/// same day, so the card leaves the day the way studying would let it.
fn settle(
    plan: &DeckPlan,
    clock: &Clock,
    id: i64,
    card: &CardSched,
    rating: Rating,
    at: i64,
) -> anyhow::Result<CardSched> {
    let at_clock = |t: i64| Clock::new(t, clock.tz, clock.rollover_hour);
    let day = clock.date_of(at);
    let mut out = scheduler::answer(&plan.fsrs, &plan.cfg, id, card, rating, &at_clock(at))?.card;
    for _ in 0..MAX_STEPS_PER_DAY {
        if out.state == CardState::Review || clock.date_of(out.due) != day {
            return Ok(out);
        }
        out = scheduler::answer(&plan.fsrs, &plan.cfg, id, &out, Rating::Good, &at_clock(out.due))?.card;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    fn clock() -> Clock {
        // 2025-10-09 11:33 in Berlin, well after the 4 AM rollover.
        Clock::new(1_760_000_000, chrono_tz::Europe::Berlin, 4)
    }

    fn plan(cards: Vec<(i64, CardSched)>, new_cards: [u32; DAYS]) -> DeckPlan {
        DeckPlan {
            fsrs: FSRS::default(),
            cfg: SchedConfig {
                desired_retention: 0.9,
                learning_steps: vec![600],
                relearning_steps: vec![600],
                max_interval: 36500,
                exam_date: None,
            },
            decay: fsrs::FSRS6_DEFAULT_DECAY,
            cards,
            new_cards,
        }
    }

    /// A learned card due at the start of the given day, last seen so long
    /// ago that its recall chance there is about the target.
    fn learned(stability: f32, day: i64) -> CardSched {
        let c = clock();
        let due = c.day_start(c.today() + Duration::days(day));
        CardSched {
            state: CardState::Review,
            step: 0,
            stability: Some(stability),
            difficulty: Some(5.0),
            due,
            last_review: Some(due - (stability.max(1.0) as i64) * DAY),
            reps: 3,
            lapses: 0,
        }
    }

    fn outlook(cards: Vec<CardSched>, new_cards: [u32; DAYS]) -> Outlook {
        let cards = cards.into_iter().enumerate().map(|(i, c)| (i as i64 + 1, c)).collect();
        forecast(&[plan(cards, new_cards)], &clock()).unwrap()
    }

    #[test]
    fn nothing_to_forecast() {
        let o = outlook(vec![], [0; DAYS]);
        assert_eq!(o.days, [0.0; DAYS]);
        assert_eq!(o.later_range(), (0, 0));
    }

    #[test]
    fn a_card_due_after_the_week_adds_nothing() {
        let o = outlook(vec![learned(50.0, 9)], [0; DAYS]);
        assert_eq!(o.later(), 0.0);
        assert_eq!(o.variance, 0.0);
    }

    #[test]
    fn a_young_card_comes_back_within_the_week() {
        let o = outlook(vec![learned(1.0, 1)], [0; DAYS]);
        assert!((o.days[1] - 1.0).abs() < 1e-9, "due tomorrow: {:?}", o.days);
        assert!(o.later() > 1.5, "a one-day card returns more than once: {:?}", o.days);
        assert!(o.variance > 0.0);
    }

    #[test]
    fn a_strong_card_mostly_stays_away_but_lapses_return() {
        let o = outlook(vec![learned(100.0, 1)], [0; DAYS]);
        let returns = o.later() - 1.0;
        // Its recall chance is about 90%, so about one in ten comes back.
        assert!(returns > 0.05 && returns < 0.2, "{:?}", o.days);
        assert!(o.later() < outlook(vec![learned(1.0, 1)], [0; DAYS]).later());
    }

    #[test]
    fn overdue_and_today_cards_count_today_only_once() {
        let o = outlook(vec![learned(3.0, -2), learned(3.0, 0)], [0; DAYS]);
        assert!((o.days[0] - 2.0).abs() < 1e-9, "{:?}", o.days);
        assert!(o.later() > 0.0, "they come back later in the week");
    }

    #[test]
    fn new_cards_start_on_their_day_and_return() {
        let o = outlook(vec![], [0, 0, 5, 0, 0, 0, 0]);
        assert_eq!(o.days[1], 0.0);
        assert!((o.days[2] - 5.0).abs() < 1e-9, "{:?}", o.days);
        assert!(o.days[3..].iter().sum::<f64>() > 0.0, "new cards come back: {:?}", o.days);
        assert!((o.floor - 5.0).abs() < 1e-9);
    }

    #[test]
    fn the_range_holds_the_mean_and_never_dips_below_the_certain() {
        let cards = (1..=40).map(|i| learned(1.0 + f32::from(i as u8) / 4.0, i % 6 + 1)).collect();
        let o = outlook(cards, [0, 3, 3, 3, 3, 3, 3]);
        let (low, high) = o.later_range();
        let mean = o.later();
        assert!((low as f64) <= mean && mean <= high as f64, "{low} {mean} {high}");
        assert!(low as f64 >= o.floor.floor(), "{low} below {}", o.floor);
        assert!(high > low);
    }

    #[test]
    fn the_same_inputs_give_the_same_forecast() {
        let cards: Vec<_> = (1..=10).map(|i| learned(2.0, i % 4)).collect();
        assert_eq!(outlook(cards.clone(), [2; DAYS]), outlook(cards, [2; DAYS]));
    }
}
