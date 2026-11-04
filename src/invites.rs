//! Invitations, and what the admin account sees.
//!
//! Signing up takes an invite link. Every account can make one link a
//! week, starting a week after it joined; a link works once, for 30 days,
//! and can be taken back while unused (which gives the week back). The
//! admin account invites without a limit and sees only the admin panel:
//! who invited whom, and an overview of all accounts.

use serde::Serialize;

use crate::{
    App,
    auth::{User, random_hex},
    error::{AppError, Result},
    now,
};

pub const WEEK: i64 = 7 * 86_400;
pub const INVITE_DAYS: i64 = 30;

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Invite {
    pub id: i64,
    pub token: String,
    pub user_id: i64,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_at: Option<i64>,
    pub used_by: Option<i64>,
}

impl Invite {
    pub fn open(&self, at: i64) -> bool {
        self.used_at.is_none() && self.expires_at > at
    }
}

/// An account's invites and when it may make the next one.
pub struct Invites {
    /// Links nobody has used yet and that still work.
    pub open: Vec<Invite>,
    /// Accounts that joined through this one's links, oldest first.
    pub joined: Vec<(User, i64)>,
    /// None: a link can be made now. Some(t): not before t.
    pub next_at: Option<i64>,
    /// The wait is the first week after joining, not the last invite.
    pub new_account: bool,
}

/// A line of the admin's overview.
#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct Account {
    pub id: i64,
    pub username: String,
    pub discriminator: i64,
    pub created_at: i64,
    pub invited_by: Option<i64>,
    pub is_admin: bool,
    pub decks: i64,
    pub cards: i64,
    pub reviews_30d: i64,
    pub last_review: Option<i64>,
    pub invites_sent: i64,
}

impl Account {
    pub fn handle(&self) -> String {
        format!("{}#{:04}", self.username, self.discriminator)
    }
}

impl App {
    /// When `user` may make an invite next; None for right now.
    async fn next_invite_at(&self, user: &User) -> Result<(Option<i64>, bool)> {
        if user.is_admin {
            return Ok((None, false));
        }
        let t = now();
        if user.created_at + WEEK > t {
            return Ok((Some(user.created_at + WEEK), true));
        }
        let last: Option<i64> = sqlx::query_scalar("SELECT MAX(created_at) FROM invites WHERE user_id = ?")
            .bind(user.id)
            .fetch_one(&self.db)
            .await?;
        Ok((last.map(|l| l + WEEK).filter(|&n| n > t), false))
    }

    pub async fn invites(&self, user: &User) -> Result<Invites> {
        let open = sqlx::query_as(
            "SELECT * FROM invites WHERE user_id = ? AND used_at IS NULL AND expires_at > ? ORDER BY created_at DESC",
        )
        .bind(user.id)
        .bind(now())
        .fetch_all(&self.db)
        .await?;
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT used_by, used_at FROM invites WHERE user_id = ? AND used_by IS NOT NULL ORDER BY used_at",
        )
        .bind(user.id)
        .fetch_all(&self.db)
        .await?;
        let mut joined = Vec::new();
        for (id, at) in rows {
            if let Ok(u) = self.user(id).await {
                joined.push((u, at));
            }
        }
        let (next_at, new_account) = self.next_invite_at(user).await?;
        Ok(Invites { open, joined, next_at, new_account })
    }

    /// A new link, if the week allows it.
    pub async fn create_invite(&self, user: &User) -> Result<Invite> {
        if self.next_invite_at(user).await?.0.is_some() {
            return Err(AppError::bad("you can make one invite a week"));
        }
        let t = now();
        Ok(sqlx::query_as(
            "INSERT INTO invites (token, user_id, created_at, expires_at) VALUES (?, ?, ?, ?) RETURNING *",
        )
        .bind(random_hex(16))
        .bind(user.id)
        .bind(t)
        .bind(t + INVITE_DAYS * 86_400)
        .fetch_one(&self.db)
        .await?)
    }

    /// Take back a link nobody has used. The week comes back with it.
    pub async fn cancel_invite(&self, user: &User, id: i64) -> Result<()> {
        let done = sqlx::query("DELETE FROM invites WHERE id = ? AND user_id = ? AND used_at IS NULL")
            .bind(id)
            .bind(user.id)
            .execute(&self.db)
            .await?;
        if done.rows_affected() == 0 {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    /// The invite behind a link, with the account that made it, while it
    /// still works.
    pub async fn open_invite(&self, token: &str) -> Result<Option<(Invite, User)>> {
        if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let invite: Option<Invite> = sqlx::query_as("SELECT * FROM invites WHERE token = ?")
            .bind(token)
            .fetch_optional(&self.db)
            .await?;
        match invite.filter(|i| i.open(now())) {
            Some(i) => {
                let from = self.user(i.user_id).await?;
                Ok(Some((i, from)))
            }
            None => Ok(None),
        }
    }

    /// Every account with a few numbers, oldest first. Admin only.
    pub async fn accounts(&self, admin: &User) -> Result<Vec<Account>> {
        if !admin.is_admin {
            return Err(AppError::forbidden("admin only"));
        }
        let since = now() - 30 * 86_400;
        Ok(sqlx::query_as(
            "SELECT u.id, u.username, u.discriminator, u.created_at, u.invited_by, u.is_admin,
                (SELECT COUNT(*) FROM decks d WHERE d.user_id = u.id) AS decks,
                (SELECT COUNT(*) FROM cards c WHERE c.user_id = u.id) AS cards,
                (SELECT COUNT(*) FROM review_log r WHERE r.user_id = u.id AND r.reviewed_at >= ? AND r.source != 'import') AS reviews_30d,
                (SELECT MAX(r.reviewed_at) FROM review_log r WHERE r.user_id = u.id AND r.source != 'import') AS last_review,
                (SELECT COUNT(*) FROM invites i WHERE i.user_id = u.id) AS invites_sent
             FROM users u ORDER BY u.created_at, u.id",
        )
        .bind(since)
        .fetch_all(&self.db)
        .await?)
    }
}
