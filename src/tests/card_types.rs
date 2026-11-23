//! Card types people bring: uploading one, writing cards with it, studying
//! them, and what stays out of reach.

use std::{collections::BTreeMap, io::Write};

use zip::{ZipWriter, write::SimpleFileOptions};

use super::*;

const MANIFEST: &str = r#"{
    "name": "Chess",
    "module": "chess.js",
    "fields": [
        {"key": "fen", "label": "Position"},
        {"key": "moves", "label": "Solution"},
        {"key": "note", "label": "Note", "kind": "text", "optional": true}
    ],
    "summary": {"front": "Chess: {fen}", "back": "{moves}\n\n{note}"}
}"#;

pub(super) fn package(js: &str) -> Vec<u8> {
    let mut w = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in [("card-type.json", MANIFEST), ("chess.js", js)] {
        w.start_file(name, SimpleFileOptions::default()).unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

fn fields(fen: &str, moves: &str) -> BTreeMap<String, String> {
    [("fen".to_string(), fen.to_string()), ("moves".to_string(), moves.to_string())].into_iter().collect()
}

#[tokio::test]
async fn a_brought_type_writes_and_studies_cards() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Tactics", "", None).await.unwrap();
    let v1 = app.upload_card_type(&a, &package("export {}")).await.unwrap();
    assert_eq!((v1.name.as_str(), v1.version, v1.token.len()), ("Chess", 1, 32));
    assert_eq!(app.card_type_file(&v1.token, "chess.js").await.unwrap().1, b"export {}");
    assert!(matches!(app.card_type_file(&v1.token, "card-type.json").await, Err(AppError::NotFound)));

    let card = app.create_typed_card(&a, deck.id, v1.id, &fields("8/8 w", "Nf3")).await.unwrap();
    assert_eq!((card.front.as_str(), card.back.as_str(), card.type_id), ("Chess: 8/8 w", "Nf3", Some(v1.id)));
    // Its sides are the type's to write.
    let edit = CardInput { front: Some("x".into()), ..CardInput::default() };
    assert!(matches!(app.update_card(&a, card.id, edit).await, Err(AppError::BadRequest(_))));
    let card = app.update_typed_card(&a, card.id, &fields("8/8 b", "Nc6")).await.unwrap();
    assert_eq!(card.front, "Chess: 8/8 b");
    let typed = app.typed_card(&card).await.unwrap().unwrap();
    assert_eq!(typed.fields_json(), r#"{"fen":"8/8 b","moves":"Nc6"}"#);

    // A new upload is a new version; the card keeps its own.
    let v2 = app.upload_card_type(&a, &package("export const v = 2")).await.unwrap();
    assert_eq!(v2.version, 2);
    let latest = app.latest_card_types(&a).await.unwrap();
    assert_eq!(latest.iter().map(|t| t.id).collect::<Vec<_>>(), [v2.id]);
    assert_eq!(app.typed_card(&card).await.unwrap().unwrap().ty.id, v1.id);

    // The learner's response goes with the answer.
    app.answer_card(&a, card.id, Rating::Good, None, Some(0)).await.unwrap();
    app.note_response(&a, card.id, r#"{"move": "Nc6"}"#).await.unwrap();
    let stored: Option<String> = sqlx::query_scalar("SELECT response FROM review_log WHERE card_id = ?")
        .bind(card.id)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(stored.as_deref(), Some(r#"{"move":"Nc6"}"#));
    assert!(app.note_response(&a, card.id, "<script>").await.is_err());

    // A version in use stays; an unused one can go.
    assert!(matches!(app.delete_card_type(&a, v1.id).await, Err(AppError::Conflict(_))));
    app.delete_card_type(&a, v2.id).await.unwrap();
}

#[tokio::test]
async fn types_are_the_uploaders_own() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let b = user(&app, "mallory").await;
    let ty = app.upload_card_type(&a, &package("")).await.unwrap();
    let deck = app.create_deck(&b, "Mine", "", None).await.unwrap();
    assert!(matches!(app.card_type(&b, ty.id).await, Err(AppError::NotFound)));
    assert!(matches!(app.delete_card_type(&b, ty.id).await, Err(AppError::NotFound)));
    let stolen = app.create_typed_card(&b, deck.id, ty.id, &fields("a", "b")).await;
    assert!(matches!(stolen, Err(AppError::NotFound)));
    assert!(app.card_types(&b).await.unwrap().is_empty());
}
