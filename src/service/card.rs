//! Cards: adding, editing, listing and moving them.

#[allow(
    clippy::wildcard_imports,
    reason = "an area of the service sees the service's shared imports and types"
)]
use super::*;

/// A card. Read one with `query_as`; read many with [`CARD_JSON`], which is
/// several times quicker than a row per card.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct Card {
    pub id: i64,
    #[serde(skip_serializing)]
    pub user_id: i64,
    pub deck_id: i64,
    pub front: String,
    pub back: String,
    pub front_image: Option<String>,
    pub back_image: Option<String>,
    #[serde(deserialize_with = "stored_state")]
    pub state: CardState,
    pub step: i64,
    pub stability: Option<f64>,
    pub difficulty: Option<f64>,
    pub due: i64,
    pub last_review: Option<i64>,
    pub reps: i64,
    pub lapses: i64,
    #[serde(deserialize_with = "stored_bool")]
    pub suspended: bool,
    pub position: i64,
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// How a multicard's back is split into items; `None` for a plain card.
    /// See `multi`.
    pub multi: Option<Split>,
    /// The brought card type it was written with, see `types`; `None` for
    /// the built-in kinds.
    #[serde(default)]
    pub type_id: Option<i64>,
}

/// Every column of a card as one JSON object, for reading many cards in one
/// row: `SELECT json_group_array({CARD_JSON}) FROM cards WHERE ...` decodes
/// as `Json<Vec<Card>>`. sqlx hands rows over one at a time, which costs
/// about 30 µs per card; this costs about 1.
pub(crate) const CARD_JSON: &str = "json_object('id', id, 'user_id', user_id, 'deck_id', deck_id, 'front', front,
    'back', back, 'front_image', front_image, 'back_image', back_image, 'state', state, 'step', step,
    'stability', stability, 'difficulty', difficulty, 'due', due, 'last_review', last_review, 'reps', reps,
    'lapses', lapses, 'suspended', suspended, 'position', position, 'created_by', created_by,
    'created_at', created_at, 'updated_at', updated_at, 'multi', multi, 'type_id', type_id)";

/// A card state as stored, a number (the API writes it as a word).
fn stored_state<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<CardState, D::Error> {
    let n = i64::deserialize(d)?;
    CardState::from_stored(n).ok_or_else(|| {
        serde::de::Error::custom(format!("unknown card state {n}"))
    })
}

/// Booleans as stored: 0 and 1.
fn stored_bool<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<bool, D::Error> {
    match i64::deserialize(d)? {
        0 => Ok(false),
        1 => Ok(true),
        n => Err(serde::de::Error::custom(format!("{n} is no boolean"))),
    }
}

impl Card {
    pub fn sched(&self) -> CardSched {
        CardSched {
            state: self.state,
            step: self.step,
            stability: self.stability.map(num::f64_f32),
            difficulty: self.difficulty.map(num::f64_f32),
            due: self.due,
            last_review: self.last_review,
            reps: self.reps,
            lapses: self.lapses,
        }
    }

    pub const fn card_state(&self) -> CardState {
        self.state
    }

    /// JSON shape used by the MCP API.
    pub fn to_json(&self, clock: &Clock, decay: f32) -> Value {
        let state = self.card_state();
        let due = if state == CardState::New {
            Value::Null
        } else {
            json!(crate::format::iso(self.due, clock))
        };
        json!({
            "id": self.id,
            "deck_id": self.deck_id,
            "front": self.front,
            "back": self.back,
            "front_image": self.front_image,
            "back_image": self.back_image,
            "state": state.as_str(),
            "suspended": self.suspended,
            "due": due,
            "is_due": state != CardState::New && self.due <= clock.now,
            "stability_days": self.stability.map(|s| (s * 100.0).round() / 100.0),
            "difficulty": self.difficulty.map(|d| (d * 100.0).round() / 100.0),
            "retrievability": scheduler::retrievability(&self.sched(), decay, clock)
                .map(|r| (f64::from(r) * 1000.0).round() / 1000.0),
            "reps": self.reps,
            "lapses": self.lapses,
            "created_by": self.created_by,
            "multi": self.multi.map(Split::as_str),
            "is_list": self.multi.is_some(),
        })
    }
}

#[derive(Debug, Default)]
pub struct CardInput {
    pub front: Option<String>,
    pub back: Option<String>,
    /// `Set(None)` clears the image.
    pub front_image: Patch<Option<String>>,
    pub back_image: Patch<Option<String>>,
    /// Learn the back item by item, split this way; `Set(None)` makes it a
    /// plain card again.
    pub multi: Patch<Option<Split>>,
}

/// A card to add. `NewCard::new(front, back)` for plain text; set the
/// other fields with struct update syntax.
#[derive(Debug, Default, Clone)]
pub struct NewCard<'a> {
    pub front: &'a str,
    pub back: &'a str,
    pub front_image: Option<String>,
    pub back_image: Option<String>,
    /// Learn the back item by item, split this way.
    pub multi: Option<Split>,
}

impl<'a> NewCard<'a> {
    pub fn new(front: &'a str, back: &'a str) -> Self {
        Self { front, back, ..Self::default() }
    }
}

/// A card's text, as the quick find reads it.
#[derive(Debug)]
pub struct CardText {
    pub id: i64,
    pub deck_id: i64,
    pub front: String,
    pub back: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct CardFilter {
    pub deck_id: Option<i64>,
    pub query: Option<String>,
    /// new | learning | review | relearning | due | suspended
    pub state: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl App {
    /// # Errors
    ///
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn card(&self, access: &Access, id: i64) -> Result<Card> {
        let card: Card =
            sqlx::query_as("SELECT * FROM cards WHERE id = ? AND user_id = ?")
                .bind(id)
                .bind(access.user_id())
                .fetch_one(&self.db)
                .await?;
        access.require_deck(card.deck_id)?;
        Ok(card)
    }

    async fn check_media(
        &self,
        access: &Access,
        id: Option<&str>,
    ) -> Result<()> {
        if let Some(id) = id {
            let found: Option<String> = sqlx::query_scalar(
                "SELECT id FROM media WHERE id = ? AND user_id = ?",
            )
            .bind(id)
            .bind(access.user_id())
            .fetch_optional(&self.db)
            .await?;
            if found.is_none() {
                return Err(AppError::bad(format!("unknown image id '{id}'")));
            }
        }
        Ok(())
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the deck isn't the user's or lies outside the token's decks.
    /// - `BadRequest` if the front has neither text nor an image, a side is too long, or an image id isn't the
    ///   user's.
    /// - `Internal` if the database fails.
    pub async fn create_card(
        &self,
        access: &Access,
        deck_id: i64,
        new: NewCard<'_>,
    ) -> Result<Card> {
        access.require_write()?;
        self.deck(access, deck_id).await?;
        if new.front.trim().is_empty() && new.front_image.is_none() {
            return Err(AppError::bad("the front needs text or an image"));
        }
        let (front, back) =
            (markdown::tidy(new.front), markdown::tidy(new.back));
        if let Some(how) = new.multi {
            crate::multi::split(&back, how)
                .ok_or_else(|| crate::multi::no_items(how))?;
        }
        check_len("front", &front, MAX_CARD_TEXT)?;
        check_len("back", &back, MAX_CARD_TEXT)?;
        self.check_media(access, new.front_image.as_deref()).await?;
        self.check_media(access, new.back_image.as_deref()).await?;
        let source = match access.actor {
            Actor::Web => "web",
            Actor::Agent { .. } => "agent",
        };
        let tidy = NewCard { front: &front, back: &back, ..new };
        let id =
            self.insert_card(access.user_id(), deck_id, &tidy, source).await?;
        let card = self.card(access, id).await?;
        if tidy.multi.is_some() {
            self.sync_items(&card, tidy.multi).await?;
            return self.card(access, id).await;
        }
        Ok(card)
    }

    /// Stores a card's text and images unchecked, for imports and copies.
    /// It does not split a multicard: the caller syncs its items.
    pub(crate) async fn insert_card(
        &self,
        user_id: i64,
        deck_id: i64,
        new: &NewCard<'_>,
        created_by: &str,
    ) -> Result<i64> {
        let t = now();
        // Imports and copies are not validated field by field; cap them here.
        let cap = |x: &str| -> String {
            x.trim().chars().take(MAX_CARD_TEXT).collect()
        };
        Ok(sqlx::query_scalar(
            "INSERT INTO cards (user_id, deck_id, front, back, front_image, back_image, position,
                created_by, created_at, updated_at, suspended, held)
             VALUES (?, ?, ?, ?, ?, ?,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM cards WHERE user_id = ?), ?, ?, ?,
                (SELECT status != 'active' FROM decks WHERE id = ?),
                (SELECT status != 'active' FROM decks WHERE id = ?))
             RETURNING id",
        )
        .bind(user_id)
        .bind(deck_id)
        .bind(cap(new.front))
        .bind(cap(new.back))
        .bind(&new.front_image)
        .bind(&new.back_image)
        .bind(user_id)
        .bind(created_by)
        .bind(t)
        .bind(t)
        .bind(deck_id)
        .bind(deck_id)
        .fetch_one(&self.db)
        .await?)
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `BadRequest` if the front has neither text nor an image, a side is too long, or an image id isn't the
    ///   user's.
    /// - `Internal` if the database fails.
    pub async fn update_card(
        &self,
        access: &Access,
        id: i64,
        input: CardInput,
    ) -> Result<Card> {
        access.require_write()?;
        let mut card = self.card(access, id).await?;
        if card.type_id.is_some()
            && (input.front.is_some()
                || input.back.is_some()
                || input.multi != Patch::Keep)
        {
            return Err(AppError::bad(
                "this card's type writes its sides; change its fields instead",
            ));
        }
        let was = card.multi;
        let multi = input.multi.or(was);
        if let Some(f) = input.front {
            card.front = markdown::tidy(f.trim());
        }
        if let Some(b) = input.back {
            card.back = markdown::tidy(b.trim());
        }
        if let Patch::Set(img) = input.front_image {
            self.check_media(access, img.as_deref()).await?;
            card.front_image = img;
        }
        if let Patch::Set(img) = input.back_image {
            self.check_media(access, img.as_deref()).await?;
            card.back_image = img;
        }
        if card.front.is_empty() && card.front_image.is_none() {
            return Err(AppError::bad("the front needs text or an image"));
        }
        check_len("front", &card.front, MAX_CARD_TEXT)?;
        check_len("back", &card.back, MAX_CARD_TEXT)?;
        if let Some(how) = multi {
            crate::multi::split(&card.back, how)
                .ok_or_else(|| crate::multi::no_items(how))?;
        }
        sqlx::query(
            "UPDATE cards SET front = ?, back = ?, front_image = ?, back_image = ?, updated_at = ?
             WHERE id = ? AND user_id = ?",
        )
        .bind(&card.front)
        .bind(&card.back)
        .bind(&card.front_image)
        .bind(&card.back_image)
        .bind(now())
        .bind(id)
        .bind(access.user_id())
        .execute(&self.db)
        .await?;
        if multi.is_some() || was.is_some() {
            self.sync_items(&card, multi).await?;
        }
        self.card(access, id).await
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn delete_card(&self, access: &Access, id: i64) -> Result<()> {
        access.require_write()?;
        self.card(access, id).await?;
        sqlx::query("DELETE FROM cards WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks, or the same for the target
    ///   deck.
    /// - `Internal` if the database fails.
    pub async fn move_card(
        &self,
        access: &Access,
        id: i64,
        deck_id: i64,
    ) -> Result<Card> {
        access.require_write()?;
        self.card(access, id).await?;
        // A card moved into a resting deck rests with it; one the deck held
        // wakes when it moves to a deck being studied. Suspended by hand
        // stays suspended.
        let rests = !self.deck(access, deck_id).await?.status.is_active();
        sqlx::query(
            "UPDATE cards SET deck_id = ?, updated_at = ?,
                suspended = CASE WHEN ? THEN 1 WHEN held = 1 THEN 0 ELSE suspended END,
                held = CASE WHEN ? THEN (held = 1 OR suspended = 0) ELSE 0 END
             WHERE id = ? AND user_id = ?",
        )
        .bind(deck_id)
        .bind(now())
        .bind(rests)
        .bind(rests)
        .bind(id)
        .bind(access.user_id())
        .execute(&self.db)
        .await?;
        self.card(access, id).await
    }

    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn set_suspended(
        &self,
        access: &Access,
        id: i64,
        suspended: bool,
    ) -> Result<Card> {
        access.require_write()?;
        self.card(access, id).await?;
        // By hand: no longer the deck's to wake.
        sqlx::query("UPDATE cards SET suspended = ?, held = 0, updated_at = ? WHERE id = ? AND user_id = ?")
            .bind(suspended)
            .bind(now())
            .bind(id)
            .bind(access.user_id())
            .execute(&self.db)
            .await?;
        self.card(access, id).await
    }

    /// Forget all progress; the card becomes new again. Review history is
    /// kept but the optimizer ignores everything before the reset.
    ///
    /// # Errors
    ///
    /// - `Forbidden` if the token is read-only.
    /// - `NotFound` if the card isn't the user's or lies outside the token's decks.
    /// - `Internal` if the database fails.
    pub async fn reset_card(&self, access: &Access, id: i64) -> Result<Card> {
        access.require_write()?;
        let card = self.card(access, id).await?;
        let mut tx = self.db.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE cards SET state = 0, step = 0, stability = NULL, difficulty = NULL, due = 0,
                last_review = NULL, reps = 0, lapses = 0, updated_at = ? WHERE id = ? AND user_id = ?",
        )
        .bind(now())
        .bind(id)
        .bind(access.user_id())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE card_items SET state = 0, step = 0, stability = NULL, difficulty = NULL, due = 0,
                last_review = NULL, reps = 0, lapses = 0 WHERE card_id = ?",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, source)
             VALUES (?, ?, ?, 0, ?, ?, 0, 0, 'manual')",
        )
        .bind(access.user_id())
        .bind(id)
        .bind(card.deck_id)
        .bind(now())
        .bind(card.state)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.card(access, id).await
    }

    /// Every visible card's front and the start of its back, for the quick
    /// find, newest first. Only the text: the find scores it in memory.
    ///
    /// # Errors
    ///
    /// `Internal` if the database fails.
    pub async fn card_texts(&self, access: &Access) -> Result<Vec<CardText>> {
        // One JSON array: quick find reads every card, and sqlx hands rows
        // over one at a time.
        let Json(all): Json<Vec<(i64, i64, String, String)>> = sqlx::query_scalar(
            "SELECT json_group_array(json_array(id, deck_id, front, back)) FROM (
                 SELECT id, deck_id, front, substr(back, 1, 300) AS back FROM cards
                 WHERE user_id = ? ORDER BY created_at DESC, id DESC)",
        )
        .bind(access.user_id())
        .fetch_one(&self.db)
        .await?;
        Ok(all
            .into_iter()
            .filter(|c| access.can_see_deck(c.1))
            .map(|(id, deck_id, front, back)| CardText {
                id,
                deck_id,
                front,
                back,
            })
            .collect())
    }

    /// # Errors
    ///
    /// - `NotFound` if a deck is given that the access doesn't see.
    /// - `BadRequest` if the state filter is no state.
    /// - `Internal` if the database fails.
    pub async fn list_cards(
        &self,
        access: &Access,
        filter: &CardFilter,
    ) -> Result<(Vec<Card>, i64)> {
        let deck_ids: Vec<i64> = match filter.deck_id {
            Some(id) => {
                self.deck(access, id).await?;
                vec![id]
            }
            None => {
                self.decks(access).await?.into_iter().map(|d| d.id).collect()
            }
        };
        if deck_ids.is_empty() {
            return Ok((vec![], 0));
        }
        let mut cond = String::from(
            "user_id = ? AND deck_id IN (SELECT value FROM json_each(?))",
        );
        let query =
            filter.query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        if query.is_some() {
            cond.push_str(
                " AND (front LIKE ? ESCAPE '\\' OR back LIKE ? ESCAPE '\\')",
            );
        }
        match filter.state.as_deref().unwrap_or("") {
            "" | "all" => {}
            "new" => cond.push_str(" AND state = 0 AND suspended = 0"),
            "learning" => {
                cond.push_str(" AND state IN (1, 3) AND suspended = 0")
            }
            "review" => cond.push_str(" AND state = 2 AND suspended = 0"),
            "relearning" => cond.push_str(" AND state = 3 AND suspended = 0"),
            "due" => {
                cond.push_str(" AND state != 0 AND suspended = 0 AND due <= ?")
            }
            "suspended" => cond.push_str(" AND suspended = 1"),
            other => {
                return Err(AppError::bad(format!(
                    "unknown state filter '{other}'"
                )));
            }
        }
        let like = query.map(|q| {
            format!(
                "%{}%",
                q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
            )
        });
        let due_bound = filter.state.as_deref() == Some("due");
        let limit = filter.limit.unwrap_or(50).clamp(1, 500);
        let offset = filter.offset.unwrap_or(0).max(0);

        let count_sql = format!("SELECT COUNT(*) FROM cards WHERE {cond}");
        let list_sql = format!(
            "SELECT * FROM cards WHERE {cond} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        );
        let mut count_q =
            sqlx::query_scalar::<_, i64>(&count_sql).bind(access.user_id());
        let mut list_q =
            sqlx::query_as::<_, Card>(&list_sql).bind(access.user_id());
        count_q = count_q.bind(id_list(&deck_ids));
        list_q = list_q.bind(id_list(&deck_ids));
        if let Some(l) = &like {
            count_q = count_q.bind(l.clone()).bind(l.clone());
            list_q = list_q.bind(l.clone()).bind(l.clone());
        }
        if due_bound {
            count_q = count_q.bind(now());
            list_q = list_q.bind(now());
        }
        let total = count_q.fetch_one(&self.db).await?;
        let cards = list_q.bind(limit).bind(offset).fetch_all(&self.db).await?;
        Ok((cards, total))
    }
}
