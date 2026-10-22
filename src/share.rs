//! Deck sharing. Sharing offers a deck to another user; when they accept,
//! they get their own independent copy of its cards (fresh progress, no link
//! back to the original).

use serde::Serialize;

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    now,
    service::Deck,
};

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct IncomingShare {
    pub id: i64,
    pub deck_id: i64,
    pub deck_name: String,
    pub deck_description: String,
    pub cards: i64,
    pub from: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OutgoingShare {
    pub id: i64,
    pub deck_id: i64,
    pub to: String,
    pub created_at: i64,
}

impl App {
    /// Offer a copy of a deck to another user (by `name#1234`, or a unique name).
    pub async fn share_deck(&self, access: &Access, deck_id: i64, recipient: &str) -> Result<OutgoingShare> {
        access.require_write()?;
        // Sending data to another account is an account-wide decision: a
        // token limited to some decks may not do it.
        access.require_all_decks()?;
        self.deck(access, deck_id).await?;
        let to = match self.find_user(recipient).await {
            Ok(u) => u,
            Err(AppError::NotFound) => {
                return Err(AppError::bad(format!("no user called '{}'", recipient.trim())));
            }
            Err(e) => return Err(e),
        };
        if to.id == access.user_id() {
            return Err(AppError::bad("you can't share a deck with yourself"));
        }
        sqlx::query(
            "INSERT INTO deck_shares (deck_id, from_user, to_user, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (deck_id, to_user) DO UPDATE SET created_at = excluded.created_at",
        )
        .bind(deck_id)
        .bind(access.user_id())
        .bind(to.id)
        .bind(now())
        .execute(&self.db)
        .await?;
        let mut shares = self.outgoing_shares(access, deck_id).await?;
        shares.retain(|s| s.to == to.handle());
        shares.pop().ok_or(AppError::NotFound)
    }

    pub async fn outgoing_shares(&self, access: &Access, deck_id: i64) -> Result<Vec<OutgoingShare>> {
        self.deck(access, deck_id).await?;
        Ok(sqlx::query_as(
            "SELECT s.id, s.deck_id, u.username || '#' || printf('%04d', u.discriminator) AS \"to\", s.created_at
             FROM deck_shares s JOIN users u ON u.id = s.to_user
             WHERE s.deck_id = ? AND s.from_user = ? ORDER BY s.created_at",
        )
        .bind(deck_id)
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?)
    }

    /// Withdraw a pending offer. Returns the deck it was for.
    pub async fn cancel_share(&self, access: &Access, share_id: i64) -> Result<i64> {
        access.require_write()?;
        let deck_id: i64 = sqlx::query_scalar("SELECT deck_id FROM deck_shares WHERE id = ? AND from_user = ?")
            .bind(share_id)
            .bind(access.user_id())
            .fetch_one(&self.db)
            .await?;
        self.deck(access, deck_id).await?;
        sqlx::query("DELETE FROM deck_shares WHERE id = ? AND from_user = ?")
            .bind(share_id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(deck_id)
    }

    pub async fn incoming_shares(&self, access: &Access) -> Result<Vec<IncomingShare>> {
        Ok(sqlx::query_as(
            "SELECT s.id, s.deck_id, d.name AS deck_name, d.description AS deck_description,
                (SELECT COUNT(*) FROM cards c WHERE c.deck_id = d.id) AS cards,
                u.username || '#' || printf('%04d', u.discriminator) AS \"from\", s.created_at
             FROM deck_shares s JOIN decks d ON d.id = s.deck_id JOIN users u ON u.id = s.from_user
             WHERE s.to_user = ? ORDER BY s.created_at DESC",
        )
        .bind(access.user_id())
        .fetch_all(&self.db)
        .await?)
    }

    pub async fn decline_share(&self, access: &Access, share_id: i64) -> Result<()> {
        access.require_write()?;
        let n = sqlx::query("DELETE FROM deck_shares WHERE id = ? AND to_user = ?")
            .bind(share_id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?
            .rows_affected();
        if n == 0 { Err(AppError::NotFound) } else { Ok(()) }
    }

    /// Copy the shared deck into the recipient's account.
    pub async fn accept_share(&self, access: &Access, share_id: i64) -> Result<Deck> {
        access.require_write()?;
        access.require_all_decks()?;
        let share = self
            .incoming_shares(access)
            .await?
            .into_iter()
            .find(|s| s.id == share_id)
            .ok_or(AppError::NotFound)?;
        #[derive(sqlx::FromRow)]
        struct Src {
            front: String,
            back: String,
            front_image: Option<String>,
            back_image: Option<String>,
            is_list: bool,
        }
        let cards: Vec<Src> = sqlx::query_as(
            "SELECT front, back, front_image, back_image, is_list FROM cards WHERE deck_id = ? ORDER BY position, id",
        )
        .bind(share.deck_id)
        .fetch_all(&self.db)
        .await?;

        let mut name = share.deck_name.clone();
        if self.deck_by_name(access, &name).await?.is_some() {
            name = format!("{} (from {})", share.deck_name, share.from);
            let base = name.clone();
            let mut i = 2;
            while self.deck_by_name(access, &name).await?.is_some() {
                name = format!("{base} {i}");
                i += 1;
            }
        }
        let deck = self
            .create_deck(access, &name, &share.deck_description, None)
            .await?;

        let mut images = std::collections::HashMap::new();
        for c in cards {
            let front_image = self.copy_image(access.user_id(), c.front_image, &mut images).await?;
            let back_image = self.copy_image(access.user_id(), c.back_image, &mut images).await?;
            let id = self
                .insert_card(access.user_id(), deck.id, &c.front, &c.back, front_image, back_image, "shared")
                .await?;
            if c.is_list {
                let copy = self.card(access, id).await?;
                self.sync_items(&copy, true).await?;
            }
        }
        sqlx::query("DELETE FROM deck_shares WHERE id = ? AND to_user = ?")
            .bind(share_id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(deck)
    }

    /// Give `user_id` their own copy of an image (images are per-user).
    async fn copy_image(
        &self,
        user_id: i64,
        id: Option<String>,
        done: &mut std::collections::HashMap<String, String>,
    ) -> Result<Option<String>> {
        let Some(id) = id else { return Ok(None) };
        if let Some(new) = done.get(&id) {
            return Ok(Some(new.clone()));
        }
        let new_id = uuid::Uuid::new_v4().simple().to_string();
        if tokio::fs::copy(self.media_path(&id), self.media_path(&new_id)).await.is_err() {
            return Ok(None);
        }
        sqlx::query(
            "INSERT INTO media (id, user_id, mime, width, height, bytes, created_at)
             SELECT ?, ?, mime, width, height, bytes, ? FROM media WHERE id = ?",
        )
        .bind(&new_id)
        .bind(user_id)
        .bind(now())
        .bind(&id)
        .execute(&self.db)
        .await?;
        done.insert(id, new_id.clone());
        Ok(Some(new_id))
    }
}
