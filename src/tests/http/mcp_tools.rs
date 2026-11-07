//! Every MCP tool, called over HTTP the way Claude calls it, with the
//! permissions a token can have.

use super::*;
use serde_json::{Value, json};

/// Call a tool. `Err` holds the message an agent would see.
async fn call(app: &Arc<App>, token: &str, name: &str, args: Value) -> Result<Value, String> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } });
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
    assert_eq!(res.status(), StatusCode::OK, "{name}");
    let v: Value = serde_json::from_str(&text(res).await).unwrap();
    if let Some(e) = v.get("error") {
        return Err(e["message"].as_str().unwrap_or_default().to_string());
    }
    let item = &v["result"]["content"][0];
    if item["type"] == "image" {
        return Ok(item.clone());
    }
    let content = item["text"].as_str().unwrap_or_else(|| panic!("{name}: {v}"));
    if v["result"]["isError"] == true {
        return Err(content.to_string());
    }
    // Most tools answer with JSON; a few (the CSV export) with plain text.
    Ok(serde_json::from_str(content).unwrap_or_else(|_| Value::String(content.to_string())))
}

async fn ok(app: &Arc<App>, token: &str, name: &str, args: Value) -> Value {
    call(app, token, name, args).await.unwrap_or_else(|e| panic!("{name} failed: {e}"))
}

async fn list_tools(app: &Arc<App>, token: &str) -> Vec<Value> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} });
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
    let v: Value = serde_json::from_str(&text(res).await).unwrap();
    v["result"]["tools"].as_array().unwrap().clone()
}

fn png_base64() -> String {
    use base64::Engine;
    let img = image::RgbImage::from_fn(6, 4, |x, _| image::Rgb([(x * 40) as u8, 120, 200]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(out.into_inner())
}

#[tokio::test]
async fn every_tool_is_listed_with_a_description_and_schema() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (_, token) = app.create_token(a.user_id(), "t", true, true, None).await.unwrap();
    let tools = list_tools(&app, &token).await;
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "get_overview", "list_decks", "create_deck", "update_deck", "delete_deck", "list_cards", "get_card",
        "create_cards", "update_card", "delete_cards", "set_cards_suspended", "reset_cards", "upload_image",
        "get_image", "get_next_card", "answer_card", "undo_last_answer", "get_stats", "list_presets",
        "create_preset", "update_preset", "delete_preset", "get_coach_advice", "list_cards_needing_attention",
        "dismiss_card_flag", "split_card", "get_insights", "get_workload", "list_lessons", "get_lesson",
        "optimize_fsrs", "import_csv", "import_apkg", "share_deck", "list_shares", "accept_share",
        "decline_share", "cancel_share", "export_deck_csv",
    ] {
        assert!(names.contains(&expected), "{expected} is missing from tools/list");
    }
    for t in &tools {
        assert!(!t["description"].as_str().unwrap_or_default().is_empty(), "{} has no description", t["name"]);
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
    }
}

#[tokio::test]
async fn an_agent_can_run_a_whole_session() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    app.create_user("anna", "password123", "UTC").await.unwrap();
    let anna = app.find_user("anna").await.unwrap();
    let (_, t) = app.create_token(a.user_id(), "claude", true, true, None).await.unwrap();
    let t = t.as_str();

    let overview = ok(&app, t, "get_overview", json!({})).await;
    assert_eq!(overview["permissions"]["all_decks"], true);
    assert_eq!(overview["cards_due_now"], 0);

    // Decks.
    let deck = ok(&app, t, "create_deck", json!({ "name": "Spanish", "description": "verbs" })).await;
    let deck_id = deck["id"].as_i64().unwrap();
    let renamed = ok(&app, t, "update_deck", json!({ "deck_id": deck_id, "name": "Languages::Spanish", "exam_date": "2099-01-31" })).await;
    assert_eq!(renamed["name"], "Languages::Spanish");
    assert_eq!(renamed["exam_date"], "2099-01-31");
    let cleared = ok(&app, t, "update_deck", json!({ "deck_id": deck_id, "exam_date": "" })).await;
    assert!(cleared["exam_date"].is_null(), "an empty exam date clears it");
    assert_eq!(ok(&app, t, "list_decks", json!({})).await.as_array().unwrap().len(), 1);

    // Cards, an image and a multicard; a bad card is reported, not fatal.
    let image = ok(&app, t, "upload_image", json!({ "data_base64": format!("data:image/png;base64,{}", png_base64()) })).await;
    let image_id = image["image_id"].as_str().unwrap();
    assert_eq!((image["width"].as_i64(), image["height"].as_i64()), (Some(6), Some(4)));
    let got = ok(&app, t, "get_image", json!({ "image_id": image_id })).await;
    assert_eq!(got["mimeType"], "image/png");
    assert!(!got["data"].as_str().unwrap().is_empty());
    let made = ok(
        &app,
        t,
        "create_cards",
        json!({ "deck_id": deck_id, "cards": [
            { "front": "hablar", "back": "to speak", "front_image_id": image_id },
            { "front": "comer", "back": "to eat" },
            { "front": "Three verbs", "back": "- ser\n- estar\n- ir", "multi": "list" },
            { "front": "", "back": "no front" },
        ] }),
    )
    .await;
    assert_eq!(made["created"], 3);
    assert_eq!(made["errors"].as_array().unwrap().len(), 1);
    let ids: Vec<i64> = made["card_ids"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    let by_name = ok(&app, t, "create_cards", json!({ "deck_name": "Languages::Spanish", "cards": [{ "front": "vivir", "back": "to live" }] })).await;
    assert_eq!(by_name["deck_id"], deck_id, "a deck can be named instead of numbered");

    let listed = ok(&app, t, "list_cards", json!({ "deck_id": deck_id, "query": "comer" })).await;
    assert_eq!(listed["total"], 1);
    let multi = ok(&app, t, "get_card", json!({ "card_id": ids[2] })).await;
    assert_eq!(multi["items"].as_array().unwrap().len(), 3);
    let edited = ok(&app, t, "update_card", json!({ "card_id": ids[1], "back": "to eat (lunch)" })).await;
    assert_eq!(edited["back"], "to eat (lunch)");

    // A study turn with a plain card and a multicard, then undo.
    let next = ok(&app, t, "get_next_card", json!({})).await;
    assert_eq!(next["done"], false);
    let first = next["card"]["id"].as_i64().unwrap();
    let answered = ok(&app, t, "answer_card", json!({ "card_id": first, "rating": "good" })).await;
    assert!(answered["next_review_in"].is_string());
    let undone = ok(&app, t, "undo_last_answer", json!({})).await;
    assert_eq!((undone["undone"].clone(), undone["card_id"].clone()), (json!(true), json!(first)));
    let missed = ok(&app, t, "answer_card", json!({ "card_id": ids[2], "missed": [2] })).await;
    assert_eq!(missed["card_id"], ids[2]);
    let history = ok(&app, t, "get_card", json!({ "card_id": ids[2], "include_history": true })).await;
    assert_eq!(history["history"].as_array().unwrap().len(), 1);
    assert!(call(&app, t, "answer_card", json!({ "card_id": ids[0] })).await.is_err(), "a rating or missed items is needed");

    let stats = ok(&app, t, "get_stats", json!({ "deck_id": deck_id })).await;
    assert_eq!(stats["reviews_today"], 1);
    ok(&app, t, "get_insights", json!({})).await;
    ok(&app, t, "get_coach_advice", json!({})).await;
    ok(&app, t, "list_cards_needing_attention", json!({ "deck_id": deck_id })).await;
    assert!(call(&app, t, "dismiss_card_flag", json!({ "card_id": ids[0], "kind": "nonsense" })).await.is_err());
    ok(&app, t, "dismiss_card_flag", json!({ "card_id": ids[0], "kind": "leech" })).await;

    // Split a card in two; the original goes.
    let split = ok(
        &app,
        t,
        "split_card",
        json!({ "card_id": by_name["card_ids"][0], "cards": [{ "front": "vivir (1)", "back": "a" }, { "front": "vivir (2)", "back": "b" }] }),
    )
    .await;
    assert_eq!(split["created_card_ids"].as_array().unwrap().len(), 2);
    assert!(call(&app, t, "get_card", json!({ "card_id": by_name["card_ids"][0] })).await.is_err());

    // Suspend, reset, delete.
    assert_eq!(ok(&app, t, "set_cards_suspended", json!({ "card_ids": [ids[1]], "suspended": true })).await["suspended"], true);
    assert_eq!(ok(&app, t, "list_cards", json!({ "state": "suspended" })).await["total"], 1);
    assert_eq!(ok(&app, t, "reset_cards", json!({ "card_ids": [ids[2]] })).await["reset"], 1);
    assert_eq!(ok(&app, t, "delete_cards", json!({ "card_ids": [ids[1]] })).await["deleted"], 1);

    // Presets and the workload table.
    let presets = ok(&app, t, "list_presets", json!({})).await;
    assert_eq!(presets["presets"].as_array().unwrap().len(), 5);
    assert_eq!(presets["templates"].as_array().unwrap().len(), 5);
    let preset = ok(&app, t, "create_preset", json!({ "template": "exam", "name": "Finals", "time_limit": 20, "typed_answers": true })).await;
    let preset_id = preset["id"].as_i64().unwrap();
    assert_eq!(preset["time_limit"], 20);
    let preset = ok(&app, t, "update_preset", json!({ "preset_id": preset_id, "desired_retention": 0.93 })).await;
    assert_eq!(preset["desired_retention"], 0.93);
    assert!(call(&app, t, "update_preset", json!({ "preset_id": preset_id, "desired_retention": 1.5 })).await.is_err());
    assert!(call(&app, t, "create_preset", json!({ "template": "nonsense" })).await.is_err());
    let work = ok(&app, t, "get_workload", json!({ "preset_id": preset_id, "days": 30 })).await;
    assert_eq!(work["days"], 30);
    assert_eq!(ok(&app, t, "delete_preset", json!({ "preset_id": preset_id })).await["deleted"], true);

    // Lessons.
    let lessons = ok(&app, t, "list_lessons", json!({})).await;
    let slug = lessons["lessons"][0]["slug"].as_str().unwrap().to_string();
    assert!(!ok(&app, t, "get_lesson", json!({ "slug": slug })).await["body"].as_str().unwrap().is_empty());
    assert!(call(&app, t, "get_lesson", json!({ "slug": "no-such-lesson" })).await.is_err());

    // Optimizing with almost no history is refused with the reason.
    let e = call(&app, t, "optimize_fsrs", json!({})).await.unwrap_err();
    assert!(e.contains("not enough review history"), "{e}");

    // CSV in and out, and an Anki package that isn't one.
    let imported = ok(&app, t, "import_csv", json!({ "deck_id": deck_id, "csv": "uno,one\ndos,two\n" })).await;
    assert_eq!(imported["cards_imported"], 2);
    let csv = ok(&app, t, "export_deck_csv", json!({ "deck_id": deck_id })).await;
    let csv = csv.as_str().unwrap();
    assert!(csv.starts_with("front,back\n") && csv.contains("uno,one"), "{csv}");
    // Exported text that looks like a spreadsheet formula is defused and
    // comes back unchanged on import.
    assert_eq!(crate::import::parse_csv(csv).unwrap().iter().find(|r| r.0 == "Three verbs").unwrap().1, "- ser\n- estar\n- ir");
    assert!(call(&app, t, "import_apkg", json!({ "data_base64": "bm90IGEgemlw" })).await.is_err());
    assert!(call(&app, t, "import_apkg", json!({ "data_base64": "%%%" })).await.is_err(), "bad base64");

    // Sharing: offer, see it from both sides, take it back; offer again and
    // let the other account decline, then accept.
    let share = ok(&app, t, "share_deck", json!({ "deck_id": deck_id, "user": anna.handle() })).await;
    let share_id = share["id"].as_i64().unwrap();
    assert_eq!(ok(&app, t, "list_shares", json!({ "deck_id": deck_id })).await["outgoing"].as_array().unwrap().len(), 1);
    assert_eq!(ok(&app, t, "cancel_share", json!({ "share_id": share_id })).await["cancelled"], true);
    let share = ok(&app, t, "share_deck", json!({ "deck_id": deck_id, "user": anna.handle() })).await;
    let (_, anna_token) = app.create_token(anna.id, "claude", true, true, None).await.unwrap();
    let incoming = ok(&app, &anna_token, "list_shares", json!({})).await;
    assert_eq!(incoming["incoming"][0]["id"], share["id"]);
    assert_eq!(ok(&app, &anna_token, "decline_share", json!({ "share_id": share["id"] })).await["declined"], true);
    let share = ok(&app, t, "share_deck", json!({ "deck_id": deck_id, "user": anna.handle() })).await;
    let copy = ok(&app, &anna_token, "accept_share", json!({ "share_id": share["id"] })).await;
    assert_eq!(copy["name"], "Languages::Spanish");
    assert!(call(&app, t, "share_deck", json!({ "deck_id": deck_id, "user": "nobody#0001" })).await.is_err());

    // Deleting a deck needs an explicit confirm.
    assert!(call(&app, t, "delete_deck", json!({ "deck_id": deck_id, "confirm": false })).await.is_err());
    let gone = ok(&app, t, "delete_deck", json!({ "deck_id": deck_id, "confirm": true })).await;
    assert_eq!(gone["deleted"], true);
    assert!(ok(&app, t, "list_decks", json!({})).await.as_array().unwrap().is_empty());
    // Anna's copy is hers and stays.
    assert_eq!(ok(&app, &anna_token, "list_decks", json!({})).await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn tokens_only_do_what_they_were_given() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let mine = app.create_deck(&a, "Mine", "", None).await.unwrap();
    let other = app.create_deck(&a, "Private", "", None).await.unwrap();
    let card = app.create_card(&a, mine.id, "Hund", "dog", None, None, None).await.unwrap();
    let secret = app.create_card(&a, other.id, "secret", "diary", None, None, None).await.unwrap();

    // Read only: may look (even at the next card), may not write or answer.
    let (_, ro) = app.create_token(a.user_id(), "reader", false, false, None).await.unwrap();
    let ro = ro.as_str();
    let o = ok(&app, ro, "get_overview", json!({})).await;
    assert_eq!((o["permissions"]["can_write"].clone(), o["permissions"]["can_review"].clone()), (json!(false), json!(false)));
    ok(&app, ro, "list_cards", json!({})).await;
    for (tool, args) in [
        ("create_deck", json!({ "name": "x" })),
        ("create_cards", json!({ "deck_id": mine.id, "cards": [{ "front": "a", "back": "b" }] })),
        ("update_card", json!({ "card_id": card.id, "back": "cat" })),
        ("delete_cards", json!({ "card_ids": [card.id] })),
        ("answer_card", json!({ "card_id": card.id, "rating": "good" })),
        ("import_csv", json!({ "deck_id": mine.id, "csv": "a,b" })),
        ("delete_deck", json!({ "deck_id": mine.id, "confirm": true })),
    ] {
        let r = call(&app, ro, tool, args).await;
        let r = match r {
            Ok(Value::Object(m)) if tool == "create_cards" => Err(m["errors"].to_string()).and_then(|e: String| if e == "[]" { Ok(()) } else { Err(e) }),
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        assert!(r.is_err(), "a read-only token could {tool}");
    }
    assert_eq!(app.card(&a, card.id).await.unwrap().back, "dog");

    // One deck only: the other deck and its cards don't exist for it.
    let (_, narrow) = app.create_token(a.user_id(), "narrow", true, true, Some(vec![mine.id])).await.unwrap();
    let narrow = narrow.as_str();
    let decks = ok(&app, narrow, "list_decks", json!({})).await;
    assert_eq!(decks.as_array().unwrap().len(), 1);
    assert!(call(&app, narrow, "get_card", json!({ "card_id": secret.id })).await.is_err());
    assert_eq!(ok(&app, narrow, "list_cards", json!({ "query": "secret" })).await["total"], 0);
    assert!(call(&app, narrow, "update_card", json!({ "card_id": card.id, "deck_id": other.id })).await.is_err(), "can't move cards out");
    assert!(call(&app, narrow, "create_deck", json!({ "name": "new" })).await.is_err());
    let next = ok(&app, narrow, "get_next_card", json!({})).await;
    assert_eq!(next["card"]["id"], card.id);

    // A revoked token stops working at once.
    let (tok, raw) = app.create_token(a.user_id(), "gone", true, true, None).await.unwrap();
    app.revoke_token(a.user_id(), tok.id).await.unwrap();
    let res = send(
        &app,
        Request::post("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {raw}"))
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_agent_can_pause_and_resume_a_deck() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let (_, t) = app.create_token(a.user_id(), "claude", true, true, None).await.unwrap();
    let t = t.as_str();
    let deck = ok(&app, t, "create_deck", json!({ "name": "Spanish" })).await;
    let deck_id = deck["id"].as_i64().unwrap();
    assert_eq!(deck["status"], "active");
    ok(&app, t, "create_cards", json!({ "deck_id": deck_id, "cards": [{ "front": "hablar", "back": "to speak" }] })).await;

    let paused = ok(&app, t, "update_deck", json!({ "deck_id": deck_id, "status": "paused" })).await;
    assert_eq!(paused["status"], "paused");
    let next = ok(&app, t, "get_next_card", json!({})).await;
    assert!(next.get("card").is_none_or(Value::is_null), "a paused deck is not studied: {next}");
    let listed = ok(&app, t, "list_decks", json!({})).await;
    assert_eq!(listed[0]["status"], "paused");

    let back = ok(&app, t, "update_deck", json!({ "deck_id": deck_id, "status": "active" })).await;
    assert_eq!(back["status"], "active");
    let next = ok(&app, t, "get_next_card", json!({})).await;
    assert_eq!(next["card"]["front"], "hablar");
    assert!(call(&app, t, "update_deck", json!({ "deck_id": deck_id, "status": "asleep" })).await.is_err());
}
