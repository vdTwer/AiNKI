use std::fmt;

/// What can go wrong in the service. The `# Errors` section of each public
/// method names the variants it returns and when; they mean the same
/// everywhere, so the web and MCP layers map each to one status.
#[derive(Debug)]
pub enum AppError {
    /// The thing doesn't exist, isn't the user's, or lies outside a token's
    /// decks. All three look the same, so ids say nothing about others.
    NotFound,
    /// No valid login or token.
    Unauthorized,
    /// The token may not do this: it is read-only, may not submit reviews,
    /// or is limited to some decks for an account-wide action.
    Forbidden(String),
    /// The input breaks a rule; the message says which, for the user.
    BadRequest(String),
    /// The state changed under the caller: the card was answered since it
    /// was shown, or the name is taken.
    Conflict(String),
    /// The database, the data folder or FSRS failed. A bug or an outage,
    /// never the caller's doing; the message is logged, not shown.
    Internal(anyhow::Error),
}

pub type Result<T, E = AppError> = std::result::Result<T, E>;

impl AppError {
    pub fn bad(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Unauthorized => write!(f, "not logged in"),
            Self::Forbidden(m) => write!(f, "forbidden: {m}"),
            Self::BadRequest(m) | Self::Conflict(m) => write!(f, "{m}"),
            Self::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            e => Self::Internal(e.into()),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl From<fsrs::FSRSError> for AppError {
    fn from(e: fsrs::FSRSError) -> Self {
        Self::Internal(anyhow::anyhow!("FSRS: {e:?}"))
    }
}
