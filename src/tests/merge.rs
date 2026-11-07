//! Merged decks: a new deck made of copies of other decks' cards. The
//! decks it is made from never change.

use super::*;
use crate::{merge::MergeInput, multi::Split};

const LIST: &str = "- one\n- two\n- three";

/// Two decks with two cards each, one card answered once.
async fn two_decks(app: &App, a: &Access) -> (i64, i64) {
    let spanish = app.create_deck(a, "Spanish", "", None).await.unwrap();
    let french = app.create_deck(a, "French", "", None).await.unwrap();
    app.create_card(a, spanish.id, "hablar", "to speak", None, None, None).await.unwrap();
    app.create_card(a, spanish.id, "Numbers?", LIST, None, None, Some(Split::List)).await.unwrap();
    app.create_card(a, french.id, "parler", "to speak", None, None, None).await.unwrap();
    app.create_card(a, french.id, "manger", "to eat", None, None, None).await.unwrap();
    let first = app.next_card(a, Some(spanish.id)).await.unwrap().unwrap();
    app.answer_card(a, first.card.id, Rating::Good, None, Some(0)).await.unwrap();
    (spanish.id, french.id)
}

fn input(sources: Vec<i64>, keep_progress: bool) -> MergeInput {
    MergeInput {
        sources,
        name: "Languages".into(),
        description: "All of them".into(),
        preset_id: None,
        keep_progress,
    }
}

/// Every card of a deck, as (front, reps, suspended), in deck order.
async fn cards_of(app: &App, a: &Access, deck_id: i64) -> Vec<(String, i64, bool)> {
    let filter = CardFilter { deck_id: Some(deck_id), limit: Some(500), ..Default::default() };
    let (mut cards, _) = app.list_cards(a, &filter).await.unwrap();
    cards.sort_by_key(|c| c.position);
    cards.into_iter().map(|c| (c.front, c.reps, c.suspended)).collect()
}

#[tokio::test]
async fn merging_copies_cards_and_leaves_the_originals_alone() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    let before = (cards_of(&app, &a, spanish).await, cards_of(&app, &a, french).await);

    let merged = app.merge_decks(&a, input(vec![spanish, french], false)).await.unwrap();
    assert_eq!(merged.deck.name, "Languages");
    assert_eq!(merged.deck.description, "All of them");
    assert_eq!((merged.cards, merged.duplicates), (4, 0));

    // The copies keep the order of the decks as picked, and start fresh.
    let copies = cards_of(&app, &a, merged.deck.id).await;
    let fronts: Vec<&str> = copies.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(fronts, ["hablar", "Numbers?", "parler", "manger"]);
    assert!(copies.iter().all(|c| c.1 == 0));

    // The originals are exactly as they were, and still there.
    assert_eq!((cards_of(&app, &a, spanish).await, cards_of(&app, &a, french).await), before);
    assert_eq!(app.decks(&a).await.unwrap().len(), 3);

    // A copy is its own card: editing it leaves the original alone.
    let filter = CardFilter { deck_id: Some(merged.deck.id), limit: Some(500), ..Default::default() };
    let (copies, _) = app.list_cards(&a, &filter).await.unwrap();
    let input = CardInput { front: Some("hablar (es)".into()), ..Default::default() };
    let hablar = copies.iter().find(|c| c.front == "hablar").unwrap();
    app.update_card(&a, hablar.id, input).await.unwrap();
    assert_eq!(cards_of(&app, &a, spanish).await[0].0, "hablar");

    // Deleting the merged deck takes only its copies with it.
    app.delete_deck(&a, merged.deck.id).await.unwrap();
    assert_eq!((cards_of(&app, &a, spanish).await, cards_of(&app, &a, french).await), before);
}

#[tokio::test]
async fn a_fresh_multicard_copy_keeps_its_items_but_not_their_memory() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    // Answer the multicard too, so its items have a memory to leave behind.
    let multi = app.next_card(&a, Some(spanish)).await.unwrap().unwrap();
    assert_eq!(multi.card.front, "Numbers?");
    app.answer_list(&a, multi.card.id, vec![], None, Some(0)).await.unwrap();

    let merged = app.merge_decks(&a, input(vec![spanish, french], false)).await.unwrap();
    let filter = CardFilter { deck_id: Some(merged.deck.id), limit: Some(500), ..Default::default() };
    let (copies, _) = app.list_cards(&a, &filter).await.unwrap();
    let copy = copies.iter().find(|c| c.front == "Numbers?").unwrap();
    assert_eq!(copy.multi, Some(Split::List));
    let items = app.card_items(copy).await.unwrap();
    assert_eq!(items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["one", "two", "three"]);
    assert!(items.iter().all(|i| i.reps == 0 && i.stability.is_none()));
    // The copy is new, so studying the merged deck asks it.
    assert_eq!(app.next_card(&a, Some(merged.deck.id)).await.unwrap().unwrap().card.front, "hablar");
}

#[tokio::test]
async fn keeping_progress_copies_the_schedule_of_cards_and_items() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    let multi = app.next_card(&a, Some(spanish)).await.unwrap().unwrap();
    app.answer_list(&a, multi.card.id, vec![2], None, Some(0)).await.unwrap();
    let original = app.card(&a, multi.card.id).await.unwrap();
    let original_items = app.card_items(&original).await.unwrap();

    let merged = app.merge_decks(&a, input(vec![spanish, french], true)).await.unwrap();
    let filter = CardFilter { deck_id: Some(merged.deck.id), limit: Some(500), ..Default::default() };
    let (copies, _) = app.list_cards(&a, &filter).await.unwrap();
    let copy = copies.iter().find(|c| c.front == "Numbers?").unwrap();
    assert_ne!(copy.id, original.id);
    let sched = |c: &crate::service::Card| (c.state, c.step, c.stability, c.difficulty, c.due, c.last_review, c.reps, c.lapses);
    assert_eq!(sched(copy), sched(&original));
    let items = app.card_items(copy).await.unwrap();
    let item_sched =
        |i: &crate::multi::CardItem| (i.position, i.text.clone(), i.state, i.stability, i.due, i.reps, i.lapses);
    assert_eq!(items.iter().map(item_sched).collect::<Vec<_>>(), original_items.iter().map(item_sched).collect::<Vec<_>>());

    let hablar = copies.iter().find(|c| c.front == "hablar").unwrap();
    assert_eq!(hablar.reps, 1);
    // Review history stays with the original, so stats don't count it twice.
    assert!(app.card_history(&a, hablar.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_same_card_in_two_decks_is_copied_once() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    app.create_card(&a, french, "hablar", "to speak", None, None, None).await.unwrap();
    // Same front, other back: not the same card.
    app.create_card(&a, french, "manger", "eating", None, None, None).await.unwrap();

    let merged = app.merge_decks(&a, input(vec![spanish, french], false)).await.unwrap();
    assert_eq!((merged.cards, merged.duplicates), (5, 1));
}

#[tokio::test]
async fn suspended_cards_stay_suspended() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    let filter = CardFilter { deck_id: Some(french), limit: Some(500), ..Default::default() };
    let (french_cards, _) = app.list_cards(&a, &filter).await.unwrap();
    let parler = french_cards.iter().find(|c| c.front == "parler").unwrap();
    app.set_suspended(&a, parler.id, true).await.unwrap();

    for keep in [false, true] {
        let mut i = input(vec![spanish, french], keep);
        i.name = format!("Merged {keep}");
        let merged = app.merge_decks(&a, i).await.unwrap();
        let suspended: Vec<String> =
            cards_of(&app, &a, merged.deck.id).await.into_iter().filter(|c| c.2).map(|c| c.0).collect();
        assert_eq!(suspended, ["parler"]);
    }
}

#[tokio::test]
async fn the_merged_deck_uses_the_first_decks_preset_unless_one_is_picked() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;
    let presets = app.list_presets(&a).await.unwrap();
    let current = app.deck(&a, french).await.unwrap().preset_id;
    let other = presets.iter().find(|p| p.id != current).unwrap();
    app.update_deck(&a, french, None, None, Some(other.id), None).await.unwrap();

    let merged = app.merge_decks(&a, input(vec![french, spanish], false)).await.unwrap();
    assert_eq!(merged.deck.preset_id, other.id);

    let mut picked = input(vec![french, spanish], false);
    picked.name = "Picked".into();
    picked.preset_id = Some(presets[0].id);
    assert_eq!(app.merge_decks(&a, picked).await.unwrap().deck.preset_id, presets[0].id);
}

#[tokio::test]
async fn merging_needs_two_own_decks_and_a_free_name() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (spanish, french) = two_decks(&app, &a).await;

    let one = app.merge_decks(&a, input(vec![spanish, spanish], false)).await;
    assert!(matches!(one, Err(AppError::BadRequest(_))), "{one:?}");

    let mut taken = input(vec![spanish, french], false);
    taken.name = "spanish".into();
    assert!(matches!(app.merge_decks(&a, taken).await, Err(AppError::Conflict(_))));

    let mut empty = input(vec![spanish, french], false);
    empty.name = "  ".into();
    assert!(matches!(app.merge_decks(&a, empty).await, Err(AppError::BadRequest(_))));

    // Someone else's deck can't be merged in.
    let anna = user(&app, "anna").await;
    let hers = app.create_deck(&anna, "Hers", "", None).await.unwrap();
    assert!(app.merge_decks(&a, input(vec![spanish, hers.id], false)).await.is_err());

    // A failed merge leaves no deck behind.
    assert_eq!(app.decks(&a).await.unwrap().len(), 2);
}
