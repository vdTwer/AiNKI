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
