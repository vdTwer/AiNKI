//! Card kinds: how a card is written and studied. A basic card is text
//! that you grade yourself; other kinds keep their own content in the same
//! two sides, check it when saved, and may grade the answer themselves.
//! Each new kind adds a variant here and a module for its rules (see
//! `chess`).

use serde::{Deserialize, Serialize};

use crate::{chess, error::{AppError, Result}, markdown};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, schemars::JsonSchema)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum CardKind {
    /// Text on both sides; you say how well you remembered.
    #[default]
    Basic,
    /// A position (FEN) on the front; the move that answers it on the back.
    Chess,
}

impl CardKind {
    pub const ALL: [CardKind; 2] = [CardKind::Basic, CardKind::Chess];

    pub fn as_str(self) -> &'static str {
        match self {
            CardKind::Basic => "basic",
            CardKind::Chess => "chess",
        }
    }

    pub fn parse(s: &str) -> Option<CardKind> {
        CardKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// What people see when picking a kind.
    pub fn label(self) -> &'static str {
        match self {
            CardKind::Basic => "Text",
            CardKind::Chess => "Chess",
        }
    }

    /// The sides as they are saved, or what is wrong with them.
    pub fn tidy(self, front: &str, back: &str) -> Result<(String, String)> {
        match self {
            CardKind::Basic => Ok((markdown::tidy(front.trim()), markdown::tidy(back.trim()))),
            CardKind::Chess => chess::tidy(front, back).map_err(AppError::bad),
        }
    }

    /// Only basic cards take images and can be learned item by item.
    pub fn is_basic(self) -> bool {
        self == CardKind::Basic
    }

    /// How a card is named in lists.
    pub fn title(self, front: &str) -> String {
        match self {
            CardKind::Basic => front.to_string(),
            CardKind::Chess => chess::title(front),
        }
    }
}
