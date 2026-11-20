//! Smart breaks: suggest a break when answers show tiredness, and size it
//! by how this user recovered from earlier pauses.
//!
//! Each review answer is compared with the same session when it was fresh:
//! - time: how much slower the recall (`think_ms`) was than expected, in
//!   standard deviations. A card that has been timed before is compared with
//!   its own earlier times, so hard cards don't pass for tiredness.
//! - recall: a miss weighs by how sure FSRS was. Missing a 97% card is
//!   strong evidence, missing a 70% card is almost none.
//!
//! Both are log-likelihood ratios of "tired" against "fresh", so they add.
//! A CUSUM sums them, never below zero, and a break is due once it crosses
//! `THRESHOLD`. Nothing here knows tiredness for sure: the threshold is a
//! choice of how rarely a fresh learner is interrupted for nothing.

/// The first part of a session sets its fresh baseline.
pub const BASELINE_SECS: i64 = 10 * 60;
/// No suggestion before this much of a session has passed.
pub const MIN_SESSION_SECS: i64 = 10 * 60;
/// After this long, a break is suggested whatever the answers say.
pub const CEILING_SECS: i64 = 90 * 60;
/// A pause this long inside a session starts the watch over: it was a break.
pub const RESET_GAP_SECS: i64 = 2 * 60;
/// Timed answers needed in the baseline before time counts at all.
const MIN_BASELINE: usize = 8;
/// Answer times vary at least this much (natural log units).
const SIGMA_FLOOR: f64 = 0.3;
/// The slowdown watched for, in standard deviations; the CUSUM's
/// reference value is half of it.
const SHIFT: f64 = 1.0;
/// The drop in recall watched for: a tired user remembers 15% less.
const RECALL_DROP: f64 = 0.15;
/// The CUSUM's alarm level. In theory h = 5 already alarms only once in
/// ~900 fresh answers, but the baseline is only an estimate from ten
/// minutes: in simulation h = 5 interrupted a third of fresh 45-minute
/// sessions and h = 8 still 8%. h = 10 stays under 2% and still notices a
/// real one-sigma slowdown within about 22 answers (four to five minutes).
const THRESHOLD: f64 = 10.0;
/// One very slow answer (a sneeze) counts as three sigmas at most.
const Z_CLIP: f64 = 3.0;
/// Until there is history, recovery takes about three minutes.
pub const DEFAULT_TAU_MIN: f64 = 3.0;
/// A break lasts until the slowdown is expected to be down to this.
const RECOVERED: f64 = 0.2;
const MIN_BREAK_MIN: f64 = 2.0;
const MAX_BREAK_MIN: f64 = 20.0;
/// Answers on each side of a gap that tell how well it rested.
const SIDE: usize = 8;

/// One answer of a review card on the website, oldest first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Answer {
    pub at: i64,
    pub card: i64,
    /// Front to answer, in ms, without hidden or paused time.
    pub think_ms: Option<i64>,
    pub recalled: bool,
    /// FSRS's recall chance at the moment of the answer.
    pub r: Option<f64>,
}

/// For each answer, the mean log time of its card's earlier answers, once
/// there were at least three.
pub fn card_means(answers: &[Answer]) -> Vec<Option<f64>> {
    let mut seen: std::collections::HashMap<i64, (f64, u32)> = std::collections::HashMap::new();
    answers
        .iter()
        .map(|a| {
            let e = seen.entry(a.card).or_insert((0.0, 0));
            let mean = (e.1 >= 3).then(|| e.0 / f64::from(e.1));
            if let Some(ms) = a.think_ms.filter(|ms| *ms > 0) {
                e.0 += (ms as f64).ln();
                e.1 += 1;
            }
            mean
        })
        .collect()
}

/// How much slower than fresh each answer of a session was, in standard
/// deviations. The baseline is the session's first ten minutes. `None`
/// where there is no time, or not enough baseline to compare with.
pub fn slowdowns(session: &[Answer], means: &[Option<f64>]) -> Vec<Option<f64>> {
    let Some(first) = session.first() else { return vec![] };
    let logs: Vec<Option<f64>> = session.iter().map(|a| a.think_ms.filter(|ms| *ms > 0).map(|ms| (ms as f64).ln())).collect();
    let in_base = |i: usize| session[i].at < first.at + BASELINE_SECS;
    // Cards timed before are compared with themselves, shifted by how this
    // session ran against their history; the others with the session.
    let base_known: Vec<f64> = (0..session.len()).filter(|&i| in_base(i)).filter_map(|i| Some(logs[i]? - means[i]?)).collect();
    let base_new: Vec<f64> = (0..session.len()).filter(|&i| in_base(i) && means[i].is_none()).filter_map(|i| logs[i]).collect();
    let offset = (base_known.len() >= MIN_BASELINE / 2).then(|| mean(&base_known));
    let level = (base_new.len() >= MIN_BASELINE / 2).then(|| mean(&base_new));
    let expected = |i: usize| match means[i] {
        Some(m) => offset.map(|o| m + o),
        None => level,
    };
    let residuals: Vec<f64> = (0..session.len()).filter(|&i| in_base(i)).filter_map(|i| Some(logs[i]? - expected(i)?)).collect();
    if residuals.len() < MIN_BASELINE {
        return vec![None; session.len()];
    }
    // The median deviation, so a few odd answers in the baseline don't
    // stretch it.
    let sigma = (1.4826 * mad(&residuals)).max(SIGMA_FLOOR);
    (0..session.len()).map(|i| Some(((logs[i]? - expected(i)?) / sigma).clamp(-Z_CLIP, Z_CLIP))).collect()
}

/// The evidence one answer adds for "tired" over "fresh".
fn evidence(a: &Answer, slowdown: Option<f64>) -> f64 {
    let time = slowdown.map_or(0.0, |z| SHIFT * (z - SHIFT / 2.0));
    let recall = a.r.map_or(0.0, |r| {
        let r = r.clamp(0.01, 0.99);
        if a.recalled {
            (1.0 - RECALL_DROP).ln()
        } else {
            ((1.0 - r * (1.0 - RECALL_DROP)) / (1.0 - r)).ln()
        }
    });
    time + recall
}

/// Is a break due? `session` holds this session's answers, `means` their
/// card means, `watch_from` when the watch last started over (a skipped
/// break). Returns how much slower than fresh the last answers were, which
/// sets the break's length.
pub fn tired(session: &[Answer], means: &[Option<f64>], watch_from: i64, now: i64) -> Option<f64> {
    let start = session.first()?.at;
    if now - start < MIN_SESSION_SECS {
        return None;
    }
    let z = slowdowns(session, means);
    let recent = recent_slowdown(&z, session.len());
    if now - start >= CEILING_SECS {
        return Some(recent.unwrap_or(1.0));
    }
    let mut cusum = 0.0_f64;
    let mut prev = start;
    let mut alarm = false;
    for (i, a) in session.iter().enumerate() {
        if a.at - prev >= RESET_GAP_SECS {
            cusum = 0.0;
            alarm = false;
        }
        prev = a.at;
        if a.at < start + BASELINE_SECS || a.at < watch_from {
            continue;
        }
        cusum = (cusum + evidence(a, z[i])).max(0.0);
        alarm |= cusum >= THRESHOLD;
    }
    alarm.then(|| recent.unwrap_or(1.0))
}

/// Mean slowdown of the last few timed answers before `end`.
fn recent_slowdown(z: &[Option<f64>], end: usize) -> Option<f64> {
    let last: Vec<f64> = z[..end].iter().rev().filter_map(|v| *v).take(SIDE).collect();
    (!last.is_empty()).then(|| mean(&last))
}

/// Mean slowdown of the first few timed answers from `start` on.
fn early_slowdown(z: &[Option<f64>], start: usize) -> Option<f64> {
    let first: Vec<f64> = z[start..].iter().filter_map(|v| *v).take(SIDE).collect();
    (first.len() == SIDE).then(|| mean(&first))
}

/// How fast this user recovers, in minutes: slowdown after a pause of g
/// minutes is taken to fall as e^(-g/tau). Learned from pauses of 2 to 60
/// minutes in `history` that came after a real slowdown. `None` with fewer
/// than three.
pub fn recovery_minutes(history: &[Answer], means: &[Option<f64>]) -> Option<f64> {
    let mut taus = vec![];
    // Runs of study with no pause over an hour share one baseline.
    let mut s = 0;
    while s < history.len() {
        let mut e = s + 1;
        while e < history.len() && history[e].at - history[e - 1].at < 3600 {
            e += 1;
        }
        let z = slowdowns(&history[s..e], &means[s..e]);
        for i in 1..(e - s) {
            let gap = history[s + i].at - history[s + i - 1].at;
            if gap < RESET_GAP_SECS {
                continue;
            }
            let (Some(before), Some(after)) = (
                recent_slowdown(&z, i).filter(|_| z[..i].iter().flatten().count() >= SIDE),
                early_slowdown(&z, i),
            ) else {
                continue;
            };
            if before < 0.5 {
                continue;
            }
            let rested = (1.0 - after / before).clamp(0.05, 0.95);
            taus.push((gap as f64 / 60.0 / -(1.0 - rested).ln()).clamp(0.5, 30.0));
        }
        s = e;
    }
    (taus.len() >= 3).then(|| median(&mut taus))
}

/// Minutes of break: long enough for `slowdown` to fall to `RECOVERED`.
pub fn break_minutes(slowdown: f64, tau: f64) -> i64 {
    let m = tau * (slowdown.max(2.0 * RECOVERED) / RECOVERED).ln();
    m.clamp(MIN_BREAK_MIN, MAX_BREAK_MIN).round() as i64
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// Median absolute deviation from the median.
fn mad(v: &[f64]) -> f64 {
    let mut c = v.to_vec();
    let m = median(&mut c);
    let mut d: Vec<f64> = v.iter().map(|x| (x - m).abs()).collect();
    median(&mut d)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
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

    /// A session of one answer every 12 s for `minutes`. From `tired_after`
    /// minutes on, answers slow by `slow` (log units) and recall drops.
    fn session(seed: u64, minutes: i64, tired_after: i64, slow: f64, drop: f64) -> Vec<Answer> {
        let mut n = Noise(seed);
        let mut out = vec![];
        let mut t = 0;
        let mut card = 0;
        while t < minutes * 60 {
            card += 1;
            let tired = t >= tired_after * 60;
            let r = 0.9;
            let ms = (2000.0 * (0.35 * n.normal() + if tired { slow } else { 0.0 }).exp()) as i64;
            let p = if tired { r * (1.0 - drop) } else { r };
            out.push(Answer { at: t, card, think_ms: Some(ms), recalled: n.next() < p, r: Some(r) });
            t += 12;
        }
        out
    }

    fn no_means(s: &[Answer]) -> Vec<Option<f64>> {
        vec![None; s.len()]
    }

    #[test]
    fn a_fresh_session_is_left_alone() {
        // 100 fresh sessions of 45 minutes: at most two wrong suggestions.
        let wrong = (0..100)
            .filter(|&seed| {
                let s = session(seed, 45, 999, 0.0, 0.0);
                tired(&s, &no_means(&s), 0, 45 * 60).is_some()
            })
            .count();
        assert!(wrong <= 2, "{wrong} fresh sessions interrupted");
    }

    #[test]
    fn slower_answers_bring_a_break() {
        let s = session(1, 40, 20, 0.5, 0.0);
        let slowdown = tired(&s, &no_means(&s), 0, 40 * 60).expect("tired by minute 40");
        assert!(slowdown > 0.8, "{slowdown}");
        // Not before the slowdown began, and soon after it.
        let upto = |m: i64| s.iter().filter(|a| a.at < m * 60).copied().collect::<Vec<_>>();
        assert!(tired(&upto(20), &no_means(&s), 0, 20 * 60).is_none());
        assert!(tired(&upto(25), &no_means(&s), 0, 25 * 60).is_some());
    }

    #[test]
    fn misses_alone_bring_a_break_too() {
        // No times at all, as without JavaScript: recall drops by 40%.
        let mut s = session(2, 40, 15, 0.0, 0.4);
        for a in &mut s {
            a.think_ms = None;
        }
        assert!(tired(&s, &no_means(&s), 0, 40 * 60).is_some());
    }

    #[test]
    fn never_in_the_first_ten_minutes_and_always_after_ninety() {
        let s = session(3, 10, 0, 2.0, 0.9);
        assert!(tired(&s[..s.len() - 1], &no_means(&s), 0, 9 * 60).is_none());
        let s = session(4, 91, 999, 0.0, 0.0);
        assert!(tired(&s, &no_means(&s), 0, 91 * 60).is_some());
    }

    #[test]
    fn skipping_a_break_starts_the_watch_over() {
        let s = session(5, 40, 15, 0.6, 0.0);
        assert!(tired(&s, &no_means(&s), 0, 40 * 60).is_some());
        assert!(tired(&s, &no_means(&s), 39 * 60, 40 * 60).is_none());
    }

    #[test]
    fn hard_cards_are_compared_with_themselves() {
        // Every card is slow, but no slower than it always is.
        let mut s = session(6, 40, 999, 0.0, 0.0);
        let means: Vec<Option<f64>> = s
            .iter_mut()
            .enumerate()
            .map(|(i, a)| {
                let hard = if i > 100 { 1.5 } else { 0.0 };
                a.think_ms = a.think_ms.map(|ms| (ms as f64 * f64::exp(hard)) as i64);
                Some(2000f64.ln() + hard)
            })
            .collect();
        assert!(tired(&s, &means, 0, 40 * 60).is_none());
        assert!(tired(&s, &no_means(&s), 0, 40 * 60).is_some(), "without its history it looks like tiredness");
    }

    #[test]
    fn card_means_use_only_earlier_answers() {
        let a = |card, ms| Answer { at: 0, card, think_ms: Some(ms), recalled: true, r: None };
        let m = card_means(&[a(1, 1000), a(1, 1000), a(2, 5), a(1, 1000), a(1, 9999)]);
        assert_eq!(m[..3], [None, None, None]);
        assert!(m[4].is_some_and(|v| (v - 1000f64.ln()).abs() < 1e-9));
    }

    #[test]
    fn recovery_is_learned_from_pauses() {
        // Three days: slow by the end, a 6-minute pause, back to half the
        // slowdown. e^(-6/tau) = 0.5 gives tau = 8.7 minutes.
        let mut history = vec![];
        for day in 0..3 {
            let base = day * 86_400;
            let mut s = session(10 + day as u64, 30, 20, 1.0, 0.0);
            let mut after = session(20 + day as u64, 10, 0, 0.5, 0.0);
            for a in &mut s {
                a.at += base;
                a.card += 1000 * day;
            }
            for a in &mut after {
                a.at += base + 36 * 60;
                a.card += 1000 * day + 500;
            }
            history.extend(s);
            history.extend(after);
        }
        let tau = recovery_minutes(&history, &no_means(&history)).expect("three pauses");
        assert!((5.0..14.0).contains(&tau), "{tau}");
        assert_eq!(recovery_minutes(&history[..200], &no_means(&history[..200])), None);
    }

    #[test]
    fn a_bigger_slowdown_means_a_longer_break() {
        assert_eq!(break_minutes(1.0, DEFAULT_TAU_MIN), 5);
        assert!(break_minutes(2.0, DEFAULT_TAU_MIN) > 5);
        assert_eq!(break_minutes(0.1, DEFAULT_TAU_MIN), 2);
        assert_eq!(break_minutes(50.0, 30.0), 20);
    }
}
