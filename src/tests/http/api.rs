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
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "cards.example.com");
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
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

    let (s, deck) = call(
        &app,
        t,
        "POST",
        "/api/v1/decks",
        Some(json!({ "name": "German" })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(deck["name"], "German");
    let id = deck["id"].as_i64().unwrap();

    let (s, decks) = call(&app, t, "GET", "/api/v1/decks", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(decks.as_array().unwrap().len(), 1);
    assert_eq!(decks[0]["id"], id);
    assert_eq!(decks[0]["total"], 0);

    let (s, got) =
        call(&app, t, "GET", &format!("/api/v1/decks/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["name"], "German");

    let (s, card) = call(
        &app,
        t,
        "POST",
        &format!("/api/v1/decks/{id}/cards"),
        Some(json!({ "front": "Hund", "back": "dog" })),
    )
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
        Some(
            json!({ "front": "Pets", "back": "- dog\n- cat", "multi": "list" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(multi["items"].as_array().unwrap().len(), 2);

    let (s, list) = call(
        &app,
        t,
        "GET",
        &format!("/api/v1/decks/{id}/cards?limit=1"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["total"], 2);
    assert_eq!(list["cards"].as_array().unwrap().len(), 1);

    let (s, got) =
        call(&app, t, "GET", &format!("/api/v1/cards/{card_id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got["back"], "dog");
}

#[tokio::test]
async fn errors_map_to_statuses() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let b = user(&app, "anna").await;
    let deck = app.create_deck(&a, "German", "", None).await.unwrap();
    let card = app
        .create_card(&a, deck.id, NewCard::new("Hund", "dog"))
        .await
        .unwrap();
    let w = token(&app, &a, true).await;
    let w = Some(w.as_str());
    let ro = token(&app, &a, false).await;
    let ro = Some(ro.as_str());
    let anna = token(&app, &b, true).await;
    let anna = Some(anna.as_str());

    // No or a wrong token.
    assert_eq!(
        call(&app, None, "GET", "/api/v1/decks", None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, Some("ainki_nope"), "GET", "/api/v1/decks", None).await.0,
        StatusCode::UNAUTHORIZED
    );

    // Someone else's things don't exist.
    assert_eq!(
        call(&app, anna, "GET", "/api/v1/decks", None).await.1,
        json!([])
    );
    for path in [
        format!("/api/v1/decks/{}", deck.id),
        format!("/api/v1/decks/{}/cards", deck.id),
        format!("/api/v1/cards/{}", card.id),
    ] {
        assert_eq!(
            call(&app, anna, "GET", &path, None).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let new_card = json!({ "front": "a", "back": "b" });
    let (s, _) = call(
        &app,
        anna,
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(new_card.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A read-only token may look, not write.
    assert_eq!(
        call(&app, ro, "GET", "/api/v1/decks", None).await.0,
        StatusCode::OK
    );
    let (s, e) =
        call(&app, ro, "POST", "/api/v1/decks", Some(json!({ "name": "x" })))
            .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(e["error"].as_str().unwrap().contains("forbidden"), "{e}");
    let (s, _) = call(
        &app,
        ro,
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(new_card),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Bad input and taken names.
    let (s, e) =
        call(&app, w, "POST", "/api/v1/decks", Some(json!({ "name": " " })))
            .await;
    assert_eq!(
        (s, e["error"].clone()),
        (StatusCode::BAD_REQUEST, json!("deck name is empty"))
    );
    let (s, _) = call(
        &app,
        w,
        "POST",
        "/api/v1/decks",
        Some(json!({ "name": "German" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = call(
        &app,
        w,
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(json!({ "back": "b" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        call(&app, w, "GET", "/api/v1/decks/nope", None).await.0,
        StatusCode::BAD_REQUEST
    );
}

/// Send raw bytes, as the browser extension does with a picture.
async fn upload(
    app: &Arc<App>,
    token: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let req = Request::post("/api/v1/images")
        .header(header::HOST, "cards.example.com")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "image/png")
        .body(Body::from(bytes))
        .unwrap();
    let res = send(app, req).await;
    let status = res.status();
    (status, serde_json::from_str(&text(res).await).unwrap_or(Value::Null))
}

#[tokio::test]
async fn images_upload_and_go_on_new_cards() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Birds", "", None).await.unwrap();
    let w = token(&app, &a, true).await;
    let ro = token(&app, &a, false).await;

    let (s, img) = upload(&app, &w, tiny_png()).await;
    assert_eq!(s, StatusCode::CREATED, "{img}");
    assert_eq!(
        (img["width"].clone(), img["height"].clone()),
        (json!(4), json!(4))
    );
    let id = img["image_id"].as_str().unwrap().to_string();

    let (s, card) = call(
        &app,
        Some(&w),
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(json!({ "front": "", "back": "Robin", "front_image_id": id })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{card}");
    assert_eq!(card["front_image"], json!(id));

    let (s, card) = call(
        &app,
        Some(&w),
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(json!({ "front": "Which bird?", "back_image_id": id })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{card}");
    assert_eq!(card["back_image"], json!(id));

    // Photos are bigger than the API's usual 256 KB body limit.
    let noise = image::RgbImage::from_fn(700, 700, |x, y| {
        let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503))
            .to_le_bytes();
        image::Rgb([v[0], v[1], v[2]])
    });
    let mut big = Vec::new();
    noise
        .write_to(&mut std::io::Cursor::new(&mut big), image::ImageFormat::Png)
        .unwrap();
    assert!(big.len() > 1024 * 1024, "{}", big.len());
    assert_eq!(upload(&app, &w, big).await.0, StatusCode::CREATED);

    // Read-only tokens can't upload; junk is refused; ids must be real.
    assert_eq!(upload(&app, &ro, tiny_png()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        upload(&app, &w, b"not an image".to_vec()).await.0,
        StatusCode::BAD_REQUEST
    );
    let (s, _) = call(
        &app,
        Some(&w),
        "POST",
        &format!("/api/v1/decks/{}/cards", deck.id),
        Some(json!({ "front": "x", "back_image_id": "0123456789abcdef0123456789abcdef" })),
    )
    .await;
    assert!(s == StatusCode::NOT_FOUND || s == StatusCode::BAD_REQUEST, "{s}");
}
