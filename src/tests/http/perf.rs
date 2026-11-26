//! Speed budgets for a heavy user: 20,000 cards in five decks and 150,000
//! answers spread over a year, the size of a few years of daily study.
//!
//! Run with `cargo test --release perf -- --ignored --nocapture`. CI runs it
//! in release mode; debug builds are ten times slower and not measured.

use super::*;

const CARDS: i64 = 20_000;
const REVIEWS: i64 = 150_000;
const DECKS: usize = 5;

/// Each page and the slowest median it may take, in milliseconds: about
/// twice what a cloud machine measured on 2026-11-21 (home 213, stats 223,
/// quick find 38, attention 38). Before that day's fixes, home took 900 ms
/// and stats 970, so a page that starts to scale with the answers again
/// fails here, while a slower runner does not.
const BUDGETS: [(&str, u128); 8] = [
    ("/", 300),
    ("/stats", 300),
    ("/study", 100),
    ("/decks/DECK", 150),
    ("/find?q=front+number+1999", 100),
    ("/attention", 100),
    ("/settings", 100),
    ("/presets", 100),
];

async fn seed(app: &App, a: &Access) -> i64 {
    let mut decks = Vec::with_capacity(DECKS);
    for n in 0..DECKS {
        decks.push(
            app.create_deck(a, &format!("Big::Deck {n}"), "", None)
                .await
                .unwrap()
                .id,
        );
    }
    let decks_json = serde_json::to_string(&decks).unwrap();
    // Every seventh card has lapsed, a few are due now, most are due later.
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?)
         INSERT INTO cards (user_id, deck_id, front, back, state, stability, difficulty, due, last_review,
             reps, lapses, position, created_at, updated_at)
         SELECT ?, json_extract(?, '$[' || (i % ?) || ']'), 'front number ' || i, 'back', 2, 20.0, 5.0,
             ? + (i % 30 - 3) * 86400, ? - 86400, 5, CASE WHEN i % 7 = 0 THEN 2 + i % 5 ELSE 0 END, i, 0, 0
         FROM n",
    )
    .bind(CARDS)
    .bind(a.user_id())
    .bind(&decks_json)
    .bind(i64::try_from(DECKS).unwrap())
    .bind(now())
    .bind(now())
    .execute(&app.db)
    .await
    .unwrap();
    // Answers spread evenly over the last year, one in ten forgotten.
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?)
         INSERT INTO review_log (user_id, card_id, deck_id, rating, reviewed_at, state_before, elapsed_days,
             scheduled_secs, duration_ms, source)
         SELECT ?, c.id, c.deck_id, CASE WHEN i % 10 = 0 THEN 1 ELSE 3 END, ? - i * (365 * 86400 / ?), 2, 5,
             432000, 6000, 'web'
         FROM n JOIN cards c ON c.id = (SELECT min(id) FROM cards WHERE user_id = ?) + i % ?",
    )
    .bind(REVIEWS)
    .bind(a.user_id())
    .bind(now() - 60)
    .bind(REVIEWS)
    .bind(a.user_id())
    .bind(CARDS)
    .execute(&app.db)
    .await
    .unwrap();
    // The server gathers planner statistics at startup; so does this test.
    sqlx::query("PRAGMA optimize=0x10002").execute(&app.db).await.unwrap();
    decks.first().copied().unwrap()
}

#[tokio::test]
#[ignore = "slow: seeds 150,000 answers; run in release mode"]
async fn pages_stay_fast_for_a_heavy_user() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = seed(&app, &a).await;
    let cookie = login_cookie(&app, "kolja").await;
    let mut over = Vec::new();
    for (page, budget) in BUDGETS {
        let path = page.replace("DECK", &deck.to_string());
        let mut times: Vec<u128> = Vec::new();
        for _ in 0..5 {
            let req = Request::get(&path)
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap();
            let t = std::time::Instant::now();
            let res = send(&app, req).await;
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
            times.push(t.elapsed().as_millis());
        }
        times.sort_unstable();
        let median = times[times.len() / 2];
        println!("{path:<32} {median:>5} ms (budget {budget} ms)");
        if median > budget {
            over.push(format!("{path} took {median} ms, budget {budget} ms"));
        }
    }
    if cfg!(debug_assertions) {
        println!("debug build: budgets not checked");
        return;
    }
    assert!(over.is_empty(), "over budget:\n{}", over.join("\n"));
}
