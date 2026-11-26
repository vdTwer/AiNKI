//! The REST API: create and read decks and cards with a Bearer token.

use super::*;
use serde_json::{Value, json};

/// Call the API. Returns the status and the JSON body (`Null` if empty).
async fn call(
    app: &Arc<App>,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header(header::HOST, "cards.example.com");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = send(app, req).await;
    let status = res.status();
    let body = text(res).await;
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

async fn token(app: &Arc<App>, a: &Access, write: bool) -> String {
    app.create_token(a.user_id(), "api", write, false, None).await.unwrap().1
}

#[tokio::test]
async fn create_and_read_decks_and_cards() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let t = token(&app, &a, true).await;
    let t = Some(t.as_str());

    let (s, deck) = call(&app, t, "POST", "/api/v1/decks", Some(json!({ "name": "German" }))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(deck["name"], "German");
    let id = deck["id"].as_i64().unwrap();

    let (s, decks) = call(&app, t, "GET", "/api/v1/decks", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(decks.as_array().unwrap().len(), 1);
    assert_eq!(decks[0]["id"], id);
    assert_eq!(decks[0]["total"], 0);

    let (s, got) = call(&app, t, "GET", &format!("/api/v1/decks/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["name"], "German");

    let (s, card) =
        call(&app, t, "POST", &format!("/api/v1/decks/{id}/cards"), Some(json!({ "front": "Hund", "back": "dog" })))
            .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        (card["front"].clone(), card["back"].clone(), card["deck_id"].clone()),
        (json!("Hund"), json!("dog"), json!(id))
    );
    let card_id = card["id"].as_i64().unwrap();

    let (s, multi) = call(
        &app,
        t,
        "POST",
        &format!("/api/v1/decks/{id}/cards"),
        Some(json!({ "front": "Pets", "back": "- dog\n- cat", "multi": "list" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(multi["items"].as_array().unwrap().len(), 2);

    let (s, list) = call(&app, t, "GET", &format!("/api/v1/decks/{id}/cards?limit=1"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["total"], 2);
    assert_eq!(list["cards"].as_array().unwrap().len(), 1);

    let (s, got) = call(&app, t, "GET", &format!("/api/v1/cards/{card_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["back"], "dog");
}

#[tokio::test]
async fn errors_map_to_statuses() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let b = user(&app, "anna").await;
    let deck = app.create_deck(&a, "German", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, NewCard::new("Hund", "dog")).await.unwrap();
    let w = token(&app, &a, true).await;
    let w = Some(w.as_str());
    let ro = token(&app, &a, false).await;
    let ro = Some(ro.as_str());
    let anna = token(&app, &b, true).await;
    let anna = Some(anna.as_str());

    // No or a wrong token.
    assert_eq!(call(&app, None, "GET", "/api/v1/decks", None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, Some("ainki_nope"), "GET", "/api/v1/decks", None).await.0, StatusCode::UNAUTHORIZED);

    // Someone else's things don't exist.
    assert_eq!(call(&app, anna, "GET", "/api/v1/decks", None).await.1, json!([]));
    for path in [
        format!("/api/v1/decks/{}", deck.id),
        format!("/api/v1/decks/{}/cards", deck.id),
        format!("/api/v1/cards/{}", card.id),
    ] {
        assert_eq!(call(&app, anna, "GET", &path, None).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    let new_card = json!({ "front": "a", "back": "b" });
    let (s, _) = call(&app, anna, "POST", &format!("/api/v1/decks/{}/cards", deck.id), Some(new_card.clone())).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A read-only token may look, not write.
    assert_eq!(call(&app, ro, "GET", "/api/v1/decks", None).await.0, StatusCode::OK);
    let (s, e) = call(&app, ro, "POST", "/api/v1/decks", Some(json!({ "name": "x" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(e["error"].as_str().unwrap().contains("forbidden"), "{e}");
    let (s, _) = call(&app, ro, "POST", &format!("/api/v1/decks/{}/cards", deck.id), Some(new_card)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Bad input and taken names.
    let (s, e) = call(&app, w, "POST", "/api/v1/decks", Some(json!({ "name": " " }))).await;
    assert_eq!((s, e["error"].clone()), (StatusCode::BAD_REQUEST, json!("deck name is empty")));
    let (s, _) = call(&app, w, "POST", "/api/v1/decks", Some(json!({ "name": "German" }))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = call(&app, w, "POST", &format!("/api/v1/decks/{}/cards", deck.id), Some(json!({ "back": "b" }))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(call(&app, w, "GET", "/api/v1/decks/nope", None).await.0, StatusCode::BAD_REQUEST);
}
