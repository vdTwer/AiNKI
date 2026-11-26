//! A small REST API: create and read decks and cards, and upload images
//! for new cards. Nothing else.
//!
//! It takes the same API tokens as the MCP server (Settings → Agent access)
//! and calls the same service functions, so a token sees only its user's
//! decks and may write only if it was given write access.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    auth::Access,
    error::AppError,
    multi::Split,
    service::{CardFilter, NewCard},
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/decks", get(list_decks).post(create_deck))
        .route("/api/v1/decks/{id}", get(get_deck))
        .route("/api/v1/decks/{id}/cards", get(list_cards).post(create_card))
        .route("/api/v1/cards/{id}", get(get_card))
        .route(
            "/api/v1/images",
            post(upload_image).layer(DefaultBodyLimit::max(
                crate::media::MAX_UPLOAD_BYTES + 64 * 1024,
            )),
        )
}

/// An [`AppError`] as a status and `{ "error": message }`.
pub struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Internal(e) => {
                tracing::error!("API request failed: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let msg = match self.0 {
            AppError::Internal(_) => "internal server error".to_string(),
            AppError::Unauthorized => {
                "missing or invalid token: send Authorization: Bearer <token>"
                    .to_string()
            }
            e => e.to_string(),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

/// The caller, from the Bearer token.
pub struct Caller(Access);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token =
            crate::mcp::bearer(&parts.headers).ok_or(AppError::Unauthorized)?;
        app.token_access(token)
            .await
            .map(Self)
            .map_err(|_| AppError::Unauthorized.into())
    }
}

fn to_json(v: impl serde::Serialize) -> ApiResult {
    Ok(Json(serde_json::to_value(v).map_err(|e| AppError::Internal(e.into()))?))
}

async fn list_decks(
    State(app): State<AppState>,
    Caller(a): Caller,
) -> ApiResult {
    to_json(app.deck_summaries(&a).await?)
}

async fn get_deck(
    State(app): State<AppState>,
    Caller(a): Caller,
    Path(id): Path<i64>,
) -> ApiResult {
    to_json(app.deck(&a, id).await?)
}

#[derive(Deserialize)]
struct NewDeck {
    name: String,
    #[serde(default)]
    description: String,
    preset_id: Option<i64>,
}

async fn create_deck(
    State(app): State<AppState>,
    Caller(a): Caller,
    Json(d): Json<NewDeck>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let deck =
        app.create_deck(&a, &d.name, &d.description, d.preset_id).await?;
    Ok((StatusCode::CREATED, to_json(deck)?))
}

#[derive(Deserialize)]
struct CardPage {
    query: Option<String>,
    state: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_cards(
    State(app): State<AppState>,
    Caller(a): Caller,
    Path(id): Path<i64>,
    Query(p): Query<CardPage>,
) -> ApiResult {
    // An empty list would hide that the deck isn't there.
    app.deck(&a, id).await?;
    let filter = CardFilter {
        deck_id: Some(id),
        query: p.query,
        state: p.state,
        limit: p.limit,
        offset: p.offset,
    };
    Ok(Json(app.cards_json(&a, &filter).await?))
}

async fn get_card(
    State(app): State<AppState>,
    Caller(a): Caller,
    Path(id): Path<i64>,
) -> ApiResult {
    let card = app.card(&a, id).await?;
    Ok(Json(app.card_json(&a, &card, false).await?))
}

#[derive(Deserialize)]
struct NewCardBody {
    front: String,
    #[serde(default)]
    back: String,
    /// marked, list, sections or parts
    multi: Option<Split>,
    /// From `POST /api/v1/images`.
    front_image_id: Option<String>,
    back_image_id: Option<String>,
}

async fn create_card(
    State(app): State<AppState>,
    Caller(a): Caller,
    Path(id): Path<i64>,
    Json(c): Json<NewCardBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let new = NewCard {
        multi: c.multi,
        front_image: c.front_image_id,
        back_image: c.back_image_id,
        ..NewCard::new(&c.front, &c.back)
    };
    let card = app.create_card(&a, id, new).await?;
    Ok((StatusCode::CREATED, Json(app.card_json(&a, &card, false).await?)))
}

/// The request body is the image itself (PNG, JPEG, GIF or WebP); it is
/// stored as a PNG of at most 1200 px a side.
async fn upload_image(
    State(app): State<AppState>,
    Caller(a): Caller,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let m = app.store_image(&a, body.to_vec()).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "image_id": m.id, "width": m.width, "height": m.height })),
    ))
}
