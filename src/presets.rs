//! Built-in scheduling presets.
//!
//! The numbers follow the FSRS project's published guidance:
//! - FSRS-6 default parameters are fitted on ~10k Anki collections and are
//!   used until the user has enough history to optimize their own.
//! - Desired retention between 0.80 and 0.95 is the useful range: below ~0.80
//!   you forget too much to be efficient, above ~0.95 the workload grows
//!   steeply (0.95 costs roughly twice the reviews of 0.90).
//! - Learning steps should be short (minutes, < 1 day). Inter-day spacing is
//!   FSRS's job; multi-day learning steps fight the model.
//! - Daily review limit should be at least ~10x the new card limit, since each
//!   new card produces about ten reviews over its first month.

pub struct PresetTemplate {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub desired_retention: f64,
    pub new_per_day: i64,
    pub reviews_per_day: i64,
    pub learning_steps: &'static str,
    pub relearning_steps: &'static str,
    pub max_interval: i64,
}

pub const TEMPLATES: &[PresetTemplate] = &[
    PresetTemplate {
        key: "standard",
        name: "Standard",
        description: "Good default for most material. 90% target retention, 20 new cards a day.",
        desired_retention: 0.90,
        new_per_day: 20,
        reviews_per_day: 200,
        learning_steps: "10m",
        relearning_steps: "10m",
        max_interval: 36500,
    },
    PresetTemplate {
        key: "exam",
        name: "Exam prep",
        description: "High retention for an upcoming test. Set an exam date on the deck so no card is scheduled past it. Expect about twice the reviews of Standard.",
        desired_retention: 0.95,
        new_per_day: 50,
        reviews_per_day: 9999,
        learning_steps: "1m 10m",
        relearning_steps: "10m",
        max_interval: 36500,
    },
    PresetTemplate {
        key: "language",
        name: "Language vocabulary",
        description: "Lots of small cards learned every day. 90% retention, 30 new cards a day.",
        desired_retention: 0.90,
        new_per_day: 30,
        reviews_per_day: 300,
        learning_steps: "10m",
        relearning_steps: "10m",
        max_interval: 36500,
    },
    PresetTemplate {
        key: "relaxed",
        name: "Relaxed",
        description: "Long-term learning on a small time budget. 85% retention is close to the least work per card remembered.",
        desired_retention: 0.85,
        new_per_day: 10,
        reviews_per_day: 100,
        learning_steps: "10m",
        relearning_steps: "10m",
        max_interval: 36500,
    },
    PresetTemplate {
        key: "maintenance",
        name: "Maintenance",
        description: "Keep old knowledge around with minimal effort. 80% retention, no new cards.",
        desired_retention: 0.80,
        new_per_day: 0,
        reviews_per_day: 100,
        learning_steps: "10m",
        relearning_steps: "10m",
        max_interval: 36500,
    },
];

pub fn template(key: &str) -> Option<&'static PresetTemplate> {
    TEMPLATES.iter().find(|t| t.key == key)
}
