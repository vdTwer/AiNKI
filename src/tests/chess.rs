//! Chess cards: a card kind whose sides are a position and the move that
//! answers it. The rules themselves are tested in `chess`; here, how the
//! kind travels through saving, editing, copying and answering.

use super::*;
use crate::{kind::CardKind, merge::MergeInput, multi::Split};

const SCHOLAR: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4";

#[tokio::test]
async fn chess_cards_are_checked_and_saved_as_san() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Tactics", "", None).await.unwrap();
    let card = app
        .create_card_of(&a, deck.id, CardKind::Chess, &format!("{SCHOLAR}\n\nMate in one"), "h5f7\n\nf7 falls.", None, None, None)
        .await
        .unwrap();
    assert_eq!(card.kind, CardKind::Chess);
    assert_eq!(card.back, "Qxf7#\n\nf7 falls.");
    assert_eq!(card.to_json(&a.user.clock(), &[])["kind"], "chess");

    let bad = app.create_card_of(&a, deck.id, CardKind::Chess, SCHOLAR, "Qxf8", None, None, None).await;
    assert!(matches!(bad, Err(AppError::BadRequest(m)) if m.contains("“Qxf8”")));
    let listy = app.create_card_of(&a, deck.id, CardKind::Chess, SCHOLAR, "Qxf7", None, None, Some(Split::List)).await;
    assert!(matches!(listy, Err(AppError::BadRequest(m)) if m.contains("item by item")));

    // Editing checks again; a text card can become a chess card and back.
    let edit = |back: &str| CardInput { back: Some(back.into()), ..Default::default() };
    assert!(app.update_card(&a, card.id, edit("Ke3")).await.is_err());
    assert_eq!(app.update_card(&a, card.id, edit("Qxf7, Nc3")).await.unwrap().back, "Qxf7#, Nc3");
    let text = app.create_card(&a, deck.id, SCHOLAR, "Qxf7", None, None, None).await.unwrap();
    assert_eq!(text.kind, CardKind::Basic);
    let turned = app.update_card(&a, text.id, CardInput { kind: Some(CardKind::Chess), ..Default::default() }).await.unwrap();
    assert_eq!((turned.kind, turned.back.as_str()), (CardKind::Chess, "Qxf7#"));
    let back = app.update_card(&a, text.id, CardInput { kind: Some(CardKind::Basic), back: Some("*mate*".into()), ..Default::default() }).await.unwrap();
    assert_eq!((back.kind, back.back.as_str()), (CardKind::Basic, "*mate*"));
}

#[tokio::test]
async fn a_played_move_grades_itself() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Tactics", "", None).await.unwrap();
    let card = app.create_card_of(&a, deck.id, CardKind::Chess, SCHOLAR, "Qxf7#", None, None, None).await.unwrap();
    let (_, _, right) = app.play_move(&a, card.id, "h5f7").await.unwrap();
    assert_eq!(right.rating(), Rating::Good);
    let (_, _, wrong) = app.play_move(&a, card.id, "Nc3").await.unwrap();
    assert_eq!(wrong.rating(), Rating::Again);
    assert!(app.play_move(&a, card.id, "Ke3").await.is_err(), "not legal");
    let text = app.create_card(&a, deck.id, "Hund", "dog", None, None, None).await.unwrap();
    assert!(app.play_move(&a, text.id, "e4").await.is_err(), "not a chess card");
}

#[tokio::test]
async fn copies_stay_chess_cards() {
    let app = app().await;
    let kolja = user(&app, "kolja").await;
    let anna = user(&app, "anna").await;
    let deck = app.create_deck(&kolja, "Tactics", "", None).await.unwrap();
    app.create_card_of(&kolja, deck.id, CardKind::Chess, SCHOLAR, "Qxf7#", None, None, None).await.unwrap();
    let kinds = |deck_id: i64, who: Access| {
        let app = app.clone();
        async move {
            let filter = CardFilter { deck_id: Some(deck_id), ..Default::default() };
            app.list_cards(&who, &filter).await.unwrap().0.into_iter().map(|c| c.kind).collect::<Vec<_>>()
        }
    };

    let other = app.create_deck(&kolja, "Openings", "", None).await.unwrap();
    let merged = app
        .merge_decks(
            &kolja,
            MergeInput {
                sources: vec![deck.id, other.id],
                name: "All chess".into(),
                description: String::new(),
                preset_id: None,
                keep_progress: false,
                delete_sources: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(kinds(merged.deck.id, kolja.clone()).await, [CardKind::Chess]);

    app.share_deck(&kolja, deck.id, &anna.user.handle()).await.unwrap();
    let share = app.incoming_shares(&anna).await.unwrap().remove(0);
    let copy = app.accept_share(&anna, share.id).await.unwrap();
    assert_eq!(kinds(copy.id, anna.clone()).await, [CardKind::Chess]);
}

#[tokio::test]
async fn importing_positions_makes_chess_cards() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Puzzles", "", None).await.unwrap();
    let report = app
        .import_chess(&a, deck.id, &format!("{SCHOLAR} bm Qxf7#; c0 \"mate\";\n{SCHOLAR} Ke3\nnonsense here\n"))
        .await
        .unwrap();
    assert_eq!(report.cards_imported, 1);
    assert_eq!(report.problems.len(), 2);
    assert!(report.problems[0].starts_with("line 2: "), "{:?}", report.problems);
    assert!(app.import_chess(&a, deck.id, "\n# nothing\n").await.is_err());
}
