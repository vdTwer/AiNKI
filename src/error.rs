use std::fmt;

#[derive(Debug)]
pub enum AppError {
    NotFound,
    Unauthorized,
    Forbidden(String),
    BadRequest(String),
    Conflict(String),
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
            Self::BadRequest(m) => write!(f, "{m}"),
            Self::Conflict(m) => write!(f, "{m}"),
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
