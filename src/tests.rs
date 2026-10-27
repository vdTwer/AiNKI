//! Tests against an in-memory database: the service first, then the
//! website and MCP over HTTP (the `http` module).

use std::{str::FromStr, sync::Arc};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::{
    App, Config,
    auth::Access,
    error::AppError,
    now,
    scheduler::{CardState, Rating},
    service::{CardFilter, CardInput, PresetInput},
};

async fn app() -> Arc<App> {
    app_with(|_| {}).await
}

async fn app_with(configure: impl FnOnce(&mut Config)) -> Arc<App> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .foreign_keys(true);
    // One connection: every in-memory connection is its own database.
    let db = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    let dir = std::env::temp_dir().join(format!("ainki-test-{}", uuid::Uuid::new_v4().simple()));
    let mut config = Config {
        data_dir: dir,
        bind: "127.0.0.1:0".into(),
        allow_signup: true,
        secure_cookies: false,
        default_timezone: "UTC".into(),
        public_url: None,
    };
    configure(&mut config);
    Arc::new(App::new(db, config))
}

async fn user(app: &App, name: &str) -> Access {
    Access::web(app.create_user(name, "password123", "Europe/Berlin").await.unwrap())
}

#[tokio::test]
async fn study_flow_and_undo() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    assert_eq!(app.list_presets(&a).await.unwrap().len(), 5);
    let deck = app.create_deck(&a, "German", "", None).await.unwrap();
    for (f, b) in [("Hund", "dog"), ("Katze", "cat"), ("Maus", "mouse")] {
        app.create_card(&a, deck.id, f, b, None, None, false).await.unwrap();
    }

    let first = app.next_card(&a, None).await.unwrap().unwrap();
    assert_eq!(first.card.front, "Hund");
    assert_eq!(first.remaining, 3);
    assert_eq!(first.previews.len(), 4);

    let r = app
        .answer_card(&a, first.card.id, Rating::Good, Some(1500), Some(0))
        .await
        .unwrap();
    assert_eq!(r.card.card_state(), CardState::Review);
    assert!(r.card.stability.is_some());

    // A double submit is rejected.
    let dup = app.answer_card(&a, first.card.id, Rating::Good, None, Some(0)).await;
    assert!(matches!(dup, Err(AppError::Conflict(_))));

    let stats = app.stats(&a, None).await.unwrap();
    assert_eq!(stats.reviews_today, 1);
    assert_eq!(stats.new, 2);

    let restored = app.undo_last(&a).await.unwrap().unwrap();
    assert_eq!(restored.id, first.card.id);
    assert_eq!(restored.card_state(), CardState::New);
    assert_eq!(app.stats(&a, None).await.unwrap().reviews_today, 0);

    // Again keeps the card in learning; it comes back within the session.
    let r = app.answer_card(&a, first.card.id, Rating::Again, None, None).await.unwrap();
    assert_eq!(r.card.card_state(), CardState::Learning);
    assert_eq!(r.interval_secs, 600);
}

#[tokio::test]
async fn a_learning_card_studied_ahead_still_counts_as_due() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "German", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "Hund", "dog", None, None, false).await.unwrap();
    app.answer_card(&a, card.id, Rating::Again, None, None).await.unwrap();

    // Due in ten minutes, so studying shows it already; the home page and
    // the deck list must not call the day done meanwhile.
    assert_eq!(app.next_card(&a, None).await.unwrap().unwrap().card.id, card.id);
    let decks = app.deck_summaries(&a).await.unwrap();
    assert_eq!(decks[0].due_total(), 1);
}

#[tokio::test]
async fn list_cards_remember_each_item() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Literary Studies", "", None).await.unwrap();
    // Item by item needs a numbered list.
    assert!(app.create_card(&a, deck.id, "q", "just text", None, None, true).await.is_err());
    let back = "Cohen:\n1. Cultural body\n2. Always escapes\n3. Category crisis";
    let card = app.create_card(&a, deck.id, "Theses on monsters?", back, None, None, true).await.unwrap();
    assert!(card.is_list);
    assert_eq!(card.card_state(), CardState::New);
    let items = app.card_items(&card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["Cultural body", "Always escapes", "Category crisis"]);
    assert_eq!(crate::lists::asked(&card, &items, now()), [true, true, true]);

    // 1 of 3: the second and third were missed.
    let r = app.answer_list(&a, card.id, vec![1, 2], None, Some(0)).await.unwrap();
    let card = r.card;
    assert_eq!(card.card_state(), CardState::Learning);
    let items = app.card_items(&card).await.unwrap();
    assert!(items.iter().all(|i| i.reps == 1));
    assert!(items[1].due < items[0].due);
    // Next time only the missed ones are asked; the other shows as known.
    let asked = crate::lists::asked(&card, &items, now());
    assert_eq!(asked, [false, true, true]);
    assert!(app.answer_list(&a, card.id, vec![0], None, Some(card.reps)).await.is_err());
    let r = app.answer_list(&a, card.id, vec![], None, Some(card.reps)).await.unwrap();
    let items = app.card_items(&r.card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.reps).collect::<Vec<_>>(), [1, 2, 2]);
    let history = app.card_history(&a, card.id).await.unwrap();
    assert_eq!(history[0]["items_asked"], 3);
    assert_eq!(history[0]["items_recalled"], 1);
    assert_eq!(history[0]["rating"], "again");
    assert_eq!(history[1]["items_asked"], 2);
    // The optimizer learns from the items, not the card's summary.
    assert_eq!(app.optimizable_reviews(&a).await.unwrap(), 5);

    // Undo puts the items back too.
    app.undo_last(&a).await.unwrap().unwrap();
    let card = app.card(&a, card.id).await.unwrap();
    let items = app.card_items(&card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.reps).collect::<Vec<_>>(), [1, 1, 1]);

    // Editing keeps the memory of items that stay; a new one starts fresh.
    let edited = format!("{back}\n4. Gates of difference");
    let card = app
        .update_card(&a, card.id, CardInput { back: Some(edited), ..Default::default() })
        .await
        .unwrap();
    let items = app.card_items(&card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.reps).collect::<Vec<_>>(), [1, 1, 1, 0]);
    // A plain rating counts for every asked item.
    app.answer_card(&a, card.id, Rating::Again, None, None).await.unwrap();

    // Switching it off makes a plain card that keeps its schedule.
    let card = app
        .update_card(&a, card.id, CardInput { is_list: Some(false), ..Default::default() })
        .await
        .unwrap();
    assert!(!card.is_list);
    assert!(app.card_items(&card).await.unwrap().is_empty());
    assert_ne!(card.card_state(), CardState::New);
}

#[tokio::test]
async fn a_studied_card_turned_into_a_list_keeps_its_memory() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Math", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "Operations on F^n?", "1. Scalar multiplication\n2. Addition", None, None, false).await.unwrap();
    let card = app.answer_card(&a, card.id, Rating::Good, None, None).await.unwrap().card;
    let list = app
        .update_card(&a, card.id, CardInput { is_list: Some(true), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(list.due, card.due);
    assert_eq!(list.card_state(), card.card_state());
    let items = app.card_items(&list).await.unwrap();
    assert!(items.iter().all(|i| i.stability == card.stability && i.due == card.due));
    // A list in review asks for every item.
    assert_eq!(crate::lists::asked(&list, &items, now()), [true, true]);
}

#[tokio::test]
async fn daily_new_limit_is_respected() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let preset = app
        .create_preset(&a, Some("standard"), PresetInput { new_per_day: Some(1), ..Default::default() })
        .await
        .unwrap();
    let deck = app.create_deck(&a, "Small", "", Some(preset.id)).await.unwrap();
    for f in ["a", "b", "c"] {
        app.create_card(&a, deck.id, f, "x", None, None, false).await.unwrap();
    }
    let sc = app.next_card(&a, Some(deck.id)).await.unwrap().unwrap();
    app.answer_card(&a, sc.card.id, Rating::Good, None, None).await.unwrap();
    assert!(app.next_card(&a, Some(deck.id)).await.unwrap().is_none());
    let summary = &app.deck_summaries(&a).await.unwrap()[0];
    assert_eq!(summary.new_today, 0);
    assert_eq!(summary.new, 2);
}

#[tokio::test]
async fn token_permissions() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let other = user(&app, "eve").await;
    let d1 = app.create_deck(&a, "One", "", None).await.unwrap();
    let d2 = app.create_deck(&a, "Two", "", None).await.unwrap();
    let c2 = app.create_card(&a, d2.id, "secret", "x", None, None, false).await.unwrap();

    let (_, raw) = app
        .create_token(a.user_id(), "reader", false, false, Some(vec![d1.id]))
        .await
        .unwrap();
    let t = app.token_access(&raw).await.unwrap();
    assert_eq!(t.source(), "agent");
    assert_eq!(app.decks(&t).await.unwrap().len(), 1);
    assert!(matches!(app.card(&t, c2.id).await, Err(AppError::NotFound)));
    assert!(matches!(
        app.create_card(&t, d1.id, "q", "a", None, None, false).await,
        Err(AppError::Forbidden(_))
    ));
    assert!(matches!(app.create_deck(&t, "New", "", None).await, Err(AppError::Forbidden(_))));

    // Other users never see each other's data.
    assert!(app.decks(&other).await.unwrap().is_empty());
    assert!(matches!(app.card(&other, c2.id).await, Err(AppError::NotFound)));
    let (cards, total) = app.list_cards(&other, &CardFilter::default()).await.unwrap();
    assert!(cards.is_empty() && total == 0);

    // A writer agent's cards are labelled.
    let (tok, raw) = app.create_token(a.user_id(), "writer", true, true, None).await.unwrap();
    let w = app.token_access(&raw).await.unwrap();
    let c = app.create_card(&w, d1.id, "from agent", "", None, None, false).await.unwrap();
    assert_eq!(c.created_by, "agent");

    app.revoke_token(a.user_id(), tok.id).await.unwrap();
    assert!(app.token_access(&raw).await.is_err());
}

#[tokio::test]
async fn usernames_with_tags() {
    let app = app().await;
    let a = app.create_user("anna", "password123", "UTC").await.unwrap();
    let b = app.create_user("Anna", "password456", "UTC").await.unwrap();
    assert_ne!(a.discriminator, b.discriminator);
    assert!(matches!(app.authenticate("anna", "password123").await, Err(AppError::BadRequest(_))));
    assert_eq!(app.authenticate(&a.handle(), "password123").await.unwrap().id, a.id);
    assert!(app.authenticate(&b.handle(), "password123").await.is_err());
    assert!(app.create_user("x", "password123", "UTC").await.is_err());
    assert!(app.create_user("bob", "short", "UTC").await.is_err());
}

#[tokio::test]
async fn optimizer_fits_parameters() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Big", "", None).await.unwrap();
    let start = now() - 200 * 86_400;
    // Synthetic history: 300 cards, reviews at growing intervals with some lapses.
    for i in 0..300i64 {
        let card = app
            .create_card(&a, deck.id, &format!("q{i}"), "a", None, None, false)
            .await
            .unwrap();
        let mut t = start + i * 600;
        let mut ivl = 1;
        for n in 0..6 {
            let rating = if (i + n) % 7 == 0 { 1 } else { 3 };
            let state_before = if n == 0 { 0 } else { 2 };
            sqlx::query(
                "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                    elapsed_days, scheduled_secs, source)
                 VALUES (?, ?, ?, ?, ?, ?, 0, 0, 'web')",
            )
            .bind(a.user_id())
            .bind(card.id)
            .bind(deck.id)
            .bind(rating)
            .bind(t)
            .bind(state_before)
            .execute(&app.db)
            .await
            .unwrap();
            ivl = if rating == 1 { 1 } else { ivl * 3 };
            t += ivl * 86_400;
        }
    }
    let result = app.optimize_parameters(&a).await.unwrap();
    assert_eq!(result["parameters"].as_array().unwrap().len(), 21);
    let user = app.user(a.user_id()).await.unwrap();
    assert_eq!(user.params().len(), 21);

    app.reset_parameters(&Access::web(user)).await.unwrap();
    assert!(app.user(a.user_id()).await.unwrap().params().is_empty());
}

#[tokio::test]
async fn csv_round_trip_and_exam_date() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Csv", "", None).await.unwrap();
    let report = app.import_csv(&a, deck.id, "front;back\nuno;one\ndos;two\n").await.unwrap();
    assert_eq!(report.cards_imported, 2);
    let csv = app.export_csv(&a, deck.id).await.unwrap();
    assert_eq!(csv, "front,back\nuno,one\ndos,two\n");

    assert!(app.update_deck(&a, deck.id, None, None, None, Some("not a date")).await.is_err());
    let d = app
        .update_deck(&a, deck.id, None, None, None, Some("2030-01-31"))
        .await
        .unwrap();
    assert_eq!(d.exam_date.as_deref(), Some("2030-01-31"));
    let d = app.update_deck(&a, deck.id, None, None, None, Some("")).await.unwrap();
    assert!(d.exam_date.is_none());
}

#[tokio::test]
async fn sharing_gives_an_independent_copy() {
    let app = app().await;
    let kolja = user(&app, "kolja").await;
    let anna = user(&app, "anna").await;
    let deck = app.create_deck(&kolja, "Spanish", "verbs", None).await.unwrap();
    let img = app.store_image(&kolja, tiny_png()).await.unwrap();
    let c = app
        .create_card(&kolja, deck.id, "hablar", "to speak", Some(img.id.clone()), None, false)
        .await
        .unwrap();
    app.answer_card(&kolja, c.id, Rating::Good, None, None).await.unwrap();

    assert!(app.share_deck(&kolja, deck.id, &kolja.user.handle()).await.is_err());
    assert!(app.share_deck(&kolja, deck.id, "nobody#0001").await.is_err());
    let out = app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    assert_eq!(out.to, anna.user.handle());
    // Sharing again doesn't duplicate.
    app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    let incoming = app.incoming_shares(&anna).await.unwrap();
    assert_eq!(incoming.len(), 1);
    assert_eq!(incoming[0].cards, 1);
    assert_eq!(incoming[0].from, kolja.user.handle());

    // Anna already has a deck with that name.
    app.create_deck(&anna, "Spanish", "", None).await.unwrap();
    let copy = app.accept_share(&anna, incoming[0].id).await.unwrap();
    assert!(copy.name.starts_with("Spanish (from kolja#"));
    let (cards, _) = app
        .list_cards(&anna, &CardFilter { deck_id: Some(copy.id), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].card_state(), CardState::New);
    assert_eq!(cards[0].created_by, "shared");
    let anna_img = cards[0].front_image.clone().unwrap();
    assert_ne!(anna_img, img.id);
    assert!(app.media(anna.user_id(), &anna_img).await.is_ok());
    assert!(app.media(anna.user_id(), &img.id).await.is_err());
    assert!(app.incoming_shares(&anna).await.unwrap().is_empty());

    // Edits to the original don't reach the copy.
    app.update_card(&kolja, c.id, crate::service::CardInput { front: Some("comer".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(app.card(&anna, cards[0].id).await.unwrap().front, "hablar");

    // Decline and cancel.
    let s = app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    app.cancel_share(&kolja, s.id).await.unwrap();
    assert!(app.incoming_shares(&anna).await.unwrap().is_empty());
    let s = app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    assert!(app.decline_share(&kolja, s.id).await.is_err());
    app.decline_share(&anna, s.id).await.unwrap();
}

fn tiny_png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[tokio::test]
async fn oauth_flow() {
    use base64::Engine;
    use sha2::Digest;

    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "D", "", None).await.unwrap();
    app.create_card(&a, deck.id, "q", "a", None, None, false).await.unwrap();

    let reg = app
        .register_client(crate::oauth::RegistrationRequest {
            redirect_uris: vec!["https://claude.ai/api/mcp/auth_callback".into()],
            client_name: Some("Claude".into()),
            token_endpoint_auth_method: Some("none".into()),
        })
        .await
        .unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();
    assert!(reg.get("client_secret").is_none());
    let client = app.client(&client_id).await.unwrap().unwrap();

    let verifier = "a".repeat(50);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let redirect = "https://claude.ai/api/mcp/auth_callback";
    let code = app
        .create_auth_code(a.user_id(), &client, redirect, &challenge, true, false, None)
        .await
        .unwrap();

    // Wrong verifier fails and burns the code.
    assert!(app.exchange_code(&client_id, None, &code, Some(redirect), &"b".repeat(50)).await.is_err());
    assert!(app.exchange_code(&client_id, None, &code, Some(redirect), &verifier).await.is_err());

    let code = app
        .create_auth_code(a.user_id(), &client, redirect, &challenge, true, false, None)
        .await
        .unwrap();
    let tokens = app.exchange_code(&client_id, None, &code, Some(redirect), &verifier).await.unwrap();
    assert_eq!(tokens.scope, "read write");
    let agent = app.token_access(&tokens.access_token).await.unwrap();
    assert_eq!(app.decks(&agent).await.unwrap().len(), 1);
    assert!(agent.require_write().is_ok());
    assert!(agent.require_review().is_err());
    // The refresh token is not an access token.
    assert!(app.token_access(&tokens.refresh_token).await.is_err());

    // Refresh rotates: the old refresh token stops working. A quick retry
    // is just refused...
    let fresh = app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.unwrap();
    assert!(app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.is_err());
    assert!(app.token_access(&fresh.access_token).await.is_ok());
    // ...but reuse later means the token leaked: the connection is revoked.
    sqlx::query("UPDATE oauth_tokens SET used_at = used_at - 3600 WHERE kind = 'refresh_used'")
        .execute(&app.db)
        .await
        .unwrap();
    assert!(app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.is_err());
    assert!(app.token_access(&fresh.access_token).await.is_err());
    assert!(app.refresh_tokens(&client_id, None, &fresh.refresh_token).await.is_err());
    // Connect again for the rest of the test.
    let code = app
        .create_auth_code(a.user_id(), &client, redirect, &challenge, true, false, None)
        .await
        .unwrap();
    let tokens = app.exchange_code(&client_id, None, &code, Some(redirect), &verifier).await.unwrap();
    let fresh = app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.unwrap();
    assert!(app.token_access(&fresh.access_token).await.is_ok());
    // Another client can't use it.
    assert!(app.refresh_tokens("someone-else", None, &fresh.refresh_token).await.is_err());

    // The connection shows up next to tokens; disconnecting kills it.
    let list = app.list_tokens(a.user_id()).await.unwrap();
    assert_eq!(list.len(), 1, "the revoked connection is gone");
    assert_eq!(list[0].0.name, "Claude");
    assert_eq!(list[0].0.oauth_client_id.as_deref(), Some(client_id.as_str()));
    app.revoke_token(a.user_id(), list[0].0.id).await.unwrap();
    assert!(app.token_access(&fresh.access_token).await.is_err());
    assert!(app.refresh_tokens(&client_id, None, &fresh.refresh_token).await.is_err());

    // Confidential clients must present their secret.
    let reg = app
        .register_client(crate::oauth::RegistrationRequest {
            redirect_uris: vec!["https://example.com/cb".into()],
            client_name: None,
            token_endpoint_auth_method: Some("client_secret_post".into()),
        })
        .await
        .unwrap();
    let id = reg["client_id"].as_str().unwrap();
    let secret = reg["client_secret"].as_str().unwrap();
    let client = app.client(id).await.unwrap().unwrap();
    let code = app
        .create_auth_code(a.user_id(), &client, "https://example.com/cb", &challenge, false, true, Some(vec![deck.id]))
        .await
        .unwrap();
    assert!(app.exchange_code(id, Some("wrong"), &code, None, &verifier).await.is_err());
    let code = app
        .create_auth_code(a.user_id(), &client, "https://example.com/cb", &challenge, false, true, Some(vec![deck.id]))
        .await
        .unwrap();
    let t = app.exchange_code(id, Some(secret), &code, None, &verifier).await.unwrap();
    let agent = app.token_access(&t.access_token).await.unwrap();
    assert!(agent.require_write().is_err());
    assert!(agent.require_all_decks().is_err());

    // Bad registrations.
    assert!(app
        .register_client(crate::oauth::RegistrationRequest {
            redirect_uris: vec!["http://evil.example/cb".into()],
            client_name: None,
            token_endpoint_auth_method: None,
        })
        .await
        .is_err());
}

// ----- HTTP: security and the website flows ---------------------------------

mod http {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    async fn send(app: &Arc<App>, req: Request<Body>) -> axum::response::Response {
        crate::web::router(app.clone()).oneshot(req).await.unwrap()
    }

    fn form(path: &str, body: &str) -> axum::http::request::Builder {
        let _ = body;
        Request::post(path)
            .header(header::HOST, "cards.example.com")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
    }

    async fn login_cookie(app: &Arc<App>, name: &str) -> String {
        let user = app.find_user(name).await.unwrap();
        let token = app.create_session(user.id).await.unwrap();
        format!("{}={token}", app.config.session_cookie_name())
    }

    /// A same-site form post from a logged-in browser.
    async fn submit(app: &Arc<App>, cookie: &str, path: &str, body: &str) -> axum::response::Response {
        let req = form(path, "")
            .header(header::ORIGIN, "https://cards.example.com")
            .header(header::COOKIE, cookie)
            .body(Body::from(body.to_string()))
            .unwrap();
        send(app, req).await
    }

    /// The same for a multipart form (the card form), text fields only.
    async fn submit_multipart(app: &Arc<App>, cookie: &str, path: &str, fields: &[(&str, &str)]) -> axum::response::Response {
        let mut body = String::new();
        for (k, v) in fields {
            body += &format!("--XX\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n");
        }
        body += "--XX--\r\n";
        let req = Request::post(path)
            .header(header::HOST, "cards.example.com")
            .header(header::ORIGIN, "https://cards.example.com")
            .header(header::CONTENT_TYPE, "multipart/form-data; boundary=XX")
            .header(header::COOKIE, cookie)
            .body(Body::from(body))
            .unwrap();
        send(app, req).await
    }

    async fn text(res: axum::response::Response) -> String {
        String::from_utf8_lossy(&axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap()).into_owned()
    }

    fn location(res: &axum::response::Response) -> &str {
        res.headers()[header::LOCATION].to_str().unwrap()
    }

    /// The flash message a redirect carries, decoded.
    fn flash(res: &axum::response::Response) -> String {
        let c = res
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|c| c.to_str().unwrap().to_string())
            .find(|c| c.starts_with("ainki_flash="))
            .unwrap_or_default();
        let v = c.trim_start_matches("ainki_flash=").split(';').next().unwrap_or("");
        crate::security::percent_decode(v)
    }

    #[tokio::test]
    async fn card_create_and_edit_through_the_website() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Spanish", "", None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let d = deck.id.to_string();

        let res = submit_multipart(&app, &cookie, "/cards/new", &[("deck", &d), ("front", "el gato"), ("back", "the cat")]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&res), format!("/cards/new?deck={d}"));
        let (cards, _) = app.list_cards(&a, &CardFilter { deck_id: Some(deck.id), ..Default::default() }).await.unwrap();
        assert_eq!(cards.len(), 1);
        let id = cards[0].id;

        // A mistake shows the form again with what was typed.
        let res = submit_multipart(&app, &cookie, "/cards/new", &[("deck", &d), ("front", ""), ("back", "kept back")]).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert!(text(res).await.contains("kept back"));

        let res = submit_multipart(&app, &cookie, &format!("/cards/{id}"), &[("deck", &d), ("front", "el gato negro"), ("back", "the black cat")]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(app.card(&a, id).await.unwrap().front, "el gato negro");

        let res = submit(&app, &cookie, &format!("/cards/{id}/suspend"), "").await;
        assert_eq!(location(&res), format!("/cards/{id}"));
        assert!(app.card(&a, id).await.unwrap().suspended);
        let res = submit(&app, &cookie, &format!("/cards/{id}/delete"), "").await;
        assert_eq!(location(&res), format!("/decks/{d}"));
        assert_eq!(flash(&res), "Card deleted.");
        assert!(app.card(&a, id).await.is_err());
    }

    #[tokio::test]
    async fn deck_edit_stays_on_the_deck_settings() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Spanish", "", None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let path = format!("/decks/{}/edit", deck.id);

        let res = submit(&app, &cookie, &path, "name=Spanish+A1&description=&exam_date=").await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(location(&res), path);
        assert_eq!(flash(&res), "Deck saved.");
        assert_eq!(app.deck(&a, deck.id).await.unwrap().name, "Spanish A1");

        let res = submit(&app, &cookie, &path, "name=&description=still+here").await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert!(text(res).await.contains("still here"));
    }

    #[tokio::test]
    async fn presets_are_created_on_submit_and_deleted() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let before = app.list_presets(&a).await.unwrap().len();

        // Opening the form creates nothing.
        let res = send(&app, Request::get("/presets/new").header(header::COOKIE, cookie.clone()).body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(app.list_presets(&a).await.unwrap().len(), before);

        let body = "name=Exam&description=&desired_retention=93&new_per_day=30&reviews_per_day=300\
                    &learning_steps=1m+10m&relearning_steps=10m&max_interval=180";
        let res = submit(&app, &cookie, "/presets/new", body).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let presets = app.list_presets(&a).await.unwrap();
        let p = presets.iter().find(|p| p.name == "Exam").unwrap();
        // Retention is typed in percent.
        assert!((p.desired_retention - 0.93).abs() < 1e-9);

        let res = submit(&app, &cookie, &format!("/presets/{}/delete", p.id), "").await;
        assert_eq!(location(&res), "/presets");
        assert_eq!(app.list_presets(&a).await.unwrap().len(), before);
    }

    #[tokio::test]
    async fn cancelling_a_share_goes_back_to_the_deck() {
        let app = app().await;
        let kolja = user(&app, "kolja").await;
        let anna = user(&app, "anna").await;
        let deck = app.create_deck(&kolja, "Spanish", "", None).await.unwrap();
        let share = app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;

        let res = submit(&app, &cookie, &format!("/shares/{}/cancel", share.id), "").await;
        assert_eq!(location(&res), format!("/decks/{}/edit#share", deck.id));
        assert_eq!(flash(&res), "Share cancelled.");
        assert!(app.incoming_shares(&anna).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn logged_out_links_come_back_after_login() {
        let app = app().await;
        let res = send(&app, Request::get("/decks/7?q=perro").body(Body::empty()).unwrap()).await;
        assert_eq!(location(&res), "/login?next=%2Fdecks%2F7%3Fq%3Dperro");
        let res = send(&app, Request::get("/tokens").body(Body::empty()).unwrap()).await;
        assert_eq!(location(&res), "/access");
    }

    #[tokio::test]
    async fn security_headers_everywhere() {
        let app = app().await;
        let res = send(&app, Request::get("/login").body(Body::empty()).unwrap()).await;
        let h = res.headers();
        assert!(h["content-security-policy"].to_str().unwrap().contains("script-src 'self'"));
        assert!(h["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
        assert_eq!(h["x-frame-options"], "DENY");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert!(!h.contains_key(header::STRICT_TRANSPORT_SECURITY));

        let secure = app_with(|c| c.public_url = Some("https://cards.example.com".into())).await;
        let res = send(&secure, Request::get("/login").body(Body::empty()).unwrap()).await;
        assert!(res.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
        assert_eq!(secure.config.session_cookie_name(), "__Host-ainki_session");
    }

    #[tokio::test]
    async fn cross_origin_posts_are_blocked() {
        let app = app().await;
        user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let post = |origin: &str| {
            form("/decks/new", "")
                .header(header::ORIGIN, origin)
                .header(header::COOKIE, cookie.clone())
                .body(Body::from("name=Evil"))
                .unwrap()
        };
        assert_eq!(send(&app, post("https://evil.example")).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(send(&app, post("https://sub.cards.example.com")).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(send(&app, post("null")).await.status(), StatusCode::FORBIDDEN);
        assert_eq!(send(&app, post("https://cards.example.com")).await.status(), StatusCode::SEE_OTHER);
        // Token endpoints are meant to be called cross-origin.
        let res = send(
            &app,
            Request::post("/oauth/token")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("grant_type=refresh_token&client_id=x&refresh_token=y"))
                .unwrap(),
        )
        .await;
        assert_ne!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn login_is_throttled() {
        let app = app().await;
        user(&app, "kolja").await;
        let attempt = |pw: &str| {
            form("/login", "")
                .header("x-forwarded-for", "203.0.113.5")
                .body(Body::from(format!("username=kolja&password={pw}")))
                .unwrap()
        };
        for _ in 0..20 {
            assert_eq!(send(&app, attempt("wrong-password")).await.status(), StatusCode::UNAUTHORIZED);
        }
        // Even the right password is refused while throttled.
        assert_eq!(send(&app, attempt("password123")).await.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn oversized_bodies_are_rejected() {
        let app = app().await;
        let big = format!("username=a&password={}", "x".repeat(2 * 1024 * 1024));
        let res = send(&app, form("/login", "").body(Body::from(big)).unwrap()).await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn signup_closes_after_first_account() {
        let app = app_with(|c| c.allow_signup = false).await;
        let signup = |name: &str| form("/signup", "").body(Body::from(format!("username={name}&password=password123"))).unwrap();
        let first = send(&app, signup("owner")).await;
        assert_eq!(first.status(), StatusCode::SEE_OTHER);
        assert!(first.headers().get_all(header::SET_COOKIE).iter().any(|c| c.to_str().unwrap().contains("ainki_session=")));
        let second = send(&app, signup("stranger")).await;
        assert_eq!(second.headers()[header::LOCATION], "/login");
        assert!(app.find_user("stranger").await.is_err());
    }

    #[tokio::test]
    async fn flash_messages_come_from_cookies_only() {
        let app = app().await;
        user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        // A crafted ?msg= link shows nothing.
        let res = send(
            &app,
            Request::get("/?msg=Your%20account%20is%20locked")
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("locked"));
        // The flash cookie is shown once and then cleared.
        let res = send(
            &app,
            Request::get("/")
                .header(header::COOKIE, format!("{cookie}; ainki_flash=Saved%20it."))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert!(res.headers().get_all(header::SET_COOKIE).iter().any(|c| c.to_str().unwrap().starts_with("ainki_flash=;")));
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Saved it."));
    }

    #[tokio::test]
    async fn login_redirect_stays_on_site() {
        let app = app().await;
        user(&app, "kolja").await;
        for (next, expect) in [
            ("/decks/1", "/decks/1"),
            ("//evil.example", "/"),
            ("/\\evil.example", "/"),
            ("/%09/evil.example", "/%09/evil.example"),
            ("https://evil.example", "/"),
        ] {
            let body = format!("username=kolja&password=password123&next={}", crate::web::urlenc(next));
            let res = send(&app, form("/login", "").body(Body::from(body)).unwrap()).await;
            assert_eq!(res.headers()[header::LOCATION], expect, "next={next}");
        }
        // A literal tab inside the path is refused.
        let body = format!("username=kolja&password=password123&next={}", crate::web::urlenc("/\t/evil.example"));
        let res = send(&app, form("/login", "").body(Body::from(body)).unwrap()).await;
        assert_eq!(res.headers()[header::LOCATION], "/");
    }

    #[tokio::test]
    async fn desktop_look_is_opt_in_by_script_and_setting() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let page = |c: String| Request::get("/").header(header::COOKIE, c).body(Body::empty()).unwrap();
        let body = axum::body::to_bytes(send(&app, page(cookie.clone())).await.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        // The base page is the simple look; the hand-drawn one is only a
        // desktop-only stylesheet that theme.js may switch on.
        assert!(html.contains(r#"data-look="calm""#));
        assert!(html.contains(r#"<link rel="stylesheet" href="/style.css?v="#));
        assert!(html.contains(r#"" media="(min-width: 60em) and (hover: hover) and (pointer: fine)""#));
        assert!(html.contains(r#"href="/calm.css?v="#));
        assert!(html.contains(r#"<script src="/theme.js?v="#));
        assert!(!html.contains(r#"class="calm""#));

        app.update_settings(a.user_id(), "UTC", 4, false, 100, true, "simple").await.unwrap();
        let body = axum::body::to_bytes(send(&app, page(cookie)).await.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains(r#"data-look="simple""#));
        assert!(app.update_settings(a.user_id(), "UTC", 4, false, 100, true, "neon").await.is_err());

        for (path, kind) in [("/calm.css", "text/css"), ("/theme.js", "text/javascript"), ("/fonts/newsreader-latin.woff2", "font/woff2")] {
            let res = send(&app, Request::get(path).body(Body::empty()).unwrap()).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert!(res.headers()[header::CONTENT_TYPE].to_str().unwrap().starts_with(kind));
        }
        let res = send(&app, Request::get("/fonts/../ainki.db").body(Body::empty()).unwrap()).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn study_page_carries_both_sides() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Languages::Spanish", "", None).await.unwrap();
        app.create_card(&a, deck.id, "el perro", "the dog", None, None, false).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let res = send(&app, Request::get("/study").header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await;
        let html = String::from_utf8_lossy(&axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap()).into_owned();
        // The answer is already on the page, hidden until revealed.
        assert!(html.contains("the dog"));
        assert!(html.contains(r#"<section class="face back" id="answer" hidden>"#));
        assert!(html.contains(r#"action="/study/answer" hidden>"#));
        // Nested deck names are shown as a path, not with "::".
        assert!(!html.contains("Languages::Spanish"));
        assert!(html.contains(r#"<span class="deck-leaf">Spanish</span>"#));
        assert!(!html.contains("/offline"));
    }

    #[tokio::test]
    async fn list_cards_through_the_website() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Literary Studies", "", None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let d = deck.id.to_string();
        let back = "1. Cultural body\n2. Always escapes\n3. Category crisis";

        // The switch is only on the page once the back has a list.
        let res = send(&app, Request::get("/cards/new").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert!(text(res).await.contains(r#"<label class="check list-toggle" hidden>"#));
        let res = submit_multipart(&app, &cookie, "/cards/new", &[("deck", &d), ("front", "Theses?"), ("back", back), ("list", "1")]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let (cards, _) = app.list_cards(&a, &CardFilter { deck_id: Some(deck.id), ..Default::default() }).await.unwrap();
        let card = &cards[0];
        assert!(card.is_list);
        let res = send(&app, Request::get(format!("/cards/{}", card.id)).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        let html = text(res).await;
        assert!(html.contains(r#"<label class="check list-toggle"><input type="checkbox" name="list" value="1" checked>"#));
        assert!(html.contains("Always escapes"));

        // Studying: one row per item, one Done.
        let res = send(&app, Request::get("/study").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        let html = text(res).await;
        assert!(html.contains(r#"<form class="list-answer" method="post" action="/study/answer">"#));
        assert!(html.contains(r#"<input type="checkbox" name="recalled" value="2">"#));
        assert!(html.contains("0 of 3 remembered"));
        let res = submit(&app, &cookie, "/study/answer", &format!("card={}&reps=0&asked=0&asked=1&asked=2&recalled=1", card.id)).await;
        assert_eq!(location(&res), "/study");
        let history = app.card_history(&a, card.id).await.unwrap();
        assert_eq!(history[0]["items_recalled"], 1);
        let res = send(&app, Request::get(format!("/cards/{}", card.id)).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert!(text(res).await.contains("1 of 3 remembered"));
    }

    #[tokio::test]
    async fn mcp_requires_a_token() {
        let app = app().await;
        let res = send(
            &app,
            Request::post("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, "Bearer ainki_not_a_real_token")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(res.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().contains("resource_metadata="));
    }
}
