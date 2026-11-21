//! Everything a user can do. The website and the MCP server are both thin
//! layers over these functions, so agents can do whatever humans can (within
//! the limits of their token).
//!
//! One file per area, each holding its types and the `App` methods that work
//! on them. This file holds what they share.

use std::collections::HashMap;

use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    App,
    auth::{Access, Actor},
    error::{AppError, Result},
    markdown,
    multi::{Missed, Split},
    now, num, presets,
    scheduler::{self, CardSched, CardState, Clock, Rating, SchedConfig},
};

mod card;
mod deck;
mod preset;
mod settings;
mod stats;
mod study;

pub use card::*;
pub use deck::*;
pub use preset::*;
pub use settings::*;
pub use stats::*;
pub use study::*;

/// Show learning cards up to this many seconds early when nothing else is due.
const LEARN_AHEAD_SECS: i64 = 20 * 60;

/// One field of an update: keep what is stored, or replace it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Patch<T> {
    #[default]
    Keep,
    Set(T),
}

impl<T> Patch<T> {
    /// The new value, or `current` when the field is kept.
    pub fn or(self, current: T) -> T {
        match self {
            Self::Keep => current,
            Self::Set(v) => v,
        }
    }
}

pub const MAX_CARD_TEXT: usize = 20_000;
pub const MAX_NAME: usize = 200;
pub const MAX_DESCRIPTION: usize = 2_000;

/// Reject oversized text so one request can't bloat the database or pages.
fn check_len(what: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max {
        Err(AppError::bad(format!("{what} is too long (at most {max} characters)")))
    } else {
        Ok(())
    }
}

/// Ids bound as one JSON array, for `IN (SELECT value FROM json_each(?))`.
/// One parameter however many ids: no placeholder lists to build, no
/// limit on their number, and the SQL stays a fixed string.
pub(crate) const fn id_list(ids: &[i64]) -> sqlx::types::Json<&[i64]> {
    sqlx::types::Json(ids)
}
