//! Upgrading a v1 database (what runs in production on `main`) to v2.
//!
//! Production keeps its data across the switch, so this builds a database
//! with only the migrations that `main` shipped, fills it the way v1 left
//! it, runs the v2 migrations and startup on top, and checks that nothing
//! was lost and every v2 feature works on the old rows.

use std::{borrow::Cow, str::FromStr};

use sqlx::{
    SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use super::*;

/// The last migration that `main` (v1, production) ships.
const LAST_V1_MIGRATION: i64 = 8;

/// SHA-384 of each v1 migration as it is on `main`. sqlx refuses to start
/// when an applied migration's file has changed, so editing one of these
/// would take production down on the next deploy.
const V1_CHECKSUMS: [(i64, &str); 8] = [
    (1, "7b56961e44764764b6f7c1bef0230709c63bf89e1200dfc7878adc65e9194273a035f0809d818066c9e83d52d34a1985"),
    (2, "66ca605dff91fa0dce25e4554e9f517e2eb833a0cddb025a8d6d43ba770c0a479fa2727e3f737ad2a1f7897f30cb5455"),
    (3, "d4234e63bfc8b9bf0f9fd3c2ebeb014eb697691302bfa89547a25feb2783d5782be4e9dcf520d8534ddbd23618f7323e"),
    (4, "eb84443d45f8ebf270d8edf07bf3636b679744241de56f82e99e90cedee4def97d2502c504152d65b109bfba618438b4"),
    (5, "45cbf27b4144dc55c1c8fc58f0add2560744c35cfa8d8b007c436b59bee28ac87d345355b0e840e7bc5a48978314a0bf"),
    (6, "6d7e4798945e3610a527d6e0e2564068de10be897c4199008ad772de6c2e41981c112e91910c748496e01ab939503647"),
    (7, "688476e24687cbdeca31d56499feee8fcda9b1ae70576b0dcbf2140a413bbd13b3325c0ef2c3b15a352f8a00e61d51f9"),
    (8, "97bcf35891f2c984e751973f49c280df72e48d8b3137e9f62649f52838a080fc070e75d2784ae41960512aea4b0c9dd3"),
];

fn all_migrations() -> Migrator {
    sqlx::migrate!("./migrations")
}

fn v1_migrations() -> Migrator {
    let all = all_migrations();
    let v1: Vec<_> = all.migrations.iter().filter(|m| m.version <= LAST_V1_MIGRATION).cloned().collect();
    Migrator { migrations: Cow::Owned(v1), ..all_migrations() }
}

async fn v1_database() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap().foreign_keys(true);
    let db = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
    v1_migrations().run(&db).await.unwrap();
    db
}

fn config() -> Config {
    Config {
        data_dir: std::env::temp_dir().join(format!("ainki-upgrade-{}", uuid::Uuid::new_v4().simple())),
        bind: "127.0.0.1:0".into(),
        allow_signup: false,
        secure_cookies: false,
        default_timezone: "UTC".into(),
        public_url: None,
    }
}

const DAY: i64 = 86_400;

/// What a v1 account looks like after some months of use: the five
/// presets v1 made at signup (one renamed, one added), nested decks, new,
/// learned, suspended and list cards with history, an API token, a share
/// and a session.
struct V1 {
    user: i64,
    friend: i64,
    spanish: i64,
    renamed_preset: i64,
    review_card: i64,
    list_card: i64,
    suspended_card: i64,
}

async fn seed_v1(db: &SqlitePool) -> V1 {
    let t = now();
    let joined = t - 120 * DAY;
    let hash = crate::auth::hash_password("password123").unwrap();
    let mut users = vec![];
    for (name, tag) in [("kolja", 1234), ("friend", 42)] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, discriminator, password_hash, timezone, rollover_hour, four_buttons,
                font_scale, fsrs_params, fsrs_params_updated_at, desktop_look, created_at)
             VALUES (?, ?, ?, 'Europe/Berlin', 4, 0, 110, '[]', NULL, 'calm', ?) RETURNING id",
        )
        .bind(name)
        .bind(tag)
        .bind(&hash)
        .bind(joined)
        .fetch_one(db)
        .await
        .unwrap();
        users.push(id);
    }
    let (user, friend) = (users[0], users[1]);

    // v1 made these five at signup, without a template column.
    let mut presets = vec![];
    for (name, retention) in [
        ("Standard", 0.9),
        ("Exam prep", 0.95),
        ("Language vocabulary", 0.9),
        ("Relaxed", 0.85),
        ("My own pace", 0.88), // was "Maintenance", renamed by the user
        ("Custom", 0.92),     // added by the user
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO presets (user_id, name, description, desired_retention, new_per_day, reviews_per_day,
                learning_steps, relearning_steps, max_interval, created_at)
             VALUES (?, ?, '', ?, 20, 200, '1m 10m', '10m', 36500, ?) RETURNING id",
        )
        .bind(user)
        .bind(name)
        .bind(retention)
        .bind(joined)
        .fetch_one(db)
        .await
        .unwrap();
        presets.push(id);
    }
    let friend_preset: i64 = sqlx::query_scalar(
        "INSERT INTO presets (user_id, name, desired_retention, new_per_day, reviews_per_day,
            learning_steps, relearning_steps, max_interval, created_at)
         VALUES (?, 'Standard', 0.9, 20, 200, '1m 10m', '10m', 36500, ?) RETURNING id",
    )
    .bind(friend)
    .bind(joined)
    .fetch_one(db)
    .await
    .unwrap();
    let _ = friend_preset;

    let deck = |name: &'static str, preset: i64| async move {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO decks (user_id, name, description, preset_id, created_at) VALUES (?, ?, '', ?, ?) RETURNING id",
        )
        .bind(user)
        .bind(name)
        .bind(preset)
        .bind(joined)
        .fetch_one(db)
        .await
        .unwrap()
    };
    let spanish = deck("Languages::Spanish", presets[2]).await;
    let history = deck("History", presets[4]).await;

    let card = |deck: i64, front: &'static str, back: &'static str, state: i64, due: i64, extra: &'static str| async move {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO cards (user_id, deck_id, front, back, state, due, created_at, updated_at, position)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, (SELECT COUNT(*) FROM cards)) RETURNING id",
        )
        .bind(user)
        .bind(deck)
        .bind(front)
        .bind(back)
        .bind(state)
        .bind(due)
        .bind(joined)
        .bind(joined)
        .fetch_one(db)
        .await
        .unwrap();
        if !extra.is_empty() {
            sqlx::query(&format!("UPDATE cards SET {extra} WHERE id = ?")).bind(id).execute(db).await.unwrap();
        }
        id
    };
    let _new = card(spanish, "el gato", "the cat", 0, 0, "").await;
    let review_card = card(
        spanish,
        "el perro",
        "the dog",
        2,
        t - DAY,
        "stability = 12.5, difficulty = 5.1, reps = 4, lapses = 1, last_review = unixepoch() - 13 * 86400",
    )
    .await;
    let suspended_card = card(history, "Year of the Battle of Hastings", "1066", 0, 0, "suspended = 1").await;
    let list_card = card(
        history,
        "The three estates",
        "- clergy\n- nobility\n- commoners",
        2,
        t - DAY,
        "multi = 'list', stability = 9.0, difficulty = 6.0, reps = 2, last_review = unixepoch() - 10 * 86400",
    )
    .await;

    // Review history for the plain card, from new.
    for (i, (rating, state_before)) in [(3, 0), (3, 1), (1, 2), (3, 3)].iter().enumerate() {
        sqlx::query(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days,
                scheduled_secs, stability, difficulty, duration_ms, source)
             VALUES (?, ?, ?, ?, ?, ?, ?, 86400, 3.0, 5.0, 4000, 'web')",
        )
        .bind(user)
        .bind(review_card)
        .bind(spanish)
        .bind(rating)
        .bind(t - (40 - i as i64 * 7) * DAY)
        .bind(state_before)
        .bind(i as i64 * 7)
        .execute(db)
        .await
        .unwrap();
    }
    // The list card's items with their own memory, as v1 kept them.
    for (pos, text) in ["clergy", "nobility", "commoners"].iter().enumerate() {
        let item: i64 = sqlx::query_scalar(
            "INSERT INTO card_items (card_id, position, text, state, stability, difficulty, due, last_review, reps)
             VALUES (?, ?, ?, 2, ?, 6.0, ?, unixepoch() - 10 * 86400, 2) RETURNING id",
        )
        .bind(list_card)
        .bind(pos as i64)
        .bind(text)
        .bind(9.0 + pos as f64)
        .bind(t - DAY + pos as i64)
        .fetch_one(db)
        .await
        .unwrap();
        let log: i64 = sqlx::query_scalar(
            "INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days,
                scheduled_secs, source, items_asked, items_recalled)
             VALUES (?, ?, ?, 3, unixepoch() - 10 * 86400, 0, 0, 864000, 'web', 3, 3) RETURNING id",
        )
        .bind(user)
        .bind(list_card)
        .bind(history)
        .fetch_one(db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO item_review_log (review_log_id, item_id, rating, state_before, reviewed_at)
             VALUES (?, ?, 3, 0, unixepoch() - 10 * 86400)",
        )
        .bind(log)
        .bind(item)
        .execute(db)
        .await
        .unwrap();
    }

    // Logged in, an agent token, and a deck offered to a friend.
    sqlx::query("INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
        .bind(crate::auth::sha256_hex("v1-session"))
        .bind(user)
        .bind(t - DAY)
        .bind(t + 29 * DAY)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO deck_shares (deck_id, from_user, to_user, created_at) VALUES (?, ?, ?, ?)")
        .bind(spanish)
        .bind(user)
        .bind(friend)
        .bind(t - DAY)
        .execute(db)
        .await
        .unwrap();

    V1 { user, friend, spanish, renamed_preset: presets[4], review_card, list_card, suspended_card }
}

#[test]
fn v1_migrations_are_unchanged() {
    let all = all_migrations();
    for (version, expected) in V1_CHECKSUMS {
        let m = all.migrations.iter().find(|m| m.version == version).expect("v1 migration still present");
        assert_eq!(hex::encode(&*m.checksum), expected, "migration {version:04} differs from the one on main");
    }
    // New migrations only come after the v1 ones and are numbered in order.
    let versions: Vec<i64> = all.migrations.iter().map(|m| m.version).collect();
    assert_eq!(versions, (1..=versions.len() as i64).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_v1_database_upgrades_without_losing_anything() {
    let db = v1_database().await;
    let v1 = seed_v1(&db).await;
    let count = |sql: &'static str| {
        let db = db.clone();
        async move { sqlx::query_scalar::<_, i64>(sql).fetch_one(&db).await.unwrap() }
    };
    let before = [
        count("SELECT COUNT(*) FROM cards").await,
        count("SELECT COUNT(*) FROM review_log").await,
        count("SELECT COUNT(*) FROM card_items").await,
        count("SELECT COUNT(*) FROM item_review_log").await,
        count("SELECT COUNT(*) FROM decks").await,
        count("SELECT COUNT(*) FROM presets").await,
    ];
    let card_before: (f64, f64, i64, i64, i64) =
        sqlx::query_as("SELECT stability, difficulty, due, reps, lapses FROM cards WHERE id = ?")
            .bind(v1.review_card)
            .fetch_one(&db)
            .await
            .unwrap();

    // What production does on the first v2 start.
    all_migrations().run(&db).await.unwrap();
    let app = Arc::new(App::new(db.clone(), config()));
    app.resplit_multicards().await.unwrap();
    // A second start changes nothing.
    all_migrations().run(&db).await.unwrap();
    app.resplit_multicards().await.unwrap();

    let after = [
        count("SELECT COUNT(*) FROM cards").await,
        count("SELECT COUNT(*) FROM review_log").await,
        count("SELECT COUNT(*) FROM card_items").await,
        count("SELECT COUNT(*) FROM item_review_log").await,
        count("SELECT COUNT(*) FROM decks").await,
        count("SELECT COUNT(*) FROM presets").await,
    ];
    assert_eq!(before, after, "rows survive the upgrade");
    let card_after: (f64, f64, i64, i64, i64) =
        sqlx::query_as("SELECT stability, difficulty, due, reps, lapses FROM cards WHERE id = ?")
            .bind(v1.review_card)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(card_before, card_after, "memory and schedule are untouched");

    // The old password and the old session still work.
    let user = app.authenticate("kolja#1234", "password123").await.unwrap();
    assert_eq!(user.id, v1.user);
    assert_eq!(app.session_user("v1-session").await.unwrap().map(|u| u.id), Some(v1.user));
    // v1 settings are kept; v2 settings start at their defaults.
    assert_eq!((user.font_scale, user.desktop_look.as_str(), user.timezone.as_str()), (110, "calm", "Europe/Berlin"));
    assert!(user.auto_optimize);
    assert!(!user.catch_up && !user.confidence_check && !user.is_admin);
    assert_eq!((user.session_minutes, user.catch_up_reviews), (0, 100));
    assert_eq!((user.quiet_from, user.quiet_to, user.invited_by), (None, None, None));

    // The presets v1 made learn their template by name; renamed ones don't.
    let templates: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT name, template FROM presets WHERE user_id = ? ORDER BY id")
            .bind(v1.user)
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(
        templates,
        vec![
            ("Standard".into(), Some("standard".into())),
            ("Exam prep".into(), Some("exam".into())),
            ("Language vocabulary".into(), Some("language".into())),
            ("Relaxed".into(), Some("relaxed".into())),
            ("My own pace".into(), None),
            ("Custom".into(), None),
        ]
    );
    let a = Access::web(user.clone());
    let presets = app.list_presets(&a).await.unwrap();
    assert_eq!(presets.len(), 6);
    let renamed = presets.iter().find(|p| p.id == v1.renamed_preset).unwrap();
    assert_eq!((renamed.time_limit, renamed.typed_answers), (0, false));
    // Decks on a renamed preset still use the user's FSRS parameters.
    assert_eq!(app.deck_params(&a, v1.spanish).await.unwrap(), user.params());

    // Studying picks up where v1 left off: two overdue reviews and the new
    // card are due; the suspended one is not.
    let next = app.next_card(&a, None).await.unwrap().unwrap();
    assert_eq!(next.remaining, 3);
    let list = app.card(&a, v1.list_card).await.unwrap();
    let items = app.card_items(&list).await.unwrap();
    assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["clergy", "nobility", "commoners"]);
    assert!(items.iter().all(|i| i.reps == 2 && i.stability.is_some()), "items keep their memory");
    assert!(app.card(&a, v1.suspended_card).await.unwrap().suspended);
    let r = app.answer_card(&a, v1.review_card, Rating::Good, Some(2000), Some(4)).await.unwrap();
    assert_eq!(r.card.reps, 5);
    app.undo_last(&a).await.unwrap().unwrap();

    // v2 features on old data: stats, the week, the coach and the optimizer.
    let stats = app.stats(&a, None).await.unwrap();
    assert_eq!(stats.new, 1);
    app.week(&a).await.unwrap();
    app.deck_retention(&a).await.unwrap();
    app.calibration(&a, None).await.unwrap();
    app.attention(&a, None).await.unwrap();
    app.workload(&a, v1.renamed_preset, 30).await.unwrap();
    app.optimize_parameters(&a).await.unwrap();

    // Accounts older than a week may invite right away; the share made in
    // v1 can still be accepted.
    let invite = app.create_invite(&user).await.unwrap();
    assert!(app.open_invite(&invite.token).await.unwrap().is_some());
    let friend = Access::web(app.user(v1.friend).await.unwrap());
    let shares = app.incoming_shares(&friend).await.unwrap();
    assert_eq!(shares.len(), 1);
    let copy = app.accept_share(&friend, shares[0].id).await.unwrap();
    assert_eq!(copy.name, "Languages::Spanish");

    // Signing up stays closed: there are accounts, and no invite was given.
    assert!(!app.signup_open().await);
}

#[tokio::test]
async fn an_empty_v1_database_upgrades_and_opens_signup() {
    let db = v1_database().await;
    all_migrations().run(&db).await.unwrap();
    let app = Arc::new(App::new(db, config()));
    app.resplit_multicards().await.unwrap();
    assert!(app.signup_open().await, "the first account can always sign up");
}
