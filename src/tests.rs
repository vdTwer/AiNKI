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
        app.create_card(&a, deck.id, f, b, None, None, None).await.unwrap();
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
    let card = app.create_card(&a, deck.id, "Hund", "dog", None, None, None).await.unwrap();
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
    assert!(app.create_card(&a, deck.id, "q", "just text", None, None, Some(crate::multi::Split::List)).await.is_err());
    let back = "Cohen:\n1. Cultural body\n2. Always escapes\n3. Category crisis";
    let card = app.create_card(&a, deck.id, "Theses on monsters?", back, None, None, Some(crate::multi::Split::List)).await.unwrap();
    assert_eq!(card.multi, Some(crate::multi::Split::List));
    assert_eq!(card.card_state(), CardState::New);
    let items = app.card_items(&card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["Cultural body", "Always escapes", "Category crisis"]);
    assert_eq!(crate::multi::asked(&card, &items, now()), [true, true, true]);

    // 1 of 3: the second and third were missed.
    let r = app.answer_list(&a, card.id, vec![1, 2], None, Some(0)).await.unwrap();
    let card = r.card;
    assert_eq!(card.card_state(), CardState::Learning);
    let items = app.card_items(&card).await.unwrap();
    assert!(items.iter().all(|i| i.reps == 1));
    assert!(items[1].due < items[0].due);
    // Next time only the missed ones are asked; the other shows as known.
    let asked = crate::multi::asked(&card, &items, now());
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
        .update_card(&a, card.id, CardInput { multi: Some(None), ..Default::default() })
        .await
        .unwrap();
    assert!(card.multi.is_none());
    assert!(app.card_items(&card).await.unwrap().is_empty());
    assert_ne!(card.card_state(), CardState::New);
}

#[tokio::test]
async fn a_studied_card_turned_into_a_list_keeps_its_memory() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Math", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "Operations on F^n?", "1. Scalar multiplication\n2. Addition", None, None, None).await.unwrap();
    let card = app.answer_card(&a, card.id, Rating::Good, None, None).await.unwrap().card;
    let list = app
        .update_card(&a, card.id, CardInput { multi: Some(Some(crate::multi::Split::List)), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(list.due, card.due);
    assert_eq!(list.card_state(), card.card_state());
    let items = app.card_items(&list).await.unwrap();
    assert!(items.iter().all(|i| i.stability == card.stability && i.due == card.due));
    // A list in review asks for every item.
    assert_eq!(crate::multi::asked(&list, &items, now()), [true, true]);
}

#[tokio::test]
async fn old_list_cards_are_split_again_and_keep_their_memory() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Lit", "", None).await.unwrap();
    let back = "1. Cultural body\n2. Always escapes\n\nRemark: Cohen, 1996";
    let card = app.create_card(&a, deck.id, "Theses?", back, None, None, Some(crate::multi::Split::List)).await.unwrap();
    let card = app.answer_list(&a, card.id, vec![], None, Some(0)).await.unwrap().card;
    // Before multicards the remark was part of the last item.
    sqlx::query("UPDATE card_items SET text = 'Always escapes\nRemark: Cohen, 1996' WHERE position = 1")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("PRAGMA user_version = 0").execute(&app.db).await.unwrap();
    app.resplit_multicards().await.unwrap();
    let items = app.card_items(&card).await.unwrap();
    assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["Cultural body", "Always escapes"]);
    assert!(items.iter().all(|i| i.reps == 1));
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
        app.create_card(&a, deck.id, f, "x", None, None, None).await.unwrap();
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
    let c2 = app.create_card(&a, d2.id, "secret", "x", None, None, None).await.unwrap();

    let (_, raw) = app
        .create_token(a.user_id(), "reader", false, false, Some(vec![d1.id]))
        .await
        .unwrap();
    let t = app.token_access(&raw).await.unwrap();
    assert_eq!(t.source(), "agent");
    assert_eq!(app.decks(&t).await.unwrap().len(), 1);
    assert!(matches!(app.card(&t, c2.id).await, Err(AppError::NotFound)));
    assert!(matches!(
        app.create_card(&t, d1.id, "q", "a", None, None, None).await,
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
    let c = app.create_card(&w, d1.id, "from agent", "", None, None, None).await.unwrap();
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
            .create_card(&a, deck.id, &format!("q{i}"), "a", None, None, None)
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
    assert_eq!(result["changed"], true);
    let user = app.user(a.user_id()).await.unwrap();
    assert_eq!(user.params().len(), 21);
    assert!(user.optimize_note.is_some() && user.optimized_at.is_some());

    // The deck's preset has well over 400 reviews: it was fitted on its own,
    // starting from the user's parameters, and kept only if better.
    let preset = app.preset(&a, deck.preset_id).await.unwrap();
    let presets = result["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(preset.has_own_params(), presets[0]["changed"] == true);

    // Running it again right away finds nothing better.
    let again = app.optimize_parameters(&Access::web(user.clone())).await.unwrap();
    assert_eq!(again["changed"], false);

    // What the preset costs at each retention: more remembered costs more time.
    let w = app.workload(&Access::web(user.clone()), deck.preset_id, 30).await.unwrap();
    assert_eq!(w.iter().map(|w| w.retention).collect::<Vec<_>>(), vec![0.80, 0.85, 0.90, 0.95]);
    assert!(w.iter().filter(|w| w.current).count() == 1);
    assert!(w[3].reviews_per_day > w[0].reviews_per_day, "{w:?}");
    assert!(w[3].remembered > w[0].remembered, "{w:?}");

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
        .create_card(&kolja, deck.id, "hablar", "to speak", Some(img.id.clone()), None, None)
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
async fn unused_images_are_removed_after_a_day() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Spanish", "", None).await.unwrap();
    let kept = app.store_image(&a, tiny_png()).await.unwrap();
    let dropped = app.store_image(&a, tiny_png()).await.unwrap();
    let fresh = app.store_image(&a, tiny_png()).await.unwrap();
    app.create_card(&a, deck.id, "gato", "cat", Some(kept.id.clone()), None, None).await.unwrap();
    sqlx::query("UPDATE media SET created_at = created_at - 2 * 86400 WHERE id != ?")
        .bind(&fresh.id)
        .execute(&app.db)
        .await
        .unwrap();
    assert_eq!(app.delete_unused_images(now() - 86_400).await.unwrap(), 1);
    assert!(app.media(a.user_id(), &kept.id).await.is_ok());
    assert!(app.media(a.user_id(), &fresh.id).await.is_ok());
    assert!(app.media(a.user_id(), &dropped.id).await.is_err());
    assert!(!app.media_path(&dropped.id).exists());
}

#[tokio::test]
async fn accepting_a_share_twice_makes_one_copy() {
    let app = app().await;
    let kolja = user(&app, "kolja").await;
    let anna = user(&app, "anna").await;
    let deck = app.create_deck(&kolja, "Spanish", "", None).await.unwrap();
    app.create_card(&kolja, deck.id, "hablar", "to speak", None, None, None).await.unwrap();
    let share = app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    let (first, second) = tokio::join!(app.accept_share(&anna, share.id), app.accept_share(&anna, share.id));
    assert!(first.is_ok() != second.is_ok());
    assert_eq!(app.decks(&anna).await.unwrap().len(), 1);
}

#[tokio::test]
async fn oauth_flow() {
    use base64::Engine;
    use sha2::Digest;

    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "D", "", None).await.unwrap();
    app.create_card(&a, deck.id, "q", "a", None, None, None).await.unwrap();

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

    fn urlencode(v: &str) -> String {
        v.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
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

        // Opened from studying, saving goes back to the session.
        let res = submit_multipart(&app, &cookie, &format!("/cards/{id}?study=1&deck={d}"), &[("deck", &d), ("front", "el gato"), ("back", "the cat")]).await;
        assert_eq!(location(&res), format!("/study?deck={d}"));
        assert_eq!(flash(&res), "Card saved.");
        let page = text(send(&app, Request::get(format!("/cards/{id}?study=1")).header(header::COOKIE, cookie.clone()).body(Body::empty()).unwrap()).await).await;
        assert!(page.contains(r#"href="/study""#) && page.contains("Back to studying"));

        let res = submit(&app, &cookie, &format!("/cards/{id}/suspend"), "").await;
        assert_eq!(location(&res), format!("/cards/{id}"));
        assert!(app.card(&a, id).await.unwrap().suspended);
        let res = submit(&app, &cookie, &format!("/cards/{id}/delete"), "").await;
        assert_eq!(location(&res), format!("/decks/{d}"));
        assert_eq!(flash(&res), "Card deleted.");
        assert!(app.card(&a, id).await.is_err());
    }

    #[tokio::test]
    async fn a_timed_card_that_runs_out_counts_as_forgotten() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Spanish", "", None).await.unwrap();
        app.update_preset(&a, deck.preset_id, PresetInput { time_limit: Some(10), ..Default::default() }).await.unwrap();
        let card = app.create_card(&a, deck.id, "el perro", "the dog", None, None, None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let get = |path: &str| send(&app, Request::get(path).header(header::COOKIE, cookie.clone()).body(Body::empty()).unwrap());

        // A new card is never timed.
        assert!(!text(get("/study").await).await.contains("data-limit"));
        make_review(&app, card.id, 5.0, 6, 1).await;
        assert!(text(get("/study").await).await.contains(r#"data-limit="10""#));

        let body = format!("card={}&reps=3&t={}&rating=3&timeout=1&sure=1", card.id, now() - 12);
        let res = submit(&app, &cookie, "/study/answer", &body).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let (rating, timed_out, confidence): (i64, bool, Option<i64>) =
            sqlx::query_as("SELECT rating, timed_out, confidence FROM review_log WHERE card_id = ?")
                .bind(card.id)
                .fetch_one(&app.db)
                .await
                .unwrap();
        assert_eq!((rating, timed_out, confidence), (Rating::Again as i64, true, Some(1)));

        // Without JavaScript, showing the answer too late is a timeout too.
        make_review(&app, card.id, 5.0, 6, 1).await;
        sqlx::query("UPDATE cards SET reps = 3 WHERE id = ?").bind(card.id).execute(&app.db).await.unwrap();
        let late = text(get(&format!("/study?card={}&show=1&t={}", card.id, now() - 30)).await).await;
        assert!(late.contains(r#"class="study revealed timed-out""#), "{late}");
    }

    #[tokio::test]
    async fn quiet_hours_show_a_rest_screen() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Spanish", "", None).await.unwrap();
        app.create_card(&a, deck.id, "el perro", "the dog", None, None, None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        // Quiet all day but this hour's end: from the next hour round to this one.
        let res = submit(&app, &cookie, "/settings/study", "quiet_from=0&quiet_to=0&session_minutes=0&catch_up_reviews=100").await;
        assert_eq!(location(&res), "/settings#studying");
        use chrono::{TimeZone, Timelike};
        let hour = a.user.tz().timestamp_opt(now(), 0).unwrap().hour();
        let body = format!("quiet_from={hour}&quiet_to={}&session_minutes=0&catch_up_reviews=100", (hour + 1) % 24);
        submit(&app, &cookie, "/settings/study", &body).await;
        let page = |c: String| send(&app, Request::get("/study").header(header::COOKIE, c).body(Body::empty()).unwrap());
        let rest = text(page(cookie.clone()).await).await;
        assert!(rest.contains("Quiet hours") && !rest.contains("el perro"));

        let res = submit(&app, &cookie, "/study/anyway", "").await;
        assert_eq!(location(&res), "/study");
        assert!(text(page(cookie.clone()).await).await.contains("el perro"));
    }

    #[tokio::test]
    async fn a_lesson_can_be_read_and_added_as_a_deck() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let get = |path: &str| send(&app, Request::get(path).header(header::COOKIE, cookie.clone()).body(Body::empty()).unwrap());
        assert!(text(get("/learn").await).await.contains("The spacing effect"));
        let page = text(get("/learn/spacing").await).await;
        assert!(page.contains("Strong evidence") && page.contains("Add as a deck"));
        assert_eq!(get("/learn/no-such-lesson").await.status(), StatusCode::NOT_FOUND);

        let res = submit(&app, &cookie, "/learn/spacing/deck", "").await;
        assert_eq!(flash(&res), "Lesson added as a deck.");
        let deck = app.deck_by_name(&a, "AiNKI lessons::The spacing effect").await.unwrap().unwrap();
        assert_eq!(location(&res), format!("/decks/{}", deck.id));
        let (cards, _) = app.list_cards(&a, &CardFilter { deck_id: Some(deck.id), ..Default::default() }).await.unwrap();
        assert_eq!(cards.len(), crate::lessons::find("spacing").unwrap().cards.len());

        // Twice makes no second copy.
        let res = submit(&app, &cookie, "/learn/spacing/deck", "").await;
        assert_eq!(flash(&res), "You already have this deck.");
        assert!(text(get("/learn/spacing").await).await.contains("This lesson is in your decks"));
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
    async fn sessions_in_use_stay_alive() {
        let app = app().await;
        user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let get = |c: &str| Request::get("/").header(header::COOKIE, c.to_string()).body(Body::empty()).unwrap();
        // Fresh: nothing to renew.
        let res = send(&app, get(&cookie)).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(!res.headers().contains_key(header::SET_COOKIE));
        // Ten days later the session gets its full lifetime again.
        sqlx::query("UPDATE sessions SET expires_at = expires_at - 10 * 86400").execute(&app.db).await.unwrap();
        let res = send(&app, get(&cookie)).await;
        assert!(res.headers()[header::SET_COOKIE].to_str().unwrap().starts_with("ainki_session="));
        let left: i64 = sqlx::query_scalar("SELECT expires_at - ? FROM sessions").bind(now()).fetch_one(&app.db).await.unwrap();
        assert!(left > (crate::auth::SESSION_DAYS - 1) * 86_400);
    }

    #[tokio::test]
    async fn settings_with_a_cleared_number_keep_its_value() {
        let app = app().await;
        user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let res = submit(&app, &cookie, "/settings", "timezone=Europe%2FBerlin&rollover_hour=&font_scale=120").await;
        assert_eq!(location(&res), "/settings");
        let u = app.find_user("kolja").await.unwrap();
        assert_eq!((u.rollover_hour, u.font_scale), (4, 120));
    }

    #[tokio::test]
    async fn exported_file_keeps_the_deck_name() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Español", "", None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let req = Request::get(format!("/decks/{}/export.csv", deck.id)).header(header::COOKIE, cookie).body(Body::empty()).unwrap();
        let res = send(&app, req).await;
        let d = res.headers()[header::CONTENT_DISPOSITION].to_str().unwrap();
        assert_eq!(d, "attachment; filename=\"Espa_ol.csv\"; filename*=UTF-8''Espa%C3%B1ol.csv");
    }

    #[tokio::test]
    async fn favicons_and_the_build_on_settings() {
        let app = app().await;
        for path in ["/favicon.ico", "/favicon.svg", "/apple-touch-icon.png"] {
            let res = send(&app, Request::get(path).body(Body::empty()).unwrap()).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
        }
        user(&app, "kolja").await;
        let cookie = login_cookie(&app, "kolja").await;
        let res = send(&app, Request::get("/settings").header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await;
        let page = text(res).await;
        assert!(page.contains(&format!("Build {}", env!("AINKI_BUILD"))));
        assert!(page.contains(r#"rel="icon""#));
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
        app.create_card(&a, deck.id, "el perro", "the dog", None, None, None).await.unwrap();
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

        // The choice is only on the page once the back can be split.
        let res = send(&app, Request::get("/cards/new").header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        assert!(text(res).await.contains(r#"<fieldset class="line multi-pick" hidden>"#));
        let res = submit_multipart(&app, &cookie, "/cards/new", &[("deck", &d), ("front", "Theses?"), ("back", back), ("multi", "list")]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let (cards, _) = app.list_cards(&a, &CardFilter { deck_id: Some(deck.id), ..Default::default() }).await.unwrap();
        let card = &cards[0];
        assert_eq!(card.multi, Some(crate::multi::Split::List));
        let res = send(&app, Request::get(format!("/cards/{}", card.id)).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        let html = text(res).await;
        assert!(html.contains(r#"<input type="radio" name="multi" value="list" checked>"#));
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
    async fn a_multicard_keeps_what_is_not_an_item() {
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "English", "", None).await.unwrap();
        let cookie = login_cookie(&app, "kolja").await;
        let back = "will\n    - decisions made at the moment of speaking: That looks heavy. I'll give you a hand.\n    \
            - guesses and opinions about the future: I think I'll make it on time.\n\nbe going to\n    \
            - intentions planned beforehand: We're going to have a barbecue.\n\n\
            ⚠️ Remark: will is for spontaneous decisions.";

        // The preview offers the list and shows the card as it is studied.
        let body = format!("front=Future+simple&multi=list&back={}", urlencode(back));
        let res = submit(&app, &cookie, "/cards/preview", &body).await;
        let v: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
        assert_eq!(v["splits"], serde_json::json!(["list"]));
        let html = v["html"].as_str().unwrap();
        assert!(html.contains("<p>be going to</p>"));
        assert!(!html.contains("name=\"recalled\""));

        let d = deck.id.to_string();
        submit_multipart(&app, &cookie, "/cards/new", &[("deck", &d), ("front", "Future simple?"), ("back", back), ("multi", "list")]).await;
        let (cards, _) = app.list_cards(&a, &CardFilter { deck_id: Some(deck.id), ..Default::default() }).await.unwrap();
        let card = &cards[0];
        // The pasted indent became a real list.
        assert!(card.back.contains("\n- guesses"));
        assert_eq!(app.card_items(card).await.unwrap().len(), 3);
        let res = send(&app, Request::get(format!("/study?card={}&show=1", card.id)).header(header::COOKIE, &cookie).body(Body::empty()).unwrap()).await;
        let html = text(res).await;
        for kept in ["<p>will</p>", "<p>be going to</p>", "Remark: will is for spontaneous decisions."] {
            assert!(html.contains(kept), "{kept}");
        }
        assert!(html.contains(r#"<input type="checkbox" name="recalled" value="2">"#));
        assert_eq!(html.matches(r#"<ol class="list-items">"#).count(), 2);
    }

    /// Call an MCP tool with a token and return its structured result.
    async fn mcp_tool(app: &Arc<App>, token: &str, name: &str, args: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
        let res = send(
            app,
            Request::post("/mcp")
                .header(header::HOST, "cards.example.com")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json, text/event-stream")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&text(res).await).unwrap();
        let content = v["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{v}"));
        serde_json::from_str(content).unwrap()
    }

    #[tokio::test]
    async fn agents_respect_quiet_hours_and_see_the_coach_and_lessons() {
        use chrono::{TimeZone, Timelike};
        let app = app().await;
        let a = user(&app, "kolja").await;
        let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
        app.create_card(&a, deck.id, "Hund", "dog", None, None, None).await.unwrap();
        let (_, token) = app.create_token(a.user_id(), "test", true, true, None).await.unwrap();

        let next = mcp_tool(&app, &token, "get_next_card", serde_json::json!({})).await;
        assert_eq!(next["done"], false);
        let hour = a.user.tz().timestamp_opt(now(), 0).unwrap().hour() as i64;
        app.update_study_settings(a.user_id(), Some((hour, (hour + 1) % 24)), 0, false, 100, false).await.unwrap();
        let next = mcp_tool(&app, &token, "get_next_card", serde_json::json!({})).await;
        assert_eq!((next["done"].clone(), next["quiet_hours"].clone()), (serde_json::json!(true), serde_json::json!(true)));
        let overview = mcp_tool(&app, &token, "get_overview", serde_json::json!({})).await;
        assert_eq!(overview["studying"]["quiet_now"], true);

        let lessons = mcp_tool(&app, &token, "list_lessons", serde_json::json!({})).await;
        assert_eq!(lessons["lessons"].as_array().unwrap().len(), crate::lessons::all().len());
        let lesson = mcp_tool(&app, &token, "get_lesson", serde_json::json!({ "slug": "learning-styles" })).await;
        assert_eq!(lesson["evidence"], "Not supported by evidence");

        let attention = mcp_tool(&app, &token, "list_cards_needing_attention", serde_json::json!({})).await;
        assert!(attention["cards"].as_array().unwrap().is_empty());
        let insights = mcp_tool(&app, &token, "get_insights", serde_json::json!({})).await;
        assert_eq!(insights["week"]["reviews"], 0);
        let work = mcp_tool(&app, &token, "get_workload", serde_json::json!({ "preset_id": deck.preset_id })).await;
        assert!(work["options"].as_array().unwrap().len() >= 4);
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

#[tokio::test]
async fn the_coach_flags_leeches_lists_and_duplicates() {
    use crate::coach::Kind;
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    let leech = app.create_card(&a, deck.id, "Capital of Burkina Faso", "Ouagadougou", None, None, None).await.unwrap();
    let list = app
        .create_card(&a, deck.id, "Three primary colours", "- red\n- yellow\n- blue", None, None, None)
        .await
        .unwrap();
    let twin = app.create_card(&a, deck.id, "capital of burkina faso?", "Ouagadougou", None, None, None).await.unwrap();
    let fine = app.create_card(&a, deck.id, "Hund", "dog", None, None, None).await.unwrap();

    // Forget the first card five times and the list card three times.
    for (id, lapses) in [(leech.id, 5), (list.id, 3)] {
        sqlx::query("UPDATE cards SET state = 2, stability = 3.0, difficulty = 7.0, lapses = ?, last_review = ?, due = ? WHERE id = ?")
            .bind(lapses)
            .bind(now() - 86_400)
            .bind(now() + 86_400)
            .bind(id)
            .execute(&app.db)
            .await
            .unwrap();
    }

    let list_of = |v: &[crate::coach::Attention], id: i64| -> Vec<Kind> {
        v.iter().find(|x| x.card.id == id).map(|x| x.flags.iter().map(|f| f.kind).collect()).unwrap_or_default()
    };
    let found = app.attention(&a, Some(deck.id)).await.unwrap();
    assert_eq!(found[0].card.id, leech.id, "leeches come first");
    assert_eq!(list_of(&found, leech.id), vec![Kind::Leech, Kind::Duplicate]);
    assert_eq!(list_of(&found, list.id), vec![Kind::Split]);
    assert_eq!(list_of(&found, twin.id), vec![Kind::Duplicate]);
    assert!(list_of(&found, fine.id).is_empty());

    // Keep as is: gone until the card slips three more times.
    app.dismiss_flag(&a, leech.id, Kind::Leech).await.unwrap();
    let found = app.attention(&a, None).await.unwrap();
    assert_eq!(list_of(&found, leech.id), vec![Kind::Duplicate]);
    sqlx::query("UPDATE cards SET lapses = 8 WHERE id = ?").bind(leech.id).execute(&app.db).await.unwrap();
    let card = app.card(&a, leech.id).await.unwrap();
    assert!(app.card_flags(&a, &card).await.unwrap().iter().any(|f| f.kind == Kind::Leech));
    assert!(app.leech_note(&card).await.unwrap().is_some());

    // Writing a card like an existing one finds it.
    let similar = app.similar_cards(&a, "Capital of Burkina Faso", None).await.unwrap();
    assert_eq!(similar.len(), 2);

    // Split the list card into three; the original goes.
    let new = app
        .split_card(
            &a,
            list.id,
            vec![
                ("Primary colour 1".into(), "red".into()),
                ("Primary colour 2".into(), "yellow".into()),
                ("Primary colour 3".into(), "blue".into()),
            ],
            false,
        )
        .await
        .unwrap();
    assert_eq!(new.len(), 3);
    assert!(app.card(&a, list.id).await.is_err());
}

#[tokio::test]
async fn calibration_compares_predicted_and_actual_recall() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "q", "a", None, None, None).await.unwrap();
    // 20 reviews nine days after the last, of a card with a ten-day memory
    // (a little over 90% predicted): 15 remembered.
    let t = now() - 86_400;
    for n in 0..20 {
        let prev = format!(r#"{{"state":"review","step":0,"stability":10.0,"difficulty":5.0,"due":0,"last_review":{},"reps":3,"lapses":0}}"#, t - 9 * 86_400);
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, source, prev_card)
             VALUES (?, ?, ?, ?, ?, 2, 10, 0, 'web', ?)",
        )
        .bind(a.user_id())
        .bind(card.id)
        .bind(deck.id)
        .bind(if n < 15 { 3 } else { 1 })
        .bind(t)
        .bind(prev)
        .execute(&app.db)
        .await
        .unwrap();
    }
    let c = app.calibration(&a, None).await.unwrap();
    assert_eq!(c.reviews, 20);
    assert!((c.actual - 0.75).abs() < 1e-9);
    assert!(c.predicted > 0.9 && c.predicted < 0.93, "{c:?}");
    assert_eq!(c.bins.len(), 1);
    assert_eq!((c.bins[0].from, c.bins[0].to), (90, 95));
}

/// A fresh view of the user after their settings changed.
async fn reload(app: &App, name: &str) -> Access {
    Access::web(app.find_user(name).await.unwrap())
}

/// Make a card a review card due since `days_overdue` days, with the given
/// stability, last reviewed `elapsed_days` ago.
async fn make_review(app: &App, card_id: i64, stability: f64, elapsed_days: i64, days_overdue: i64) {
    sqlx::query(
        "UPDATE cards SET state = 2, stability = ?, difficulty = 5.0, reps = 3, last_review = ?, due = ? WHERE id = ?",
    )
    .bind(stability)
    .bind(now() - elapsed_days * 86_400)
    .bind(now() - days_overdue * 86_400)
    .bind(card_id)
    .execute(&app.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn catch_up_mode_caps_reviews_and_starts_with_the_best_remembered() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    let mut ids = vec![];
    for n in 0..5 {
        ids.push(app.create_card(&a, deck.id, &format!("q{n}"), "a", None, None, None).await.unwrap().id);
    }
    // Three overdue reviews: 30 days gone on a 10-day memory, 30 on a
    // 100-day memory, 12 on a 10-day one. Two cards stay new.
    make_review(&app, ids[0], 10.0, 30, 20).await;
    make_review(&app, ids[1], 100.0, 30, 3).await;
    make_review(&app, ids[2], 10.0, 12, 2).await;
    assert_eq!(app.overdue(&a).await.unwrap(), 3);

    sqlx::query("UPDATE users SET catch_up = 1, catch_up_reviews = 2 WHERE id = ?")
        .bind(a.user_id())
        .execute(&app.db)
        .await
        .unwrap();
    let a = reload(&app, "kolja").await;
    let s = &app.deck_summaries(&a).await.unwrap()[0];
    assert_eq!((s.new_today, s.review_due), (0, 2));

    // Highest chance of recall first: 30/100, then 12/10; never 30/10.
    let first = app.next_card(&a, None).await.unwrap().unwrap().card;
    assert_eq!(first.id, ids[1]);
    app.answer_card(&a, first.id, Rating::Good, None, None).await.unwrap();
    let second = app.next_card(&a, None).await.unwrap().unwrap().card;
    assert_eq!(second.id, ids[2]);
    app.answer_card(&a, second.id, Rating::Good, None, None).await.unwrap();
    assert!(app.next_card(&a, None).await.unwrap().is_none());
}

#[tokio::test]
async fn quiet_hours_rest_until_they_end_or_are_lifted() {
    use chrono::{TimeZone, Timelike};
    let app = app().await;
    let a = user(&app, "kolja").await;
    let hour = a.user.tz().timestamp_opt(now(), 0).unwrap().hour() as i64;
    sqlx::query("UPDATE users SET quiet_from = ?, quiet_to = ? WHERE id = ?")
        .bind(hour)
        .bind((hour + 2) % 24)
        .bind(a.user_id())
        .execute(&app.db)
        .await
        .unwrap();
    let a = reload(&app, "kolja").await;
    let until = crate::study::quiet_until(&a.user, now()).expect("quiet now");
    assert!(until > now() + 3600 && until <= now() + 2 * 3600, "{until}");
    // Outside the window it isn't quiet.
    assert!(crate::study::quiet_until(&a.user, until + 60).is_none());

    app.lift_quiet(&a.user).await.unwrap();
    let a = reload(&app, "kolja").await;
    assert!(crate::study::quiet_until(&a.user, now()).is_none());
    // Lifting lasts until those quiet hours are over, not for the next ones.
    assert!(crate::study::quiet_until(&a.user, until + 22 * 3600 + 60).is_some());
}

#[tokio::test]
async fn a_break_is_suggested_after_the_session_length() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "q", "a", None, None, None).await.unwrap();
    let log = |mins_ago: i64| {
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, source)
             VALUES (?, ?, ?, 3, ?, 2, 1, 0, 'web')",
        )
        .bind(a.user_id())
        .bind(card.id)
        .bind(deck.id)
        .bind(now() - mins_ago * 60)
        .execute(&app.db)
    };
    // An old session, a long pause, then 25 minutes with short gaps.
    for m in [90, 85, 25, 18, 11, 5, 1] {
        log(m).await.unwrap();
    }
    assert!(app.break_due(&a).await.unwrap().is_none(), "off by default");
    sqlx::query("UPDATE users SET session_minutes = 20 WHERE id = ?").bind(a.user_id()).execute(&app.db).await.unwrap();
    let a = reload(&app, "kolja").await;
    assert_eq!(app.break_due(&a).await.unwrap(), Some(25));

    app.skip_break(a.user_id()).await.unwrap();
    let a = reload(&app, "kolja").await;
    assert_eq!(app.break_due(&a).await.unwrap(), None);
}

#[tokio::test]
async fn sure_or_not_is_kept_with_the_answer() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    for (n, sure, rating) in [(0, true, Rating::Good), (1, true, Rating::Again), (2, false, Rating::Good)] {
        let c = app.create_card(&a, deck.id, &format!("q{n}"), "a", None, None, None).await.unwrap();
        app.answer_card(&a, c.id, rating, None, None).await.unwrap();
        app.note_answer(&a, c.id, Some(sure), false).await.unwrap();
    }
    let c = app.confidence(&a, None).await.unwrap();
    assert_eq!((c.sure, c.sure_recalled, c.unsure, c.unsure_recalled), (2, 1, 1, 1));
    assert_eq!(c.sure_pct(), Some(50.0));
}

#[tokio::test]
async fn the_week_compares_retention_with_the_aim() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Facts", "", None).await.unwrap();
    let card = app.create_card(&a, deck.id, "q", "a", None, None, None).await.unwrap();
    // 25 reviews of a learned card over two days, 15 remembered: 60%
    // against the Standard aim of 90%.
    for n in 0..25 {
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before,
                elapsed_days, scheduled_secs, duration_ms, source)
             VALUES (?, ?, ?, ?, ?, 2, 3, 0, 6000, 'web')",
        )
        .bind(a.user_id())
        .bind(card.id)
        .bind(deck.id)
        .bind(if n < 15 { 3 } else { 1 })
        .bind(now() - (n % 2) * 86_400 - 60)
        .execute(&app.db)
        .await
        .unwrap();
    }
    let w = app.week(&a).await.unwrap();
    assert_eq!((w.reviews, w.retention_sample, w.new_cards), (25, 25, 0));
    assert!(w.days_studied >= 1 && w.days_studied <= 3);
    assert!((w.retention.unwrap() - 0.6).abs() < 1e-9);
    assert!((w.target.unwrap() - 0.9).abs() < 1e-9);
    assert_eq!(w.tip, crate::insight::Tip::BelowTarget);
    assert!((w.minutes - 2.5).abs() < 1e-9);

    let r = app.deck_retention(&a).await.unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].reviews, 25);
    assert!(r[0].short());
}

/// Run with `cargo test --release big_collection -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn big_collection_stays_fast() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Big", "", None).await.unwrap();
    let mut tx = app.db.begin().await.unwrap();
    for n in 0..20_000i64 {
        sqlx::query(
            "INSERT INTO cards (user_id, deck_id, front, back, state, stability, difficulty, due, last_review, reps, lapses, position, created_at, updated_at)
             VALUES (?, ?, ?, 'back', 2, 20.0, 5.0, ?, ?, 5, ?, ?, 0, 0)",
        )
        .bind(a.user_id())
        .bind(deck.id)
        .bind(format!("front number {n}"))
        .bind(now() + 86_400)
        .bind(now() - 86_400)
        .bind(if n % 7 == 0 { 2 + n % 5 } else { 0 })
        .bind(n)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let t = std::time::Instant::now();
    let found = app.attention(&a, None).await.unwrap();
    println!("attention: {} cards in {:?}", found.len(), t.elapsed());
    let t = std::time::Instant::now();
    app.week(&a).await.unwrap();
    println!("week: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    app.overdue(&a).await.unwrap();
    println!("overdue: {:?}", t.elapsed());
}
