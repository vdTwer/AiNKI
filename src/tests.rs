//! Service-level tests against an in-memory database.

use std::{str::FromStr, sync::Arc};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::{
    App, Config,
    auth::Access,
    error::AppError,
    now,
    scheduler::{CardState, Rating},
    service::{CardFilter, PresetInput},
};

async fn app() -> Arc<App> {
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
    Arc::new(App {
        db,
        config: Config {
            data_dir: dir,
            bind: "127.0.0.1:0".into(),
            allow_signup: true,
            secure_cookies: false,
            default_timezone: "UTC".into(),
            public_url: None,
        },
    })
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
        app.create_card(&a, deck.id, f, b, None, None).await.unwrap();
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
async fn daily_new_limit_is_respected() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let preset = app
        .create_preset(&a, Some("standard"), PresetInput { new_per_day: Some(1), ..Default::default() })
        .await
        .unwrap();
    let deck = app.create_deck(&a, "Small", "", Some(preset.id)).await.unwrap();
    for f in ["a", "b", "c"] {
        app.create_card(&a, deck.id, f, "x", None, None).await.unwrap();
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
    let c2 = app.create_card(&a, d2.id, "secret", "x", None, None).await.unwrap();

    let (_, raw) = app
        .create_token(a.user_id(), "reader", false, false, Some(vec![d1.id]))
        .await
        .unwrap();
    let t = app.token_access(&raw).await.unwrap();
    assert_eq!(t.source(), "agent");
    assert_eq!(app.decks(&t).await.unwrap().len(), 1);
    assert!(matches!(app.card(&t, c2.id).await, Err(AppError::NotFound)));
    assert!(matches!(
        app.create_card(&t, d1.id, "q", "a", None, None).await,
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
    let c = app.create_card(&w, d1.id, "from agent", "", None, None).await.unwrap();
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
            .create_card(&a, deck.id, &format!("q{i}"), "a", None, None)
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
        .create_card(&kolja, deck.id, "hablar", "to speak", Some(img.id.clone()), None)
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
async fn offline_answers_replay_in_order() {
    use crate::offline::OfflineAnswer;
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "D", "", None).await.unwrap();
    let c1 = app.create_card(&a, deck.id, "one", "1", None, None).await.unwrap();
    let c2 = app.create_card(&a, deck.id, "two", "2", None, None).await.unwrap();

    let q = app.offline_queue(&a).await.unwrap();
    let items = q["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["previews"]["again"]["secs"], 600);

    let t = now() - 3600;
    let ans = |id: &str, card: i64, rating: &str, reps: i64, at: i64| OfflineAnswer {
        client_id: id.into(),
        card_id: card,
        rating: rating.into(),
        reps,
        answered_at: at,
        duration_ms: Some(4000),
    };
    // Sent out of order on purpose; the server sorts by time.
    let results = app
        .offline_sync(&a, vec![ans("b", c1.id, "good", 1, t + 700), ans("a", c1.id, "again", 0, t), ans("c", c2.id, "good", 0, t + 10)])
        .await
        .unwrap();
    assert!(results.iter().all(|r| r.status == "ok"), "{results:?}");
    let card = app.card(&a, c1.id).await.unwrap();
    assert_eq!(card.reps, 2);
    assert_eq!(card.card_state(), CardState::Review);
    assert_eq!(card.last_review, Some(t + 700));

    // Retrying the same sync is harmless.
    let again = app.offline_sync(&a, vec![ans("a", c1.id, "again", 0, t)]).await.unwrap();
    assert_eq!(again[0].status, "duplicate");
    // An answer for a card that moved on in the meantime is skipped.
    let stale = app.offline_sync(&a, vec![ans("d", c2.id, "again", 0, t + 20)]).await.unwrap();
    assert_eq!(stale[0].status, "stale");
    assert_eq!(app.card(&a, c2.id).await.unwrap().reps, 1);
    // Future timestamps are clamped to now.
    let c3 = app.create_card(&a, deck.id, "three", "3", None, None).await.unwrap();
    app.offline_sync(&a, vec![ans("e", c3.id, "good", 0, now() + 86_400)]).await.unwrap();
    assert!(app.card(&a, c3.id).await.unwrap().last_review.unwrap() <= now());
}

#[tokio::test]
async fn oauth_flow() {
    use base64::Engine;
    use sha2::Digest;

    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "D", "", None).await.unwrap();
    app.create_card(&a, deck.id, "q", "a", None, None).await.unwrap();

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

    // Refresh rotates: the old refresh token stops working.
    let fresh = app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.unwrap();
    assert!(app.refresh_tokens(&client_id, None, &tokens.refresh_token).await.is_err());
    assert!(app.token_access(&fresh.access_token).await.is_ok());
    // Another client can't use it.
    assert!(app.refresh_tokens("someone-else", None, &fresh.refresh_token).await.is_err());

    // The connection shows up next to tokens; disconnecting kills it.
    let list = app.list_tokens(a.user_id()).await.unwrap();
    assert_eq!(list.len(), 1);
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
