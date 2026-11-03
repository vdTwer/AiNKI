//! Importing Anki packages (.apkg), end to end: a real collection file is
//! written with SQLite, zipped with its media and handed to the importer.

use std::{io::Write, str::FromStr};

use sqlx::{ConnectOptions, Connection, sqlite::SqliteConnectOptions};

use super::*;

const DAY: i64 = 86_400;

/// One Anki card: its note's fields, deck, template and scheduling.
struct Card {
    note: i64,
    fields: &'static str,
    deck: i64,
    ord: i64,
    ctype: i64,
    queue: i64,
    due: i64,
    ivl: i64,
    factor: i64,
    /// Original deck while the card sits in a filtered deck (0: none).
    odid: i64,
    odue: i64,
    /// (days ago, ease, revlog type)
    revlog: &'static [(i64, i64, i64)],
}

const fn card(note: i64, fields: &'static str, deck: i64) -> Card {
    Card { note, fields, deck, ord: 0, ctype: 0, queue: 0, due: 0, ivl: 0, factor: 0, odid: 0, odue: 0, revlog: &[] }
}

/// Collection format: the legacy schema (decks as JSON in `col`) or the
/// newer one (a `decks` table, zstd-compressed `collection.anki21b`).
#[derive(Clone, Copy, PartialEq)]
enum Format {
    Legacy,
    Modern,
}

async fn collection(format: Format, crt: i64, decks: &[(i64, &str)], cards: &[Card]) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("anki-{}.sqlite", uuid::Uuid::new_v4().simple()));
    let mut conn = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .unwrap()
        .create_if_missing(true)
        .connect()
        .await
        .unwrap();
    for sql in [
        "CREATE TABLE col (id INTEGER PRIMARY KEY, crt INTEGER NOT NULL, decks TEXT NOT NULL)",
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, flds TEXT NOT NULL)",
        "CREATE TABLE cards (id INTEGER PRIMARY KEY, nid INTEGER, did INTEGER, ord INTEGER, type INTEGER,
            queue INTEGER, due INTEGER, ivl INTEGER, factor INTEGER, reps INTEGER, lapses INTEGER,
            odue INTEGER NOT NULL DEFAULT 0, odid INTEGER NOT NULL DEFAULT 0)",
        "CREATE TABLE revlog (id INTEGER PRIMARY KEY, cid INTEGER, ease INTEGER, type INTEGER)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    let json: serde_json::Map<String, serde_json::Value> =
        decks.iter().map(|(id, name)| (id.to_string(), serde_json::json!({ "name": name }))).collect();
    let blob = if format == Format::Legacy { serde_json::Value::Object(json).to_string() } else { "{}".into() };
    sqlx::query("INSERT INTO col (id, crt, decks) VALUES (1, ?, ?)").bind(crt).bind(blob).execute(&mut conn).await.unwrap();
    if format == Format::Modern {
        sqlx::query("CREATE TABLE decks (id INTEGER PRIMARY KEY, name TEXT NOT NULL)").execute(&mut conn).await.unwrap();
        for (id, name) in decks {
            // The modern schema separates nested names with \x1f.
            sqlx::query("INSERT INTO decks (id, name) VALUES (?, ?)")
                .bind(id)
                .bind(name.replace("::", "\x1f"))
                .execute(&mut conn)
                .await
                .unwrap();
        }
    }
    let now_ms = now() * 1000;
    for (i, c) in cards.iter().enumerate() {
        sqlx::query("INSERT OR IGNORE INTO notes (id, flds) VALUES (?, ?)")
            .bind(c.note)
            .bind(c.fields.replace('|', "\x1f"))
            .execute(&mut conn)
            .await
            .unwrap();
        let id = 1000 + i as i64;
        sqlx::query(
            "INSERT INTO cards (id, nid, did, ord, type, queue, due, ivl, factor, reps, lapses, odue, odid)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
        )
        .bind(id)
        .bind(c.note)
        .bind(c.deck)
        .bind(c.ord)
        .bind(c.ctype)
        .bind(c.queue)
        .bind(c.due)
        .bind(c.ivl)
        .bind(c.factor)
        .bind(c.revlog.len() as i64)
        .bind(c.odue)
        .bind(c.odid)
        .execute(&mut conn)
        .await
        .unwrap();
        for (j, (days_ago, ease, kind)) in c.revlog.iter().enumerate() {
            sqlx::query("INSERT INTO revlog (id, cid, ease, type) VALUES (?, ?, ?, ?)")
                .bind(now_ms - days_ago * DAY * 1000 + j as i64)
                .bind(id)
                .bind(ease)
                .bind(kind)
                .execute(&mut conn)
                .await
                .unwrap();
        }
    }
    conn.close().await.unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    bytes
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(8, 8, |x, y| image::Rgb([(x * 30) as u8, (y * 30) as u8, 90]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn package(format: Format, collection: Vec<u8>, media: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    match format {
        Format::Legacy => {
            zip.start_file("collection.anki2", opts).unwrap();
            zip.write_all(&collection).unwrap();
        }
        Format::Modern => {
            zip.start_file("collection.anki21b", opts).unwrap();
            zip.write_all(&zstd::encode_all(&collection[..], 3).unwrap()).unwrap();
        }
    }
    let index: serde_json::Map<String, serde_json::Value> =
        media.iter().enumerate().map(|(i, (name, _))| (i.to_string(), serde_json::json!(name))).collect();
    zip.start_file("media", opts).unwrap();
    zip.write_all(serde_json::Value::Object(index).to_string().as_bytes()).unwrap();
    for (i, (_, bytes)) in media.iter().enumerate() {
        zip.start_file(i.to_string(), opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

async fn cards_in(app: &App, a: &Access, deck: &str) -> Vec<crate::service::Card> {
    let deck = app.deck_by_name(a, deck).await.unwrap().unwrap_or_else(|| panic!("deck {deck} exists"));
    let filter = CardFilter { deck_id: Some(deck.id), query: None, state: None, limit: Some(100), offset: None };
    app.list_cards(a, &filter).await.unwrap().0
}

#[tokio::test]
async fn a_legacy_package_brings_cards_history_and_images() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    // An existing deck with the same name is reused, not doubled.
    app.create_deck(&a, "Languages::Spanish", "", None).await.unwrap();
    let crt = now() - 400 * DAY;
    let today_in_anki = (now() - crt) / DAY;
    let cards = [
        card(1, "<b>el perro</b>|the dog", 10),
        // "Basic (and reversed)": the second card asks the other way round.
        Card { ord: 1, ..card(2, "la casa|the house", 10) },
        // A cloze note with two gaps makes two cards.
        card(3, "{{c1::Madrid}} is the capital of {{c2::Spain}}|", 11),
        Card { ord: 1, ..card(3, "{{c1::Madrid}} is the capital of {{c2::Spain}}|", 11) },
        // Learned in Anki from new: its memory comes from the history.
        Card {
            ctype: 2,
            queue: 2,
            due: today_in_anki + 5,
            ivl: 12,
            factor: 2500,
            revlog: &[(30, 3, 0), (29, 3, 0), (20, 3, 1), (8, 3, 1)],
            ..card(4, "el gato|the cat", 10)
        },
        // Learned, but the history was cleared: memory from ease and interval.
        Card { ctype: 2, queue: 2, due: today_in_anki + 2, ivl: 30, factor: 2300, ..card(5, "el sol|the sun", 10) },
        // Suspended before it was ever studied.
        Card { queue: -1, ..card(6, "la luna|the moon", 10) },
        // An image on the front.
        card(7, "<img src=\"bandera.png\">|the flag", 11),
        // Nothing to ask: skipped.
        card(8, "|orphan back", 11),
    ];
    let col = collection(Format::Legacy, crt, &[(10, "Languages::Spanish"), (11, "Geography")], &cards).await;
    let pkg = package(Format::Legacy, col, &[("bandera.png", png()), ("sound.mp3", vec![1, 2, 3])]);

    let report = app.import_apkg(&a, pkg).await.unwrap();
    assert_eq!(report.decks_created, vec!["Geography".to_string()]);
    assert_eq!(report.cards_imported, 8);
    assert_eq!(report.skipped, 1);
    assert_eq!(report.reviews_imported, 4);
    assert_eq!(report.images_imported, 1, "images only; sounds are left out");

    let spanish = cards_in(&app, &a, "Languages::Spanish").await;
    let by_front = |front: &str| spanish.iter().find(|c| c.front == front).unwrap_or_else(|| panic!("card {front}"));
    assert_eq!(by_front("el perro").back, "the dog", "HTML is stripped");
    assert_eq!(by_front("the house").back, "la casa", "the reverse card is reversed");
    let cat = by_front("el gato");
    assert_eq!(cat.card_state(), CardState::Review);
    assert!(cat.stability.is_some_and(|s| s > 1.0));
    let day = a.user.clock().day_start(a.user.clock().date_of(crt + (today_in_anki + 5) * DAY));
    assert_eq!(cat.due, day, "due on the same day as in Anki");
    let sun = by_front("el sol");
    assert_eq!(sun.card_state(), CardState::Review);
    assert!(sun.stability.is_some(), "memory from SM-2 ease and interval");
    assert!(by_front("la luna").suspended);

    let geo = cards_in(&app, &a, "Geography").await;
    let fronts: Vec<&str> = geo.iter().map(|c| c.front.as_str()).collect();
    assert!(fronts.contains(&"**[…]** is the capital of Spain"), "{fronts:?}");
    assert!(fronts.contains(&"Madrid is the capital of **[…]**"), "{fronts:?}");
    let flag = geo.iter().find(|c| c.back == "the flag").unwrap();
    assert!(flag.front_image.is_some());

    // The imported history counts for the optimizer and the log.
    let logged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_log WHERE source = 'import'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(logged, 4);
}

#[tokio::test]
async fn a_modern_package_reads_nested_deck_names() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let col = collection(Format::Modern, now() - 10 * DAY, &[(5, "Uni::Biology::Cells")], &[card(1, "Mitochondria|powerhouse", 5)]).await;
    let report = app.import_apkg(&a, package(Format::Modern, col, &[])).await.unwrap();
    assert_eq!(report.decks_created, vec!["Uni::Biology::Cells".to_string()]);
    assert_eq!(cards_in(&app, &a, "Uni::Biology::Cells").await[0].back, "powerhouse");
}

#[tokio::test]
async fn importing_twice_adds_the_cards_twice() {
    // Documents today's behavior: there is no duplicate check on import.
    let app = app().await;
    let a = user(&app, "kolja").await;
    let col = collection(Format::Legacy, now(), &[(1, "Default")], &[card(1, "a|b", 1)]).await;
    let pkg = package(Format::Legacy, col, &[]);
    app.import_apkg(&a, pkg.clone()).await.unwrap();
    let again = app.import_apkg(&a, pkg).await.unwrap();
    assert!(again.decks_created.is_empty());
    assert_eq!(cards_in(&app, &a, "Default").await.len(), 2);
}

#[tokio::test]
#[ignore = "bug: cards in an Anki filtered deck land in that deck with its temporary due date"]
async fn cards_in_a_filtered_deck_go_home() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let crt = now() - 100 * DAY;
    let home_due = (now() - crt) / DAY + 3;
    // While in a filtered deck, Anki keeps the real deck in odid and the
    // real due day in odue; due is a position like -100000.
    let c = Card {
        ctype: 2,
        queue: 2,
        due: -100_000,
        ivl: 10,
        factor: 2500,
        odid: 1,
        odue: home_due,
        ..card(1, "filtered|card", 2)
    };
    let col = collection(Format::Legacy, crt, &[(1, "Spanish"), (2, "Filtered Deck 1")], &[c]).await;
    let report = app.import_apkg(&a, package(Format::Legacy, col, &[])).await.unwrap();
    assert_eq!(report.decks_created, vec!["Spanish".to_string()]);
    let card = &cards_in(&app, &a, "Spanish").await[0];
    assert!(card.due > now(), "due in three days, not long ago");
}

#[tokio::test]
async fn broken_packages_and_narrow_tokens_are_refused() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    for bad in [b"not a zip".to_vec(), package(Format::Legacy, b"not sqlite".to_vec(), &[])] {
        assert!(matches!(app.import_apkg(&a, bad).await, Err(AppError::BadRequest(_))));
    }
    let empty = {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.finish().unwrap().into_inner()
    };
    assert!(matches!(app.import_apkg(&a, empty).await, Err(AppError::BadRequest(_))));

    // A token limited to some decks may not import (it would make decks).
    let deck = app.create_deck(&a, "Only this", "", None).await.unwrap();
    let (_, raw) = app.create_token(a.user_id(), "narrow", true, true, Some(vec![deck.id])).await.unwrap();
    let narrow = app.token_access(&raw).await.unwrap();
    let col = collection(Format::Legacy, now(), &[(1, "X")], &[card(1, "a|b", 1)]).await;
    assert!(app.import_apkg(&narrow, package(Format::Legacy, col, &[])).await.is_err());
    // Nothing was left behind in the data folder's tmp directory.
    let left = std::fs::read_dir(app.config.data_dir.join("tmp")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(left, 0);
}
