//! Smart breaks: an even rhythm of work stretches and breaks, steered by
//! how the answers run, and tuned to the user over time.
//!
//! - A work stretch counts time on cards (a tab hidden for a moment is still
//!   work; paused time and gaps are not). It starts at `START_WORK_MIN`.
//! - A break comes at 1.1 stretches at the latest, from 0.9 stretches when
//!   the answers run slow, and after `MIN_WORK_MIN` when they clearly slump.
//! - "Slow" is a running mean of how much longer each card took than it
//!   usually does (natural log of the ratio). One card says almost nothing:
//!   its time varies by about 0.35 while tiredness adds 0.1 or so, which is
//!   why only the mean over the last twenty-odd cards is trusted.
//! - A short break is a fifth of the stretch, at least five minutes; every
//!   fourth is a long one, three times as long.
//! - Pressing Keep going makes the next stretches longer and the breaks
//!   shorter; pausing before any suggestion does the opposite.
//!
//! The numbers come from simulating several hundred kinds of learners over
//! months of study (see the study-breaks notes); real timing data should
//! retune them.

/// The first work stretch, in minutes of time on cards.
pub const START_WORK_MIN: f64 = 25.0;
/// Learned stretches stay inside this range.
const WORK_RANGE: (f64, f64) = (15.0, 45.0);
/// Learned break lengths stay inside this multiple of the plain length.
const MULT_RANGE: (f64, f64) = (0.8, 2.5);
/// How much one Keep going, or one own pause, moves the rhythm.
const LEARN_STEP: f64 = 1.1;
/// A break may come this early in a stretch when the answers run slow...
const EARLY: f64 = 0.9;
/// ...and comes this late at the latest.
const LATE: f64 = 1.1;
/// No break before this much work, however slow the answers.
const MIN_WORK_MIN: f64 = 10.0;
/// A running slowness this high brings the break forward (about energy 50
/// in the simulation's terms)...
const SLOW: f64 = 0.08;
/// ...and this high is a slump: a break after `MIN_WORK_MIN` (energy 40).
const SLUMP: f64 = 0.15;
/// Weight of each new card in the running mean: about the last twenty.
const WEIGHT: f64 = 0.05;
/// One odd card (a sneeze) moves the mean at most this much, in log units.
const CLIP: f64 = 1.0;
/// Earlier answers a card needs before its time tells anything.
const MIN_HISTORY: u32 = 2;
/// A rest this long between two answers is a break: the stretch starts over.
pub const REST_SECS: i64 = 3 * 60;
/// After a gap this long the day's rhythm starts from scratch.
const FRESH_START_SECS: i64 = 60 * 60;
/// Slowness fades during a rest with this time constant, in minutes.
const RECOVERY_MIN: f64 = 5.0;
/// A tab hidden shorter than this still measures the card.
const AWAY_MEASURED_MS: i64 = 10_000;
/// A tab hidden longer than this was a rest, not other work.
const AWAY_REST_MS: i64 = REST_SECS * 1000;
/// An answer without times counts as at most this much work.
const UNTIMED_MS: i64 = 60_000;
const SHORT_MIN: f64 = 5.0;
const LONG_EVERY: usize = 4;
const LONG_TIMES: f64 = 3.0;
const MAX_BREAK_MIN: f64 = 45.0;

/// One answer on the website, oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Step {
    pub at: i64,
    /// Front to answer, back, and hidden tab time, as measured in the
    /// browser; `None` without JavaScript.
    pub think_ms: Option<i64>,
    pub answer_ms: Option<i64>,
    pub away_ms: Option<i64>,
    /// The card's usual front time (mean natural log of ms), when it has
    /// enough history and this answer's time means what it usually does.
    pub usual: Option<f64>,
}

/// The user's learned rhythm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rhythm {
    pub work_min: f64,
    pub mult: f64,
}

impl Default for Rhythm {
    fn default() -> Self {
        Self { work_min: START_WORK_MIN, mult: 1.0 }
    }
}

impl Rhythm {
    /// Any stored values, kept in range.
    pub fn new(work_min: f64, mult: f64) -> Self {
        let ok = |v: f64, (lo, hi): (f64, f64), d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Self { work_min: ok(work_min, WORK_RANGE, START_WORK_MIN), mult: ok(mult, MULT_RANGE, 1.0) }
    }

    /// Keep going while fresh: longer stretches, shorter breaks.
    #[must_use]
    pub fn kept_going(self) -> Self {
        Self::new(self.work_min * LEARN_STEP, self.mult / LEARN_STEP)
    }

    /// A pause before any suggestion: shorter stretches, longer breaks.
    #[must_use]
    pub fn paused_early(self) -> Self {
        Self::new(self.work_min / LEARN_STEP, self.mult * LEARN_STEP)
    }

    /// A plain break, in minutes.
    fn short_min(self) -> f64 {
        (self.work_min / 5.0).max(SHORT_MIN) * self.mult
    }
}

/// Where the day stands after its answers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct State {
    /// Minutes of work in the current stretch.
    pub worked_min: f64,
    /// Running mean of how much slower than usual the cards were.
    pub slowness: f64,
    /// Breaks taken since the day's rhythm started, own pauses included.
    pub breaks: usize,
    /// When the last answer came right after a break: the minutes of work
    /// before that break.
    pub rested_after: Option<f64>,
}

/// Walk through the day's answers. `watch_from` is the last Keep going:
/// work before it doesn't count toward the stretch.
pub fn replay(steps: &[Step], watch_from: i64) -> State {
    let mut st = State::default();
    let mut prev: Option<i64> = None;
    for s in steps {
        let gap_ms = prev.map(|p| (s.at - p).max(0) * 1000);
        let away = s.away_ms.unwrap_or(0).max(0);
        let (work_ms, rest_ms) = match (s.think_ms, gap_ms) {
            (Some(t), gap) => {
                let card = t.max(0) + s.answer_ms.unwrap_or(0).max(0);
                let (work, away_rest) = if away >= AWAY_REST_MS { (card, away) } else { (card + away, 0) };
                // Whatever the gap held beyond the card was rest: the Pause
                // cover, or time before the card was opened.
                (work, gap.map_or(0, |g| (g - work).max(away_rest)))
            }
            (None, Some(g)) => (g.min(UNTIMED_MS), (g - UNTIMED_MS).max(0)),
            (None, None) => (0, 0),
        };
        st.rested_after = None;
        if gap_ms.is_some_and(|g| g >= FRESH_START_SECS * 1000) {
            st = State::default();
        } else if rest_ms >= REST_SECS * 1000 {
            st.breaks += 1;
            st.rested_after = Some(st.worked_min);
            st.worked_min = 0.0;
            st.slowness *= (-(rest_ms as f64) / 60_000.0 / RECOVERY_MIN).exp();
        }
        if s.at <= watch_from {
            st.worked_min = 0.0;
        } else {
            st.worked_min += work_ms as f64 / 60_000.0;
        }
        if let (Some(t), Some(usual)) = (s.think_ms.filter(|t| *t > 0), s.usual)
            && away < AWAY_MEASURED_MS
        {
            let slower = ((t as f64).ln() - usual).clamp(-CLIP, CLIP);
            st.slowness = (1.0 - WEIGHT) * st.slowness + WEIGHT * slower;
        }
        prev = Some(s.at);
    }
    st
}

/// A break to suggest now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    /// Minutes of work in this stretch.
    pub worked: i64,
    pub minutes: i64,
    pub long: bool,
    /// The answers were running slow.
    pub slowing: bool,
}

/// Is a break due, and how long should it be?
pub fn suggest(st: &State, r: Rhythm) -> Option<Suggestion> {
    let w = st.worked_min;
    let slowing = st.slowness >= SLOW;
    let due =
        w >= LATE * r.work_min || (w >= EARLY * r.work_min && slowing) || (w >= MIN_WORK_MIN && st.slowness >= SLUMP);
    if !due {
        return None;
    }
    let k = st.breaks + 1;
    let long = k.is_multiple_of(LONG_EVERY) || (k % LONG_EVERY == LONG_EVERY - 1 && st.slowness >= SLUMP);
    let minutes = if long { r.short_min() * LONG_TIMES } else { r.short_min() };
    Some(Suggestion { worked: w.round() as i64, minutes: minutes.min(MAX_BREAK_MIN).round() as i64, long, slowing })
}

/// Was the latest answer the first after a pause the user took on their
/// own, before any suggestion, with enough work behind it to mean
/// tiredness? `shown_at` is when a break was last suggested.
pub fn paused_on_own(st: &State, steps: &[Step], shown_at: Option<i64>) -> bool {
    let Some(before) = st.rested_after else { return false };
    let [.., prev, last] = steps else { return false };
    let suggested = shown_at.is_some_and(|t| t >= prev.at && t <= last.at);
    before >= MIN_WORK_MIN && !suggested
}

/// For each answer, the mean log front time of its card's earlier answers,
/// once there were `MIN_HISTORY`.
pub fn card_usuals(answers: &[(i64, Option<i64>)]) -> Vec<Option<f64>> {
    let mut seen: std::collections::HashMap<i64, (f64, u32)> = std::collections::HashMap::new();
    answers
        .iter()
        .map(|&(card, ms)| {
            let e = seen.entry(card).or_insert((0.0, 0));
            let usual = (e.1 >= MIN_HISTORY).then(|| e.0 / f64::from(e.1));
            if let Some(ms) = ms.filter(|ms| *ms > 0) {
                e.0 += (ms as f64).ln();
                e.1 += 1;
            }
            usual
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic noise source, so tests don't need a crate.
    struct Noise(u64);
    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
        }
        /// Roughly normal, from twelve uniforms.
        fn normal(&mut self) -> f64 {
            (0..12).map(|_| self.next()).sum::<f64>() - 6.0
        }
    }

    const USUAL_MS: f64 = 5000.0;

    /// Cards back to back for `minutes`, each usually 5 s plus 2 s on the
    /// back, with the usual spread. From `slow_from` minutes of work on,
    /// fronts take `slow` (log units) longer.
    fn study(seed: u64, start: i64, minutes: f64, slow_from: f64, slow: f64) -> Vec<Step> {
        let mut n = Noise(seed);
        let mut out = vec![];
        let (mut t, mut worked) = (start * 1000, 0.0);
        while worked < minutes * 60_000.0 {
            let extra = if worked >= slow_from * 60_000.0 { slow } else { 0.0 };
            let think = (USUAL_MS * (0.35 * n.normal() + extra).exp()) as i64;
            let answer = 2000;
            t += think + answer;
            worked += (think + answer) as f64;
            out.push(Step {
                at: t / 1000,
                think_ms: Some(think),
                answer_ms: Some(answer),
                away_ms: Some(0),
                usual: Some(USUAL_MS.ln()),
            });
        }
        out
    }

    fn due(steps: &[Step]) -> Option<Suggestion> {
        suggest(&replay(steps, 0), Rhythm::default())
    }

    #[test]
    fn short_fresh_days_are_left_alone() {
        // 200 fresh days of 20 minutes: at most a few wrong suggestions.
        let wrong = (0..200).filter(|&seed| due(&study(seed, 0, 20.0, 999.0, 0.0)).is_some()).count();
        assert!(wrong <= 6, "{wrong} of 200 interrupted");
    }

    #[test]
    fn a_fresh_stretch_ends_on_the_clock() {
        // Fresh all along: no break at 25 minutes, one by 27.5.
        assert!(due(&study(1, 0, 24.0, 999.0, 0.0)).is_none());
        let s = due(&study(1, 0, 27.6, 999.0, 0.0)).expect("by 1.1 stretches");
        assert_eq!(s, Suggestion { worked: 28, minutes: 5, long: false, slowing: false });
    }

    #[test]
    fn slow_answers_bring_the_break_forward() {
        // Clearly slower from minute 5 on: a break soon after minute 10.
        assert!(due(&study(2, 0, 9.5, 5.0, 0.5)).is_none(), "never before ten minutes");
        let s = due(&study(2, 0, 13.0, 5.0, 0.5)).expect("a slump");
        assert!(s.slowing && s.worked < 22, "{s:?}");
        // A mild slowdown waits for the early end of the stretch.
        let mild = |m| due(&study(3, 0, m, 0.0, 0.12));
        assert!(mild(20.0).is_none());
        assert!(mild(23.0).is_some_and(|s| s.slowing));
    }

    #[test]
    fn one_slow_card_is_not_tiredness() {
        let mut s = study(4, 0, 15.0, 999.0, 0.0);
        let last = s.last_mut().unwrap();
        last.think_ms = Some(60_000);
        assert!(due(&s).is_none());
    }

    #[test]
    fn a_rest_starts_the_stretch_over_and_every_fourth_break_is_long() {
        let mut day = vec![];
        let mut start = 0;
        for i in 0..3 {
            let s = study(10 + i, start, 26.0, 999.0, 0.0);
            start = s.last().unwrap().at + 6 * 60;
            day.extend(s);
        }
        let st = replay(&day, 0);
        assert_eq!(st.breaks, 2);
        assert!(st.worked_min < 27.0);
        day.extend(study(20, start, 28.0, 999.0, 0.0));
        let s = suggest(&replay(&day, 0), Rhythm::default()).expect("fourth stretch done");
        assert!(s.long);
        assert_eq!(s.minutes, 15);
    }

    #[test]
    fn short_gaps_and_paused_time_are_not_work() {
        let mut s = study(5, 0, 20.0, 999.0, 0.0);
        // Two minutes behind the Pause cover: no break, no work either.
        let n = s.len() / 2;
        for x in &mut s[n..] {
            x.at += 120;
        }
        let st = replay(&s, 0);
        assert_eq!(st.breaks, 0);
        assert!((st.worked_min - 20.0).abs() < 0.3, "{}", st.worked_min);
        // A hidden tab for 30 s is other work: it counts, but the card's
        // time doesn't measure tiredness.
        let was = s[n].think_ms.unwrap();
        s[n].away_ms = Some(30_000);
        s[n].think_ms = Some(30_000);
        let st2 = replay(&s, 0);
        let more = (30_000.0 + 30_000.0 - was as f64) / 60_000.0;
        assert!((st2.worked_min - st.worked_min - more).abs() < 0.01);
        assert!(st2.slowness.abs() < 0.05);
    }

    #[test]
    fn keep_going_starts_the_watch_over() {
        let s = study(6, 0, 28.0, 999.0, 0.0);
        assert!(due(&s).is_some());
        let watch = s[s.len() - 3].at;
        assert!(suggest(&replay(&s, watch), Rhythm::default()).is_none());
    }

    #[test]
    fn an_hour_away_starts_the_day_over() {
        let mut s = study(7, 0, 20.0, 999.0, 0.0);
        let then = s.last().unwrap().at + 3600;
        s.extend(study(8, then, 10.0, 999.0, 0.0));
        let st = replay(&s, 0);
        assert_eq!(st.breaks, 0);
        assert!(st.worked_min < 11.0);
    }

    #[test]
    fn the_rhythm_learns_and_stays_in_range() {
        let r = Rhythm::default().kept_going();
        assert!((r.work_min - 27.5).abs() < 1e-9 && (r.mult - 1.0 / 1.1).abs() < 1e-9);
        let mut r = Rhythm::default();
        for _ in 0..30 {
            r = r.paused_early();
        }
        assert_eq!(r, Rhythm { work_min: 15.0, mult: 2.5 });
        // Short breaks never go under five minutes before the multiplier.
        assert_eq!(suggest(&State { worked_min: 17.0, ..State::default() }, r).map(|s| s.minutes), Some(13));
        assert_eq!(Rhythm::new(f64::NAN, 99.0), Rhythm { work_min: START_WORK_MIN, mult: 2.5 });
    }

    #[test]
    fn own_pauses_are_told_from_suggested_ones() {
        let mut s = study(9, 0, 15.0, 999.0, 0.0);
        let before = s.last().unwrap().at;
        s.extend(study(9, before + 4 * 60, 0.05, 999.0, 0.0).into_iter().take(1));
        let st = replay(&s, 0);
        assert!(st.rested_after.is_some());
        assert!(paused_on_own(&st, &s, None));
        assert!(!paused_on_own(&st, &s, Some(before + 1)), "the break was suggested");
        // A pause after only five minutes says nothing.
        let mut short = study(9, 0, 5.0, 999.0, 0.0);
        let b = short.last().unwrap().at;
        short.extend(study(9, b + 4 * 60, 0.05, 999.0, 0.0).into_iter().take(1));
        assert!(!paused_on_own(&replay(&short, 0), &short, None));
    }

    #[test]
    fn card_usuals_use_only_earlier_answers() {
        let u = card_usuals(&[(1, Some(1000)), (1, Some(1000)), (2, Some(5)), (1, Some(9999))]);
        assert_eq!(u[..3], [None, None, None]);
        assert!(u[3].is_some_and(|v| (v - 1000f64.ln()).abs() < 1e-9));
    }
}
