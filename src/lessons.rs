//! Built-in lessons: how AiNKI schedules, how memory works, and how to
//! write good cards. Each is a Markdown file in `lessons/` with a short
//! header and a few cards at the end, so a lesson can be added as a deck.
//!
//! File format:
//!
//! ```text
//! title: The spacing effect
//! group: memory
//! evidence: strong
//! summary: One line for the list.
//!
//! The lesson in Markdown.
//!
//! === cards
//! Front :: Back
//! ```

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// Many studies agree; meta-analyses.
    Strong,
    /// Good studies, fewer or smaller.
    Moderate,
    /// Works in some settings, not in others.
    Mixed,
    /// From FSRS's own data and simulations on real review histories.
    Fsrs,
    /// A rule of thumb from experienced users, not tested in trials.
    Practice,
    /// Tested and not supported.
    Against,
}

impl Evidence {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "strong" => Self::Strong,
            "moderate" => Self::Moderate,
            "mixed" => Self::Mixed,
            "fsrs" => Self::Fsrs,
            "practice" => Self::Practice,
            "against" => Self::Against,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Strong => "Strong evidence",
            Self::Moderate => "Moderate evidence",
            Self::Mixed => "Mixed evidence",
            Self::Fsrs => "From FSRS data",
            Self::Practice => "Rule of thumb",
            Self::Against => "Not supported by evidence",
        }
    }

    pub fn class(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Moderate => "moderate",
            Self::Mixed => "mixed",
            Self::Fsrs => "fsrs",
            Self::Practice => "practice",
            Self::Against => "against",
        }
    }
}

/// The groups in the order they are listed.
pub const GROUPS: &[(&str, &str, &str)] = &[
    ("scheduling", "How AiNKI schedules", "FSRS, retention and what it costs."),
    ("memory", "How memory works", "What research says about learning, and what it doesn't."),
    ("cards", "Writing good cards", "Cards that are quick to review and hard to forget."),
];

#[derive(Debug)]
pub struct Lesson {
    pub slug: &'static str,
    pub title: String,
    pub group: String,
    pub evidence: Evidence,
    pub summary: String,
    pub body: String,
    pub cards: Vec<(String, String)>,
}

macro_rules! lesson_files {
    ($($slug:literal),* $(,)?) => {
        &[$(($slug, include_str!(concat!("lessons/", $slug, ".md")))),*]
    };
}

/// In reading order within each group.
const FILES: &[(&str, &str)] = lesson_files![
    "forgetting-curve",
    "desired-retention",
    "two-buttons",
    "optimizing",
    "leeches",
    "backlog",
    "spacing",
    "testing",
    "desirable-difficulties",
    "interleaving",
    "interference",
    "sleep",
    "illusions",
    "learning-styles",
    "good-cards",
    "lists",
    "images-and-mnemonics",
    "timed-and-typed",
];

fn parse(slug: &'static str, text: &str) -> Lesson {
    let (head, rest) = text.split_once("\n\n").unwrap_or_else(|| panic!("lesson {slug}: no header"));
    let field = |name: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix(name).and_then(|v| v.strip_prefix(':')))
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| panic!("lesson {slug}: no {name}"))
    };
    let (body, cards) = rest.split_once("\n=== cards\n").unwrap_or((rest, ""));
    let cards = cards
        .lines()
        .filter_map(|l| l.split_once(" :: "))
        .map(|(f, b)| (f.trim().to_string(), b.trim().to_string()))
        .collect();
    let evidence = field("evidence");
    Lesson {
        slug,
        title: field("title"),
        group: field("group"),
        evidence: Evidence::parse(&evidence).unwrap_or_else(|| panic!("lesson {slug}: evidence {evidence}")),
        summary: field("summary"),
        body: body.trim().to_string(),
        cards,
    }
}

pub fn all() -> &'static [Lesson] {
    static LESSONS: OnceLock<Vec<Lesson>> = OnceLock::new();
    LESSONS.get_or_init(|| FILES.iter().map(|(slug, text)| parse(slug, text)).collect())
}

pub fn find(slug: &str) -> Option<&'static Lesson> {
    all().iter().find(|l| l.slug == slug)
}

/// The deck a lesson becomes.
pub fn deck_name(lesson: &Lesson) -> String {
    format!("AiNKI lessons::{}", lesson.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lesson_parses_and_links_resolve() {
        let lessons = all();
        assert_eq!(lessons.len(), FILES.len());
        for l in lessons {
            assert!(GROUPS.iter().any(|g| g.0 == l.group), "{}: group {}", l.slug, l.group);
            assert!(l.cards.len() >= 3, "{}: only {} cards", l.slug, l.cards.len());
            assert!(!l.body.contains('\u{2014}'), "{}: em dash", l.slug);
            // Links to other lessons point at lessons that exist.
            for part in l.body.split("](/learn/").skip(1) {
                let target = part.split([')', '#']).next().unwrap();
                assert!(find(target).is_some(), "{}: broken link to {target}", l.slug);
            }
        }
    }
}
