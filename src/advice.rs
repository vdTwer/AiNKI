//! The coach's advice: what is worth doing about your studying right now.
//!
//! Static rules over a snapshot of numbers (`Signals`), checked in a fixed
//! order. No model, no randomness: the same numbers give the same notes.
//! Every rule needs enough data before it speaks, and works on long
//! windows (two weeks, a month) so a note doesn't come and go from one day
//! to the next. Each note says what the coach saw, with the real numbers,
//! and one thing to do. When nothing is worth saying, it says nothing.
//!
//! The card coach (`coach`) looks at single cards; this looks at the whole.

use std::collections::{HashMap, HashSet};

use chrono::{Duration, Timelike};
use serde::Serialize;

use crate::{
    App,
    auth::Access,
    coach::{Attention, Kind},
    error::Result,
    service::{DeckSummary, Stats},
};

/// Recall figures need this many reviews of learned cards.
pub const MIN_RETENTION_SAMPLE: i64 = 50;
/// A deck's recall needs this many.
const MIN_DECK_SAMPLE: i64 = 30;
/// "Your usual" pace needs this many study days in the last fourteen.
pub const MIN_PACE_DAYS: i64 = 7;
/// Each new card brings about this many reviews in its first month.
const REVIEWS_PER_NEW_CARD: f64 = 10.0;

/// Every rule the coach knows, in the order it checks them. The first that
/// fires is the note on the home page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    ExamClose,
    PileUp,
    CaughtUp,
    Late,
    Leeches,
    DeckLags,
    BelowTarget,
    Sliding,
    TooSure,
    WaveComing,
    TooFast,
    HeavyDay,
    BackAfterGap,
    RunAtRisk,
    FewDays,
    LongSittings,
    SlowAnswers,
    NeverFitted,
    AboveTarget,
    OddTarget,
    TightLimit,
    ForgottenDeck,
}

impl Rule {
    pub const ALL: [Rule; 22] = [
        Rule::ExamClose,
        Rule::PileUp,
        Rule::CaughtUp,
        Rule::Late,
        Rule::Leeches,
        Rule::DeckLags,
        Rule::BelowTarget,
        Rule::Sliding,
        Rule::TooSure,
        Rule::WaveComing,
        Rule::TooFast,
        Rule::HeavyDay,
        Rule::BackAfterGap,
        Rule::RunAtRisk,
        Rule::FewDays,
        Rule::LongSittings,
        Rule::SlowAnswers,
        Rule::NeverFitted,
        Rule::AboveTarget,
        Rule::OddTarget,
        Rule::TightLimit,
        Rule::ForgottenDeck,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Rule::ExamClose => "exam_close",
            Rule::PileUp => "pile_up",
            Rule::CaughtUp => "caught_up",
            Rule::Late => "late",
            Rule::Leeches => "leeches",
            Rule::DeckLags => "deck_lags",
            Rule::BelowTarget => "below_target",
            Rule::Sliding => "sliding",
            Rule::TooSure => "too_sure",
            Rule::WaveComing => "wave_coming",
            Rule::TooFast => "too_fast",
            Rule::HeavyDay => "heavy_day",
            Rule::BackAfterGap => "back_after_gap",
            Rule::RunAtRisk => "run_at_risk",
            Rule::FewDays => "few_days",
            Rule::LongSittings => "long_sittings",
            Rule::SlowAnswers => "slow_answers",
            Rule::NeverFitted => "never_fitted",
            Rule::AboveTarget => "above_target",
            Rule::OddTarget => "odd_target",
            Rule::TightLimit => "tight_limit",
            Rule::ForgottenDeck => "forgotten_deck",
        }
    }

    pub fn parse(s: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// Recall of learned cards: how many were remembered, out of how many.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Recall {
    pub reviews: i64,
    pub remembered: i64,
}

impl Recall {
    pub fn rate(self) -> Option<f64> {
        (self.reviews > 0).then(|| self.remembered as f64 / self.reviews as f64)
    }

    /// The rate, once there are at least `min` reviews.
    fn rate_over(self, min: i64) -> Option<f64> {
        if self.reviews >= min { self.rate() } else { None }
    }
}

/// One deck as the coach sees it.
#[derive(Debug, Clone, Default)]
pub struct DeckSignal {
    pub id: i64,
    pub name: String,
    pub preset_id: i64,
    /// Recall of learned cards, last 30 days, and what its preset aims for.
    pub recall: Recall,
    pub target: f64,
    /// Review and learning cards due now.
    pub due: i64,
    /// Cards never studied (not suspended).
    pub new_left: i64,
    /// New cards started in the last seven days.
    pub new_started_7d: i64,
    /// Days until its exam, when it has one (0 is today).
    pub exam_in_days: Option<i64>,
    /// Days since any card of it was reviewed; `None` when never.
    pub days_since_review: Option<i64>,
    /// Cards in it the card coach flagged.
    pub flagged: i64,
}

/// One preset as the coach sees it.
#[derive(Debug, Clone, Default)]
pub struct PresetSignal {
    pub id: i64,
    pub name: String,
    pub desired_retention: f64,
    pub new_per_day: i64,
    pub reviews_per_day: i64,
    /// Whether FSRS was ever fitted to this user's reviews (the preset's own
    /// parameters or the user's).
    pub fitted: bool,
    /// Whether one of its decks has an exam date.
    pub has_exam: bool,
    /// Whether any deck uses it.
    pub used: bool,
}

/// Everything the rules look at, gathered once.
#[derive(Debug, Clone, Default)]
pub struct Signals {
    /// The hour on the user's clock, 0 to 23.
    pub hour: u32,
    /// Quiet hours are on right now.
    pub quiet: bool,

    // Today.
    /// Everything studying would show today: learning, reviews, new.
    pub due_today: i64,
    /// Review cards due today beyond the review limits.
    pub held_back: i64,
    pub minutes_due: i64,
    pub reviews_today: i64,

    // Backlog.
    pub overdue: i64,
    pub catch_up: bool,

    // Pace.
    /// Median reviews on a study day, last fourteen days.
    pub usual_per_day: f64,
    /// Days with reviews in the last fourteen.
    pub study_days_14: i64,
    pub new_started_7d: i64,
    /// Median session length last week, in minutes.
    pub session_minutes: Option<f64>,
    /// Median thinking time last week, in seconds.
    pub answer_seconds: Option<f64>,
    pub breaks_on: bool,

    // Habit.
    /// Days since the last review (0 is today); `None` when never.
    pub days_since_review: Option<i64>,
    pub days_studied_7: i64,
    pub reviews_7: i64,
    /// Days in a row up to yesterday (or today, if studied).
    pub streak: i64,

    // Ahead.
    /// Cards falling due on each of the next seven days (tomorrow first).
    pub next_days: Vec<i64>,

    // Memory.
    pub recall_30d: Recall,
    /// Desired retention over the reviews above.
    pub target: f64,
    pub recall_recent: Recall,
    pub recall_before: Recall,
    /// When you felt sure: only with the sure check on.
    pub sure: Option<Recall>,

    // Cards.
    pub leeches: i64,
    pub other_flags: i64,

    // Setup.
    pub optimizable_reviews: i64,
    pub ever_optimized: bool,
    pub auto_optimize: bool,
    pub decks: Vec<DeckSignal>,
    pub presets: Vec<PresetSignal>,
}

impl Signals {
    /// The usual daily pace, once there are enough study days.
    fn usual(&self) -> Option<f64> {
        (self.study_days_14 >= MIN_PACE_DAYS && self.usual_per_day > 0.0).then_some(self.usual_per_day)
    }

    fn new_per_day_7d(&self) -> f64 {
        self.new_started_7d as f64 / 7.0
    }

    /// The new-card limit of the presets in use, summed over decks.
    fn new_limit(&self) -> i64 {
        self.decks
            .iter()
            .filter(|d| d.new_left > 0)
            .filter_map(|d| self.presets.iter().find(|p| p.id == d.preset_id))
            .map(|p| p.new_per_day)
            .sum()
    }

    fn deck_preset(&self, d: &DeckSignal) -> Option<&PresetSignal> {
        self.presets.iter().find(|p| p.id == d.preset_id)
    }
}

/// Where a note points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub href: String,
    pub label: String,
}

/// One note from the coach.
#[derive(Debug, Clone, Serialize)]
pub struct Advice {
    pub rule: Rule,
    /// What the coach saw and what to do, in a sentence or two.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<Link>,
    /// The note's link is a form that turns catch-up mode on or off.
    #[serde(skip)]
    pub catch_up_toggle: Option<bool>,
}

fn link(href: impl Into<String>, label: &str) -> Option<Link> {
    Some(Link { href: href.into(), label: label.into() })
}

fn note(rule: Rule, text: String, link: Option<Link>) -> Advice {
    Advice { rule, text, link, catch_up_toggle: None }
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", crate::format::number(n), if n == 1 { one } else { many })
}

fn cards(n: i64) -> String {
    plural(n, "card", "cards")
}

fn days(n: i64) -> String {
    plural(n, "day", "days")
}

/// Everything the coach would say, most important first.
pub fn advise(s: &Signals) -> Vec<Advice> {
    let mut out = Vec::new();
    for rule in Rule::ALL {
        if let Some(a) = check(rule, s) {
            out.push(a);
        }
    }
    out
}

/// Whether one rule fires, and what it says.
pub fn check(rule: Rule, s: &Signals) -> Option<Advice> {
    match rule {
        Rule::ExamClose => exam_close(s),
        Rule::PileUp => pile_up(s),
        Rule::CaughtUp => caught_up(s),
        Rule::Late => late(s),
        Rule::Leeches => leeches(s),
        Rule::DeckLags => deck_lags(s),
        Rule::BelowTarget => below_target(s),
        Rule::Sliding => sliding(s),
        Rule::TooSure => too_sure(s),
        Rule::WaveComing => wave_coming(s),
        Rule::TooFast => too_fast(s),
        Rule::HeavyDay => heavy_day(s),
        Rule::BackAfterGap => back_after_gap(s),
        Rule::RunAtRisk => run_at_risk(s),
        Rule::FewDays => few_days(s),
        Rule::LongSittings => long_sittings(s),
        Rule::SlowAnswers => slow_answers(s),
        Rule::NeverFitted => never_fitted(s),
        Rule::AboveTarget => above_target(s),
        Rule::OddTarget => odd_target(s),
        Rule::TightLimit => tight_limit(s),
        Rule::ForgottenDeck => forgotten_deck(s),
    }
}

// ----- right now ------------------------------------------------------------

/// An exam within three weeks, with new cards left. The nearest one counts.
fn exam_close(s: &Signals) -> Option<Advice> {
    let d = s
        .decks
        .iter()
        .filter(|d| d.new_left > 0 && d.exam_in_days.is_some_and(|n| (0..=21).contains(&n)))
        .min_by_key(|d| (d.exam_in_days, d.id))?;
    let left = d.exam_in_days.unwrap_or(0);
    let when = match left {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        n => format!("in {n} days"),
    };
    let href = format!("/decks/{}", d.id);
    if left <= 3 {
        return Some(note(
            Rule::ExamClose,
            format!(
                "Your {} exam is {when}. Stop new cards now and review what you already know.",
                d.name
            ),
            link(href, "Open the deck"),
        ));
    }
    let pace = s.deck_preset(d).map(|p| p.new_per_day).unwrap_or(0);
    // Two days before the exam are for reviewing only.
    let need = (d.new_left as f64 / (left - 2).max(1) as f64).ceil() as i64;
    if need <= pace {
        return None;
    }
    Some(note(
        Rule::ExamClose,
        format!(
            "Your {} exam is {when} and {} are still new. That takes {need} new cards a day; the preset allows {pace}.",
            d.name,
            cards(d.new_left)
        ),
        link(href, "Open the deck"),
    ))
}

/// Many overdue cards, more than two days of work.
fn pile_up(s: &Signals) -> Option<Advice> {
    if s.catch_up || s.overdue <= 50 {
        return None;
    }
    let pace = s.usual().unwrap_or(100.0).max(20.0);
    if (s.overdue as f64) <= 2.0 * pace {
        return None;
    }
    let spread = (s.overdue as f64 / pace).ceil() as i64;
    Some(Advice {
        catch_up_toggle: Some(true),
        ..note(
            Rule::PileUp,
            format!(
                "{} piled up. Catch up over about {}, the ones you most likely still know first.",
                cards(s.overdue),
                days(spread)
            ),
            link("/learn/backlog", "Catch up"),
        )
    })
}

fn caught_up(s: &Signals) -> Option<Advice> {
    (s.catch_up && s.overdue < 20).then(|| Advice {
        catch_up_toggle: Some(false),
        ..note(
            Rule::CaughtUp,
            "You caught up. Turn catch-up mode off to get new cards again.".into(),
            link("/settings", "Turn it off"),
        )
    })
}

/// Late at night with a real session ahead.
fn late(s: &Signals) -> Option<Advice> {
    let late = s.hour >= 23 || s.hour < 4;
    (late && !s.quiet && s.minutes_due > 15).then(|| {
        note(
            Rule::Late,
            "It's late. Sleep helps today's cards settle. Do a short round now and the rest in the morning.".into(),
            link("/learn/sleep", "Why"),
        )
    })
}

// ----- cards that cost time -----------------------------------------------

fn leeches(s: &Signals) -> Option<Advice> {
    let all = s.leeches + s.other_flags;
    if s.leeches == 0 && s.other_flags < 5 {
        return None;
    }
    let text = if s.leeches > 0 {
        format!(
            "{} {} forgotten again and again. Fixing them saves more time than reviewing them.",
            cards(s.leeches),
            if s.leeches == 1 { "is" } else { "are" }
        )
    } else {
        format!("{} ask a lot at once or look like another card. A quick fix saves reviews later.", cards(all))
    };
    Some(note(Rule::Leeches, text, link("/attention", "Have a look")))
}

// ----- memory ---------------------------------------------------------------

/// The likely reason recall is short, as a sentence, and where to act.
fn cause(s: &Signals, deck: Option<&DeckSignal>) -> (String, Option<Link>) {
    // When the leech note already speaks, flagged cards aren't news here.
    let leech_note = s.leeches > 0 || s.other_flags >= 5;
    if let Some(d) = deck.filter(|d| d.flagged > 0 && !leech_note) {
        return (
            format!("{} in it keep slipping or ask a lot at once.", cards(d.flagged)),
            link("/attention", "Have a look"),
        );
    }
    let (new_7d, limit) = match deck {
        Some(d) => (d.new_started_7d as f64, s.deck_preset(d).map(|p| p.new_per_day).unwrap_or(0)),
        None => (s.new_started_7d as f64, s.new_limit()),
    };
    if limit > 0 && new_7d / 7.0 >= limit as f64 * 0.8 {
        return (
            "Many new cards came in this week. Fewer new cards for a while usually brings it back.".into(),
            link(deck.map_or("/presets".into(), |d| format!("/decks/{}", d.id)), "Change new cards"),
        );
    }
    let fitted = match deck {
        Some(d) => s.deck_preset(d).is_some_and(|p| p.fitted),
        None => s.ever_optimized,
    };
    if !fitted && s.optimizable_reviews >= 1000 {
        return (
            "FSRS still uses its defaults. Fitting it to your reviews often helps.".into(),
            link("/settings#fsrs", "Optimize"),
        );
    }
    ("Cards that ask less are easier to keep.".into(), link("/learn/desired-retention", "Why"))
}

/// One deck clearly under its target while the rest is fine.
fn deck_lags(s: &Signals) -> Option<Advice> {
    let short = |d: &&DeckSignal| d.recall.rate_over(MIN_DECK_SAMPLE).is_some_and(|r| r <= d.target - 0.07);
    let lagging: Vec<&DeckSignal> = s.decks.iter().filter(short).collect();
    // Several decks short is the whole picture; `below_target` speaks then.
    if lagging.len() != 1 {
        return None;
    }
    let d = lagging[0];
    let rest = Recall {
        reviews: s.recall_30d.reviews - d.recall.reviews,
        remembered: s.recall_30d.remembered - d.recall.remembered,
    };
    // Only when the rest is known and fine; with one deck, or all short,
    // `below_target` speaks instead.
    if rest.rate_over(MIN_DECK_SAMPLE).is_none_or(|r| r < s.target - 0.05) {
        return None;
    }
    let (why, to) = cause(s, Some(d));
    Some(note(
        Rule::DeckLags,
        format!(
            "{}: you remembered {}, you aim for {}. {why}",
            d.name,
            pct(d.recall.rate().unwrap_or(0.0)),
            pct(d.target)
        ),
        to,
    ))
}

fn below_target(s: &Signals) -> Option<Advice> {
    let r = s.recall_30d.rate_over(MIN_RETENTION_SAMPLE)?;
    if r > s.target - 0.05 {
        return None;
    }
    let (why, to) = cause(s, None);
    Some(note(
        Rule::BelowTarget,
        format!("You remembered {} of your cards; you aim for {}. {why}", pct(r), pct(s.target)),
        to,
    ))
}

/// The last two weeks clearly worse than the four before.
fn sliding(s: &Signals) -> Option<Advice> {
    let now = s.recall_recent.rate_over(40)?;
    let before = s.recall_before.rate_over(40)?;
    if now > before - 0.06 {
        return None;
    }
    let why = if s.new_started_7d as f64 / 7.0 >= 1.0 && s.new_limit() > 0 {
        " Many new cards came in lately; a few calmer days usually bring it back."
    } else if s.days_studied_7 <= 3 {
        " You studied on fewer days; short daily rounds bring it back."
    } else {
        ""
    };
    Some(note(
        Rule::Sliding,
        format!("Your recall dropped from {} to {} in two weeks.{why}", pct(before), pct(now)),
        link("/stats", "See your stats"),
    ))
}

fn too_sure(s: &Signals) -> Option<Advice> {
    let sure = s.sure?;
    let r = sure.rate_over(50)?;
    if r >= 0.85 {
        return None;
    }
    let one_in = (1.0 / (1.0 - r)).round() as i64;
    Some(note(
        Rule::TooSure,
        format!(
            "You forget 1 in {one_in} cards you feel sure about. Say the answer out loud before you turn the card."
        ),
        link("/learn/illusions", "Why"),
    ))
}

// ----- load ahead -------------------------------------------------------------

fn wave_coming(s: &Signals) -> Option<Advice> {
    let usual = s.usual()?;
    if s.new_started_7d == 0 || s.next_days.is_empty() {
        return None;
    }
    let ahead = s.next_days.iter().sum::<i64>() as f64 / s.next_days.len() as f64;
    if ahead < usual * 1.3 || ahead - usual < 10.0 {
        return None;
    }
    Some(note(
        Rule::WaveComing,
        format!(
            "The next week brings about {} reviews a day, {} more than now. Pause new cards for a few days.",
            ahead.round() as i64,
            (ahead - usual).round() as i64
        ),
        link("/presets", "Change new cards"),
    ))
}

fn too_fast(s: &Signals) -> Option<Advice> {
    let usual = s.usual()?;
    let new = s.new_per_day_7d();
    if new * REVIEWS_PER_NEW_CARD <= usual * 1.5 || new < 3.0 {
        return None;
    }
    Some(note(
        Rule::TooFast,
        format!(
            "You start about {} new cards a day. Each brings about ten reviews in its first month, so your reviews will grow toward {} a day.",
            new.round() as i64,
            (new * REVIEWS_PER_NEW_CARD).round() as i64
        ),
        link("/learn/backlog", "Why"),
    ))
}

fn heavy_day(s: &Signals) -> Option<Advice> {
    let usual = s.usual()?;
    (s.due_today as f64 >= usual * 1.5 && s.minutes_due > 30).then(|| {
        note(
            Rule::HeavyDay,
            format!(
                "Today is heavier than usual: {} against about {}. Do the reviews first; new cards can wait until tomorrow.",
                cards(s.due_today),
                usual.round() as i64
            ),
            None,
        )
    })
}

// ----- habit ---------------------------------------------------------------

fn back_after_gap(s: &Signals) -> Option<Advice> {
    let gap = s.days_since_review?;
    (gap >= 3 && s.due_today > 0).then(|| {
        note(
            Rule::BackAfterGap,
            format!(
                "Welcome back. {} waited for you, about {}. The ones you most likely still know come first.",
                cards(s.due_today),
                plural(s.minutes_due.max(1), "minute", "minutes")
            ),
            None,
        )
    })
}

fn run_at_risk(s: &Signals) -> Option<Advice> {
    let fires = s.streak >= 3 && s.reviews_today == 0 && s.hour >= 18 && s.due_today > 0 && !s.quiet;
    fires.then(|| {
        note(
            Rule::RunAtRisk,
            format!("Ten minutes keeps your {}-day run going.", s.streak),
            None,
        )
    })
}

fn few_days(s: &Signals) -> Option<Advice> {
    (s.days_studied_7 > 0 && s.days_studied_7 <= 3 && s.reviews_7 > 20).then(|| {
        note(
            Rule::FewDays,
            format!(
                "You studied on {} of the last seven. A little every day works better than a lot on a few days.",
                days(s.days_studied_7)
            ),
            link("/learn/spacing", "Why"),
        )
    })
}

fn long_sittings(s: &Signals) -> Option<Advice> {
    let m = s.session_minutes?;
    (!s.breaks_on && m > 45.0).then(|| {
        note(
            Rule::LongSittings,
            format!(
                "Your sessions ran about {} minutes last week. A short break every half hour keeps you sharp.",
                m.round() as i64
            ),
            link("/settings#study", "Suggest breaks"),
        )
    })
}

fn slow_answers(s: &Signals) -> Option<Advice> {
    let secs = s.answer_seconds?;
    (secs > 20.0 && s.reviews_7 >= 50).then(|| {
        note(
            Rule::SlowAnswers,
            format!(
                "A card takes you about {} seconds. Cards that ask for less are quicker and stick better.",
                secs.round() as i64
            ),
            link("/learn/good-cards", "Why"),
        )
    })
}

// ----- setup ---------------------------------------------------------------

fn never_fitted(s: &Signals) -> Option<Advice> {
    (s.optimizable_reviews >= 1000 && !s.ever_optimized && !s.auto_optimize).then(|| {
        note(
            Rule::NeverFitted,
            format!(
                "You have {} reviews, enough for FSRS to learn how you remember.",
                crate::format::number(s.optimizable_reviews)
            ),
            link("/settings#fsrs", "Optimize"),
        )
    })
}

fn above_target(s: &Signals) -> Option<Advice> {
    let r = s.recall_30d.rate_over(200)?;
    (r >= s.target + 0.04).then(|| {
        note(
            Rule::AboveTarget,
            format!(
                "You remembered {}; you aim for {}. Optimizing lets FSRS space your cards further apart.",
                pct(r),
                pct(s.target)
            ),
            link("/settings#fsrs", "Optimize"),
        )
    })
}

fn odd_target(s: &Signals) -> Option<Advice> {
    let p = s
        .presets
        .iter()
        .filter(|p| p.used)
        .find(|p| (p.desired_retention > 0.95 && !p.has_exam) || p.desired_retention < 0.80)?;
    let text = if p.desired_retention > 0.95 {
        format!(
            "{} aims for {}. Above 95% the reviews grow steeply, about twice those of 90%. Worth it before an exam, rarely otherwise.",
            p.name,
            pct(p.desired_retention)
        )
    } else {
        format!(
            "{} aims for {}. Below 80% you forget so much that relearning costs more than it saves.",
            p.name,
            pct(p.desired_retention)
        )
    };
    Some(note(Rule::OddTarget, text, link(format!("/presets/{}", p.id), "Change it")))
}

fn tight_limit(s: &Signals) -> Option<Advice> {
    if s.held_back >= 20 {
        return Some(note(
            Rule::TightLimit,
            format!(
                "Your review limit held back {} today. They pile up and get harder the longer they wait.",
                cards(s.held_back)
            ),
            link("/presets", "Raise the limit"),
        ));
    }
    let p = s
        .presets
        .iter()
        .filter(|p| p.used && p.new_per_day > 0)
        .find(|p| (p.reviews_per_day as f64) < p.new_per_day as f64 * REVIEWS_PER_NEW_CARD)?;
    Some(note(
        Rule::TightLimit,
        format!(
            "{} allows {} new cards but only {} reviews a day. Each new card brings about ten reviews, so the limit will hold cards back.",
            p.name, p.new_per_day, p.reviews_per_day
        ),
        link(format!("/presets/{}", p.id), "Change it"),
    ))
}

/// A deck with cards due that you left alone while studying others.
fn forgotten_deck(s: &Signals) -> Option<Advice> {
    if s.days_since_review.is_none_or(|d| d > 2) {
        return None;
    }
    let d = s
        .decks
        .iter()
        .filter(|d| d.due >= 10 && d.days_since_review.is_some_and(|n| n >= 14))
        .max_by_key(|d| (d.due, std::cmp::Reverse(d.id)))?;
    let weeks = d.days_since_review.unwrap_or(14) / 7;
    Some(note(
        Rule::ForgottenDeck,
        format!(
            "{} waited {}; {} are due.",
            d.name,
            plural(weeks, "week", "weeks"),
            crate::format::number(d.due)
        ),
        link(format!("/decks/{}", d.id), "Open it"),
    ))
}

// ----- gathering the signals -----------------------------------------------

/// Answer times count up to this long; a card left open says nothing.
const MAX_ANSWER_MS: i64 = 60_000;
/// A pause this long between answers ends a session.
const SESSION_GAP_SECS: i64 = 10 * 60;

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 })
}

/// Sessions from answer times (oldest first): each one's length in
/// minutes. A pause of ten minutes or more starts a new session.
pub fn session_lengths(answers: &[(i64, i64)]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut start: Option<(i64, i64)> = None;
    for &(at, ms) in answers {
        let end = at + ms.clamp(0, MAX_ANSWER_MS) / 1000;
        start = match start {
            Some((first, last)) if at - last < SESSION_GAP_SECS => Some((first, end)),
            Some((first, last)) => {
                out.push((last - first) as f64 / 60.0);
                Some((at, end))
            }
            None => Some((at, end)),
        };
    }
    if let Some((first, last)) = start {
        out.push((last - first) as f64 / 60.0);
    }
    out
}

/// What the home page has already looked up, so it isn't fetched twice.
pub struct Known<'a> {
    pub decks: &'a [DeckSummary],
    pub stats: &'a Stats,
    pub attention: &'a [Attention],
    pub overdue: i64,
}

impl App {
    /// Everything the coach would say, most important first.
    pub async fn advice(&self, access: &Access) -> Result<Vec<Advice>> {
        let decks = self.deck_summaries(access).await?;
        let stats = self.stats(access, None).await?;
        let attention = self.attention(access, None).await?;
        let overdue = self.overdue(access).await?;
        let known = Known { decks: &decks, stats: &stats, attention: &attention, overdue };
        Ok(advise(&self.signals(access, &known).await?))
    }

    /// The same, from what the caller already has.
    pub async fn advice_from(&self, access: &Access, known: &Known<'_>) -> Result<Vec<Advice>> {
        Ok(advise(&self.signals(access, known).await?))
    }

    pub async fn signals(&self, access: &Access, known: &Known<'_>) -> Result<Signals> {
        let user = &access.user;
        let clock = user.clock();
        let today = clock.today();
        let start_today = clock.start_of_today();
        let presets = self.list_presets(access).await?;
        let target_of: HashMap<i64, f64> = presets.iter().map(|p| (p.id, p.desired_retention)).collect();
        let deck_target: HashMap<i64, f64> = known
            .decks
            .iter()
            .map(|d| (d.deck.id, target_of.get(&d.deck.preset_id).copied().unwrap_or(0.9)))
            .collect();

        // Six weeks of answers: recall now and before, pace, sessions.
        #[derive(sqlx::FromRow)]
        struct Row {
            deck_id: i64,
            card_id: i64,
            rating: i64,
            state_before: i64,
            reviewed_at: i64,
            duration_ms: Option<i64>,
        }
        let since = clock.day_start(today - Duration::days(41));
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT deck_id, card_id, rating, state_before, reviewed_at, duration_ms FROM review_log
             WHERE user_id = ? AND rating > 0 AND source != 'import' AND reviewed_at >= ?
             ORDER BY reviewed_at, id",
        )
        .bind(access.user_id())
        .bind(since)
        .fetch_all(&self.db)
        .await?;
        let day_of = |at: i64| (today - clock.date_of(at)).num_days();

        let mut s = Signals {
            hour: chrono::DateTime::from_timestamp(clock.now, 0)
                .map(|t| t.with_timezone(&clock.tz).hour())
                .unwrap_or(12),
            quiet: crate::study::quiet_until(user, clock.now).is_some(),
            reviews_today: known.stats.reviews_today,
            overdue: known.overdue,
            catch_up: user.catch_up,
            breaks_on: user.session_minutes > 0,
            streak: known.stats.streak_days,
            next_days: known.stats.forecast.iter().skip(1).take(7).map(|(_, n)| *n).collect(),
            leeches: known.attention.iter().filter(|a| a.flags.iter().any(|f| f.kind == Kind::Leech)).count() as i64,
            optimizable_reviews: self.optimizable_reviews(access).await?,
            ever_optimized: user.optimized_at.is_some() || user.fsrs_params_updated_at.is_some(),
            auto_optimize: user.auto_optimize,
            ..Default::default()
        };
        s.other_flags = known.attention.len() as i64 - s.leeches;

        let visible: HashSet<i64> = known.decks.iter().map(|d| d.deck.id).collect();
        let mut per_day = [0i64; 14];
        let mut new_7d: HashMap<i64, HashSet<i64>> = HashMap::new();
        let mut recall_deck: HashMap<i64, Recall> = HashMap::new();
        let mut target_sum = 0.0;
        let mut answers_7d = Vec::new();
        let mut times_7d = Vec::new();
        for r in rows.iter().filter(|r| visible.contains(&r.deck_id)) {
            let ago = day_of(r.reviewed_at);
            if ago < 14 {
                per_day[ago as usize] += 1;
            }
            if ago < 7 {
                s.reviews_7 += 1;
                answers_7d.push((r.reviewed_at, r.duration_ms.unwrap_or(0)));
                if let Some(ms) = r.duration_ms.filter(|ms| *ms > 0) {
                    times_7d.push(ms.min(MAX_ANSWER_MS) as f64 / 1000.0);
                }
                if r.state_before == 0 {
                    new_7d.entry(r.deck_id).or_default().insert(r.card_id);
                }
            }
            if r.state_before != 2 {
                continue;
            }
            let ok = (r.rating > 1) as i64;
            let add = |c: &mut Recall| {
                c.reviews += 1;
                c.remembered += ok;
            };
            if ago < 30 {
                add(&mut s.recall_30d);
                add(recall_deck.entry(r.deck_id).or_default());
                target_sum += deck_target.get(&r.deck_id).copied().unwrap_or(0.9);
            }
            if ago < 14 {
                add(&mut s.recall_recent);
            } else {
                add(&mut s.recall_before);
            }
        }
        let study_days: Vec<f64> = per_day.iter().filter(|n| **n > 0).map(|n| *n as f64).collect();
        s.study_days_14 = study_days.len() as i64;
        s.usual_per_day = median(study_days).unwrap_or(0.0);
        s.days_studied_7 = per_day[..7].iter().filter(|n| **n > 0).count() as i64;
        s.new_started_7d = new_7d.values().map(|c| c.len() as i64).sum();
        s.answer_seconds = (times_7d.len() >= 20).then(|| median(times_7d)).flatten();
        // Today's session may still be going; only whole ones count.
        answers_7d.retain(|(at, _)| *at < start_today);
        let sessions = session_lengths(&answers_7d);
        s.session_minutes = (sessions.len() >= 3).then(|| median(sessions)).flatten();
        s.target = if s.recall_30d.reviews > 0 {
            target_sum / s.recall_30d.reviews as f64
        } else {
            let t: Vec<f64> = deck_target.values().copied().collect();
            if t.is_empty() { 0.9 } else { t.iter().sum::<f64>() / t.len() as f64 }
        };
        if user.confidence_check {
            let c = self.confidence(access, None).await?;
            s.sure = Some(Recall { reviews: c.sure, remembered: c.sure_recalled });
        }

        // The last review of each deck, however long ago.
        let last: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT deck_id, MAX(reviewed_at) FROM review_log
             WHERE user_id = ? AND rating > 0 AND source != 'import' GROUP BY deck_id",
        )
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?;
        let last: HashMap<i64, i64> = last.into_iter().filter(|(d, _)| visible.contains(d)).collect();
        s.days_since_review = last.values().max().map(|at| day_of(*at));

        // Review cards due today that the limits hold back.
        let uncapped: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT deck_id, COUNT(*) FROM cards WHERE user_id = ? AND suspended = 0 AND state = 2 AND due <= ?
             GROUP BY deck_id",
        )
        .bind(access.user_id())
        .bind(clock.now)
        .fetch_all(&self.db)
        .await?;
        let uncapped: HashMap<i64, i64> = uncapped.into_iter().collect();

        let mut flagged: HashMap<i64, i64> = HashMap::new();
        for a in known.attention {
            *flagged.entry(a.card.deck_id).or_default() += 1;
        }
        for d in known.decks {
            s.due_today += d.due_total();
            if !user.catch_up {
                s.held_back += (uncapped.get(&d.deck.id).copied().unwrap_or(0) - d.review_due).max(0);
            }
            s.decks.push(DeckSignal {
                id: d.deck.id,
                // A nested deck by its own name: "Spanish", not the path.
                name: d.deck.name.rsplit("::").map(str::trim).find(|p| !p.is_empty()).unwrap_or(&d.deck.name).to_string(),
                preset_id: d.deck.preset_id,
                recall: recall_deck.get(&d.deck.id).copied().unwrap_or_default(),
                target: deck_target.get(&d.deck.id).copied().unwrap_or(0.9),
                due: d.learning_due + uncapped.get(&d.deck.id).copied().unwrap_or(0),
                new_left: d.new,
                new_started_7d: new_7d.get(&d.deck.id).map_or(0, |c| c.len() as i64),
                exam_in_days: d.deck.exam().map(|e| (e - today).num_days()).filter(|n| *n >= 0),
                days_since_review: last.get(&d.deck.id).map(|at| day_of(*at)),
                flagged: flagged.get(&d.deck.id).copied().unwrap_or(0),
            });
        }
        let seconds = known.stats.seconds_per_review.unwrap_or(12.0);
        s.minutes_due = (s.due_today as f64 * seconds / 60.0).ceil() as i64;

        for p in &presets {
            let decks: Vec<&DeckSummary> = known.decks.iter().filter(|d| d.deck.preset_id == p.id).collect();
            s.presets.push(PresetSignal {
                id: p.id,
                name: p.name.clone(),
                desired_retention: p.desired_retention,
                new_per_day: p.new_per_day,
                reviews_per_day: p.reviews_per_day,
                fitted: p.fsrs_params_updated_at.is_some() || s.ever_optimized,
                has_exam: decks.iter().any(|d| d.deck.exam().is_some()),
                used: !decks.is_empty(),
            });
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(s: &Signals) -> Vec<Rule> {
        advise(s).into_iter().map(|a| a.rule).collect()
    }

    fn first(s: &Signals) -> Option<Rule> {
        advise(s).first().map(|a| a.rule)
    }

    /// A steady learner: 100 reviews on each of the last fourteen days, on
    /// target, nothing wrong.
    fn steady() -> Signals {
        Signals {
            hour: 10,
            due_today: 100,
            minutes_due: 20,
            reviews_today: 40,
            usual_per_day: 100.0,
            study_days_14: 14,
            new_started_7d: 35,
            session_minutes: Some(20.0),
            answer_seconds: Some(8.0),
            days_since_review: Some(1),
            days_studied_7: 7,
            reviews_7: 700,
            streak: 14,
            next_days: vec![100; 7],
            recall_30d: Recall { reviews: 2000, remembered: 1800 },
            target: 0.9,
            recall_recent: Recall { reviews: 900, remembered: 810 },
            recall_before: Recall { reviews: 1100, remembered: 990 },
            optimizable_reviews: 5000,
            ever_optimized: true,
            decks: vec![DeckSignal {
                id: 1,
                name: "Spanish".into(),
                preset_id: 1,
                recall: Recall { reviews: 2000, remembered: 1800 },
                target: 0.9,
                due: 100,
                new_left: 500,
                new_started_7d: 35,
                days_since_review: Some(1),
                ..Default::default()
            }],
            presets: vec![PresetSignal {
                id: 1,
                name: "Standard".into(),
                desired_retention: 0.9,
                new_per_day: 20,
                reviews_per_day: 200,
                fitted: true,
                used: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_steady_learner_hears_nothing() {
        assert_eq!(rules(&steady()), vec![]);
    }

    #[test]
    fn a_new_user_hears_nothing() {
        assert_eq!(rules(&Signals::default()), vec![]);
    }

    #[test]
    fn rules_parse_back() {
        for r in Rule::ALL {
            assert_eq!(Rule::parse(r.as_str()), Some(r));
        }
        assert_eq!(Rule::parse("nope"), None);
    }

    #[test]
    fn the_order_is_fixed() {
        let mut sorted = Rule::ALL;
        sorted.sort();
        assert_eq!(sorted, Rule::ALL);
    }

    #[test]
    fn exam_needs_more_new_cards_than_the_preset_allows() {
        let mut s = steady();
        s.decks[0].exam_in_days = Some(12);
        s.decks[0].new_left = 300;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::ExamClose);
        assert!(a[0].text.contains("30 new cards a day"), "{}", a[0].text);
        // Enough time at the preset's pace: nothing to say.
        s.decks[0].new_left = 100;
        assert_eq!(first(&s), None);
        // Far away: nothing either.
        s.decks[0].new_left = 1000;
        s.decks[0].exam_in_days = Some(40);
        assert_eq!(first(&s), None);
    }

    #[test]
    fn exam_in_the_last_days_stops_new_cards() {
        let mut s = steady();
        s.decks[0].exam_in_days = Some(2);
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::ExamClose);
        assert!(a[0].text.contains("in 2 days") && a[0].text.contains("Stop new cards"));
        s.decks[0].exam_in_days = Some(0);
        assert!(advise(&s)[0].text.contains("is today"));
    }

    #[test]
    fn pile_up_offers_catch_up() {
        let mut s = steady();
        s.overdue = 450;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::PileUp);
        assert_eq!(a[0].catch_up_toggle, Some(true));
        assert!(a[0].text.contains("about 5 days"), "{}", a[0].text);
        // Two days of work or less is no pile-up.
        s.overdue = 180;
        assert_eq!(first(&s), None);
        // Already catching up: no pile-up note, and not caught up yet.
        s.overdue = 450;
        s.catch_up = true;
        assert_eq!(first(&s), None);
        s.overdue = 10;
        assert_eq!(first(&s), Some(Rule::CaughtUp));
    }

    #[test]
    fn late_only_with_real_work_and_outside_quiet_hours() {
        let mut s = steady();
        s.hour = 23;
        assert_eq!(first(&s), Some(Rule::Late));
        s.hour = 3;
        assert_eq!(first(&s), Some(Rule::Late));
        s.hour = 4;
        assert_eq!(first(&s), None);
        s.hour = 23;
        s.minutes_due = 10;
        assert_eq!(first(&s), None);
        s.minutes_due = 30;
        s.quiet = true;
        assert_eq!(first(&s), None);
    }

    #[test]
    fn one_leech_counts_but_one_long_card_does_not() {
        let mut s = steady();
        s.other_flags = 2;
        assert_eq!(first(&s), None);
        s.other_flags = 5;
        assert_eq!(first(&s), Some(Rule::Leeches));
        s.other_flags = 0;
        s.leeches = 1;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::Leeches);
        assert!(a[0].text.starts_with("1 card is"));
    }

    fn two_decks() -> Signals {
        let mut s = steady();
        let mut french = s.decks[0].clone();
        french.id = 2;
        french.name = "French".into();
        french.recall = Recall { reviews: 200, remembered: 160 };
        french.new_started_7d = 0;
        s.decks.push(french);
        s.recall_30d = Recall { reviews: 2200, remembered: 1960 };
        s
    }

    #[test]
    fn one_lagging_deck_is_named_with_its_cause() {
        let mut s = two_decks();
        s.decks[1].flagged = 4;
        s.other_flags = 4;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::DeckLags);
        assert!(a[0].text.starts_with("French: you remembered 80%, you aim for 90%. 4 cards"), "{}", a[0].text);
        // With the leech note above it, the deck note names another cause.
        s.leeches = 1;
        let a = advise(&s);
        assert_eq!(a[1].rule, Rule::DeckLags);
        assert!(!a[1].text.contains("keep slipping"), "{}", a[1].text);
        s.leeches = 0;
        // Too few reviews in that deck: nothing.
        s.decks[1].recall = Recall { reviews: 20, remembered: 10 };
        assert_eq!(first(&s), None);
    }

    #[test]
    fn a_single_short_deck_is_below_target() {
        let mut s = steady();
        s.recall_30d = Recall { reviews: 2000, remembered: 1600 };
        s.decks[0].recall = s.recall_30d;
        s.new_started_7d = 0;
        assert_eq!(first(&s), Some(Rule::BelowTarget));
    }

    #[test]
    fn short_everywhere_is_below_target_not_one_deck() {
        let mut s = two_decks();
        s.decks[0].recall = Recall { reviews: 2000, remembered: 1600 };
        s.recall_30d = Recall { reviews: 2200, remembered: 1760 };
        assert_eq!(first(&s), Some(Rule::BelowTarget));
    }

    #[test]
    fn below_target_names_the_likely_cause() {
        let mut s = steady();
        s.recall_30d = Recall { reviews: 1000, remembered: 800 };
        s.decks[0].recall = Recall::default();
        // 35 new cards in 7 days against a limit of 20: not many.
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::BelowTarget);
        assert!(a[0].text.contains("Cards that ask less"), "{}", a[0].text);
        // Close to the limit every day: too many new cards.
        s.new_started_7d = 140;
        assert!(advise(&s)[0].text.contains("Many new cards"));
        // Never fitted with plenty of reviews: optimize.
        s.new_started_7d = 0;
        s.ever_optimized = false;
        assert_eq!(advise(&s)[0].link.as_ref().unwrap().href, "/settings#fsrs");
        // Too little data: silence.
        s.recall_30d = Recall { reviews: 30, remembered: 10 };
        assert!(!rules(&s).contains(&Rule::BelowTarget));
    }

    #[test]
    fn sliding_compares_two_windows() {
        let mut s = steady();
        s.recall_before = Recall { reviews: 400, remembered: 364 };
        s.recall_recent = Recall { reviews: 200, remembered: 168 };
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::Sliding);
        assert!(a[0].text.starts_with("Your recall dropped from 91% to 84%"), "{}", a[0].text);
        s.recall_recent = Recall { reviews: 30, remembered: 10 };
        assert_eq!(first(&s), None);
    }

    #[test]
    fn too_sure_only_with_the_sure_check() {
        let mut s = steady();
        s.sure = Some(Recall { reviews: 120, remembered: 100 });
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::TooSure);
        assert!(a[0].text.contains("1 in 6"), "{}", a[0].text);
        s.sure = Some(Recall { reviews: 120, remembered: 110 });
        assert_eq!(first(&s), None);
    }

    #[test]
    fn a_wave_ahead_from_new_cards() {
        let mut s = steady();
        s.next_days = vec![140; 7];
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::WaveComing);
        assert!(a[0].text.contains("about 140 reviews a day, 40 more"));
        // Without new cards it is no wave to stop.
        s.new_started_7d = 0;
        s.decks[0].new_started_7d = 0;
        assert_eq!(first(&s), None);
    }

    #[test]
    fn too_fast_is_ten_reviews_per_new_card() {
        let mut s = steady();
        s.new_started_7d = 140; // 20 a day: 200 reviews against 100 usual
        assert_eq!(first(&s), Some(Rule::TooFast));
        s.new_started_7d = 98; // 14 a day: 140, under 150
        assert_eq!(first(&s), None);
        // Not enough study days for a usual pace.
        s.new_started_7d = 140;
        s.study_days_14 = 5;
        assert!(!rules(&s).contains(&Rule::TooFast));
    }

    #[test]
    fn heavy_day() {
        let mut s = steady();
        s.due_today = 160;
        s.minutes_due = 35;
        assert_eq!(first(&s), Some(Rule::HeavyDay));
        s.minutes_due = 25;
        assert_eq!(first(&s), None);
    }

    #[test]
    fn welcome_back_without_blame() {
        let mut s = steady();
        s.days_since_review = Some(4);
        s.streak = 0;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::BackAfterGap);
        assert!(a[0].text.starts_with("Welcome back. 100 cards waited for you, about 20 minutes."));
    }

    #[test]
    fn run_at_risk_only_in_the_evening() {
        let mut s = steady();
        s.days_since_review = Some(1);
        s.reviews_today = 0;
        s.hour = 19;
        assert_eq!(first(&s), Some(Rule::RunAtRisk));
        assert!(advise(&s)[0].text.contains("14-day run"));
        s.hour = 12;
        assert_eq!(first(&s), None);
        s.hour = 19;
        s.reviews_today = 5;
        assert_eq!(first(&s), None);
    }

    #[test]
    fn habits() {
        let mut s = steady();
        s.days_studied_7 = 3;
        s.reviews_7 = 300;
        assert_eq!(first(&s), Some(Rule::FewDays));

        let mut s = steady();
        s.session_minutes = Some(60.0);
        assert_eq!(first(&s), Some(Rule::LongSittings));
        s.breaks_on = true;
        assert_eq!(first(&s), None);

        let mut s = steady();
        s.answer_seconds = Some(25.0);
        assert_eq!(first(&s), Some(Rule::SlowAnswers));
    }

    #[test]
    fn setup() {
        let mut s = steady();
        s.ever_optimized = false;
        assert_eq!(first(&s), Some(Rule::NeverFitted));
        s.auto_optimize = true;
        assert_eq!(first(&s), None);

        let mut s = steady();
        s.recall_30d = Recall { reviews: 1000, remembered: 950 };
        s.decks[0].recall = s.recall_30d;
        assert_eq!(first(&s), Some(Rule::AboveTarget));

        let mut s = steady();
        s.presets[0].desired_retention = 0.97;
        assert_eq!(first(&s), Some(Rule::OddTarget));
        s.presets[0].has_exam = true;
        assert_eq!(first(&s), None);
        s.presets[0].desired_retention = 0.75;
        assert_eq!(first(&s), Some(Rule::OddTarget));

        let mut s = steady();
        s.presets[0].reviews_per_day = 100;
        assert_eq!(first(&s), Some(Rule::TightLimit));
        s.presets[0].reviews_per_day = 200;
        s.held_back = 60;
        assert!(advise(&s)[0].text.contains("held back 60 cards"));
    }

    #[test]
    fn a_forgotten_deck() {
        let mut s = two_decks();
        s.decks[1].recall = s.decks[0].recall;
        s.recall_30d = Recall { reviews: 4000, remembered: 3600 };
        s.decks[1].days_since_review = Some(21);
        s.decks[1].due = 120;
        let a = advise(&s);
        assert_eq!(a[0].rule, Rule::ForgottenDeck);
        assert_eq!(a[0].text, "French waited 3 weeks; 120 are due.");
        // Away from everything: welcome back speaks instead.
        s.days_since_review = Some(5);
        assert!(!rules(&s).contains(&Rule::ForgottenDeck));
    }

    #[test]
    fn urgent_beats_tuning() {
        let mut s = steady();
        s.ever_optimized = false;
        s.hour = 23;
        s.leeches = 2;
        assert_eq!(rules(&s), vec![Rule::Late, Rule::Leeches, Rule::NeverFitted]);
    }

    #[test]
    fn notes_never_use_dashes() {
        let mut s = two_decks();
        s.hour = 23;
        s.overdue = 900;
        s.leeches = 3;
        s.decks[0].exam_in_days = Some(10);
        s.decks[1].flagged = 2;
        s.sure = Some(Recall { reviews: 100, remembered: 70 });
        s.presets[0].reviews_per_day = 50;
        for a in advise(&s) {
            assert!(!a.text.contains('—') && !a.text.contains(" - "), "{}", a.text);
        }
    }
    #[test]
    fn sessions_split_at_ten_minute_pauses() {
        let t = 1_000_000;
        // Two answers a minute apart, then a long pause, then one more.
        let s = session_lengths(&[(t, 10_000), (t + 60, 30_000), (t + 3600, 5_000)]);
        assert_eq!(s.len(), 2);
        assert!((s[0] - 1.5).abs() < 0.01, "{s:?}");
        // A card left open for an hour counts as a minute.
        assert!(session_lengths(&[(t, 3_600_000)])[0] <= 1.0);
        assert!(session_lengths(&[]).is_empty());
    }

    #[test]
    fn medians() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }
}
