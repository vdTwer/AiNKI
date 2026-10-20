//! Accounts, browser sessions, API tokens and per-request access control.

use std::collections::HashSet;

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use rand::{Rng, RngCore};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    App,
    error::{AppError, Result},
    now, presets,
};

pub const SESSION_DAYS: i64 = 90;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub discriminator: i64,
    #[serde(skip)]
    pub password_hash: String,
    pub timezone: String,
    pub rollover_hour: i64,
    pub four_buttons: bool,
    pub font_scale: i64,
    pub fsrs_params: String,
    pub fsrs_params_updated_at: Option<i64>,
    pub optimize_include_agent: bool,
    /// 'sketch' or 'simple'; only matters on desktop computers.
    pub desktop_look: String,
    pub created_at: i64,
}

impl User {
    pub fn handle(&self) -> String {
        format!("{}#{:04}", self.username, self.discriminator)
    }

    pub fn params(&self) -> Vec<f32> {
        serde_json::from_str(&self.fsrs_params).unwrap_or_default()
    }

    pub fn tz(&self) -> chrono_tz::Tz {
        self.timezone.parse().unwrap_or(chrono_tz::UTC)
    }

    pub fn clock(&self) -> crate::scheduler::Clock {
        crate::scheduler::Clock::new(now(), self.tz(), self.rollover_hour.clamp(0, 23) as u32)
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct ApiToken {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    #[serde(skip)]
    pub token_hash: String,
    pub prefix: String,
    pub can_write: bool,
    pub can_review: bool,
    pub all_decks: bool,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
    /// Set for connections approved through OAuth.
    pub oauth_client_id: Option<String>,
}

/// Who is acting. The website acts with the user's full rights; agents act
/// through an API token with whatever rights the user granted it.
#[derive(Debug, Clone)]
pub enum Actor {
    Web,
    Agent {
        token_id: i64,
        can_write: bool,
        can_review: bool,
        /// `None` means all decks.
        decks: Option<HashSet<i64>>,
    },
}

#[derive(Debug, Clone)]
pub struct Access {
    pub user: User,
    pub actor: Actor,
}

impl Access {
    pub fn web(user: User) -> Self {
        Self { user, actor: Actor::Web }
    }

    pub fn user_id(&self) -> i64 {
        self.user.id
    }

    pub fn source(&self) -> &'static str {
        match self.actor {
            Actor::Web => "web",
            Actor::Agent { .. } => "agent",
        }
    }

    pub fn require_write(&self) -> Result<()> {
        match &self.actor {
            Actor::Agent { can_write: false, .. } => {
                Err(AppError::forbidden("this token is read-only"))
            }
            _ => Ok(()),
        }
    }

    pub fn require_review(&self) -> Result<()> {
        match &self.actor {
            Actor::Agent { can_review: false, .. } => {
                Err(AppError::forbidden("this token may not submit reviews"))
            }
            _ => Ok(()),
        }
    }

    pub fn can_see_deck(&self, deck_id: i64) -> bool {
        match &self.actor {
            Actor::Agent { decks: Some(d), .. } => d.contains(&deck_id),
            _ => true,
        }
    }

    pub fn require_deck(&self, deck_id: i64) -> Result<()> {
        if self.can_see_deck(deck_id) {
            Ok(())
        } else {
            Err(AppError::NotFound)
        }
    }

    /// Actions that affect the whole account (creating/deleting decks,
    /// editing presets, optimizing parameters) need an unrestricted token.
    pub fn require_all_decks(&self) -> Result<()> {
        match &self.actor {
            Actor::Agent { decks: Some(_), .. } => Err(AppError::forbidden(
                "this token is limited to specific decks",
            )),
            _ => Ok(()),
        }
    }
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("hashing failed: {e}")))
}

fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
        .unwrap_or(false)
}

pub(crate) fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn validate_username(name: &str) -> Result<()> {
    let ok_len = (2..=32).contains(&name.chars().count());
    // ASCII only, so nobody can pose as "kolja" with a look-alike letter.
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(AppError::bad(
            "username must be 2–32 characters: a-z, 0-9, _ - .",
        ))
    }
}

/// Longer passwords are refused so nobody can make Argon2 hash megabytes.
pub const MAX_PASSWORD_BYTES: usize = 1024;

pub fn validate_password(pw: &str) -> Result<()> {
    if pw.chars().count() < 8 {
        Err(AppError::bad("password must be at least 8 characters"))
    } else if pw.len() > MAX_PASSWORD_BYTES {
        Err(AppError::bad("password is too long"))
    } else {
        Ok(())
    }
}

/// A valid hash of a random password, verified against when the user doesn't
/// exist so both cases take the same time.
fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password(&random_hex(16)).unwrap_or_default())
}

/// Split "name#1234" into its parts.
pub fn parse_handle(handle: &str) -> (String, Option<i64>) {
    let handle = handle.trim();
    match handle.rsplit_once('#') {
        Some((name, tag)) => match tag.parse::<i64>() {
            Ok(t) => (name.to_string(), Some(t)),
            Err(_) => (handle.to_string(), None),
        },
        None => (handle.to_string(), None),
    }
}

impl App {
    pub async fn create_user(&self, username: &str, password: &str, timezone: &str) -> Result<User> {
        let username = username.trim();
        validate_username(username)?;
        validate_password(password)?;
        let tz = if timezone.parse::<chrono_tz::Tz>().is_ok() {
            timezone.to_string()
        } else {
            self.config.default_timezone.clone()
        };
        let pw = password.to_string();
        let hash = tokio::task::spawn_blocking(move || hash_password(&pw))
            .await
            .map_err(|e| AppError::Internal(e.into()))??;

        let taken: Vec<i64> =
            sqlx::query_scalar("SELECT discriminator FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username)
                .fetch_all(&self.db)
                .await?;
        if taken.len() >= 9999 {
            return Err(AppError::bad("that username is full, pick another"));
        }
        let taken: HashSet<i64> = taken.into_iter().collect();
        let discriminator = loop {
            let d = rand::thread_rng().gen_range(1..=9999);
            if !taken.contains(&d) {
                break d;
            }
        };

        let mut tx = self.db.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, discriminator, password_hash, timezone, created_at)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(username)
        .bind(discriminator)
        .bind(&hash)
        .bind(&tz)
        .bind(now())
        .fetch_one(&mut *tx)
        .await?;
        for t in presets::TEMPLATES {
            sqlx::query(
                "INSERT INTO presets (user_id, name, description, desired_retention, new_per_day,
                    reviews_per_day, learning_steps, relearning_steps, max_interval, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(t.name)
            .bind(t.description)
            .bind(t.desired_retention)
            .bind(t.new_per_day)
            .bind(t.reviews_per_day)
            .bind(t.learning_steps)
            .bind(t.relearning_steps)
            .bind(t.max_interval)
            .bind(now())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.user(id).await
    }

    pub async fn user(&self, id: i64) -> Result<User> {
        Ok(sqlx::query_as("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&self.db)
            .await?)
    }

    pub async fn find_user(&self, handle: &str) -> Result<User> {
        let (name, tag) = parse_handle(handle);
        let users: Vec<User> = match tag {
            Some(t) => sqlx::query_as(
                "SELECT * FROM users WHERE username = ? COLLATE NOCASE AND discriminator = ?",
            )
            .bind(&name)
            .bind(t)
            .fetch_all(&self.db)
            .await?,
            None => sqlx::query_as("SELECT * FROM users WHERE username = ? COLLATE NOCASE")
                .bind(&name)
                .fetch_all(&self.db)
                .await?,
        };
        match users.len() {
            1 => Ok(users.into_iter().next().unwrap()),
            0 => Err(AppError::NotFound),
            _ => Err(AppError::bad(format!(
                "several users are called {name}; log in with your full name#tag"
            ))),
        }
    }

    pub async fn authenticate(&self, handle: &str, password: &str) -> Result<User> {
        let wrong = || AppError::bad("wrong username or password");
        if password.len() > MAX_PASSWORD_BYTES {
            return Err(wrong());
        }
        let user = match self.find_user(handle).await {
            Ok(u) => u,
            Err(AppError::NotFound) => {
                let pw = password.to_string();
                let _ = tokio::task::spawn_blocking(move || verify_password(&pw, dummy_hash())).await;
                return Err(wrong());
            }
            Err(e) => return Err(e),
        };
        let pw = password.to_string();
        let hash = user.password_hash.clone();
        let ok = tokio::task::spawn_blocking(move || verify_password(&pw, &hash))
            .await
            .unwrap_or(false);
        if ok {
            Ok(user)
        } else {
            Err(AppError::bad("wrong username or password"))
        }
    }

    pub async fn set_password(&self, user_id: i64, password: &str) -> Result<()> {
        validate_password(password)?;
        let hash = hash_password(password)?;
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(hash)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn change_password(&self, user: &User, current: &str, new: &str) -> Result<()> {
        if current.len() > MAX_PASSWORD_BYTES || !verify_password(current, &user.password_hash) {
            return Err(AppError::bad("current password is wrong"));
        }
        self.set_password(user.id, new).await?;
        // Log out every other browser.
        sqlx::query("DELETE FROM sessions WHERE user_id = ?")
            .bind(user.id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn create_session(&self, user_id: i64) -> Result<String> {
        let token = random_hex(32);
        sqlx::query("INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
            .bind(sha256_hex(&token))
            .bind(user_id)
            .bind(now())
            .bind(now() + SESSION_DAYS * 86_400)
            .execute(&self.db)
            .await?;
        Ok(token)
    }

    pub async fn session_user(&self, token: &str) -> Result<Option<User>> {
        Ok(sqlx::query_as(
            "SELECT users.* FROM sessions JOIN users ON users.id = sessions.user_id
             WHERE sessions.id = ? AND sessions.expires_at > ?",
        )
        .bind(sha256_hex(token))
        .bind(now())
        .fetch_optional(&self.db)
        .await?)
    }

    pub async fn delete_session(&self, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(sha256_hex(token))
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Returns the token record and the raw secret, which is shown only once.
    pub async fn create_token(
        &self,
        user_id: i64,
        name: &str,
        can_write: bool,
        can_review: bool,
        deck_ids: Option<Vec<i64>>,
    ) -> Result<(ApiToken, String)> {
        let raw = format!("ainki_{}", random_hex(24));
        let token = self
            .create_grant(user_id, name, can_write, can_review, deck_ids, &raw, None)
            .await?;
        Ok((token, raw))
    }

    /// Store an agent's permissions. For OAuth connections `secret` is never
    /// handed out; the client gets short-lived access tokens instead.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_grant(
        &self,
        user_id: i64,
        name: &str,
        can_write: bool,
        can_review: bool,
        deck_ids: Option<Vec<i64>>,
        secret: &str,
        oauth_client_id: Option<&str>,
    ) -> Result<ApiToken> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::bad("give the token a name"));
        }
        let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
        let name = name.as_str();
        let prefix = if oauth_client_id.is_some() { "oauth".to_string() } else { secret[..12].to_string() };
        let mut tx = self.db.begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO api_tokens (user_id, name, token_hash, prefix, can_write, can_review, all_decks,
                created_at, oauth_client_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(user_id)
        .bind(name)
        .bind(sha256_hex(secret))
        .bind(&prefix)
        .bind(can_write)
        .bind(can_review)
        .bind(deck_ids.is_none())
        .bind(now())
        .bind(oauth_client_id)
        .fetch_one(&mut *tx)
        .await?;
        if let Some(decks) = &deck_ids {
            if decks.is_empty() {
                return Err(AppError::bad("pick at least one deck, or allow all decks"));
            }
            for deck in decks {
                let owned: Option<i64> =
                    sqlx::query_scalar("SELECT id FROM decks WHERE id = ? AND user_id = ?")
                        .bind(deck)
                        .bind(user_id)
                        .fetch_optional(&mut *tx)
                        .await?;
                if owned.is_none() {
                    return Err(AppError::NotFound);
                }
                sqlx::query("INSERT INTO api_token_decks (token_id, deck_id) VALUES (?, ?)")
                    .bind(id)
                    .bind(deck)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(sqlx::query_as("SELECT * FROM api_tokens WHERE id = ?")
            .bind(id)
            .fetch_one(&self.db)
            .await?)
    }

    pub async fn list_tokens(&self, user_id: i64) -> Result<Vec<(ApiToken, Vec<String>)>> {
        let tokens: Vec<ApiToken> = sqlx::query_as(
            "SELECT * FROM api_tokens WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await?;
        let mut out = Vec::new();
        for t in tokens {
            let decks: Vec<String> = sqlx::query_scalar(
                "SELECT decks.name FROM api_token_decks JOIN decks ON decks.id = api_token_decks.deck_id
                 WHERE token_id = ? ORDER BY decks.name",
            )
            .bind(t.id)
            .fetch_all(&self.db)
            .await?;
            out.push((t, decks));
        }
        Ok(out)
    }

    pub async fn revoke_token(&self, user_id: i64, token_id: i64) -> Result<()> {
        let n = sqlx::query("UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND user_id = ?")
            .bind(now())
            .bind(token_id)
            .bind(user_id)
            .execute(&self.db)
            .await?
            .rows_affected();
        if n > 0 {
            sqlx::query("DELETE FROM oauth_tokens WHERE api_token_id = ?")
                .bind(token_id)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }

    pub async fn token_access(&self, raw: &str) -> Result<Access> {
        let hash = sha256_hex(raw.trim());
        // A manual token, or an OAuth access token pointing at its connection.
        let token: ApiToken = sqlx::query_as(
            "SELECT * FROM api_tokens WHERE revoked_at IS NULL AND (
                (token_hash = ? AND oauth_client_id IS NULL)
                OR id = (SELECT api_token_id FROM oauth_tokens
                         WHERE token_hash = ? AND kind = 'access' AND expires_at > ?))",
        )
        .bind(&hash)
        .bind(&hash)
        .bind(now())
        .fetch_optional(&self.db)
        .await?
        .ok_or(AppError::Unauthorized)?;
        sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
            .bind(now())
            .bind(token.id)
            .execute(&self.db)
            .await?;
        let decks = if token.all_decks {
            None
        } else {
            let ids: Vec<i64> =
                sqlx::query_scalar("SELECT deck_id FROM api_token_decks WHERE token_id = ?")
                    .bind(token.id)
                    .fetch_all(&self.db)
                    .await?;
            Some(ids.into_iter().collect())
        };
        Ok(Access {
            user: self.user(token.user_id).await?,
            actor: Actor::Agent {
                token_id: token.id,
                can_write: token.can_write,
                can_review: token.can_review,
                decks,
            },
        })
    }
}
