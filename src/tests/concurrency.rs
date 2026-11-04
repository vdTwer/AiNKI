//! Many people writing at once, on the real on-disk database with its pool
//! of connections. The in-memory test database has one connection, so it
//! can't show what happens when two writers meet.

use super::*;
use crate::multi::Split;

const BACK: &str = "- one\n- two\n- three";

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn writers_at_the_same_time_never_fail() {
    let dir = std::env::temp_dir().join(format!("ainki-busy-{}", uuid::Uuid::new_v4().simple()));
    let config = Config {
        data_dir: dir.clone(),
        bind: "127.0.0.1:0".into(),
        allow_signup: true,
        secure_cookies: false,
        default_timezone: "UTC".into(),
        public_url: None,
    };
    let db = crate::open_db(&config).await.unwrap();
    let app = Arc::new(App::new(db, config));

    let mut people = Vec::new();
    for i in 0..8 {
        let a = user(&app, &format!("writer{i}")).await;
        let deck = app.create_deck(&a, "Busy", "", None).await.unwrap();
        people.push((a, deck.id));
    }
    let mut tasks = Vec::new();
    for (a, deck) in people {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            let mut failures = Vec::new();
            for n in 0..25 {
                let multi = (n % 2 == 0).then_some(Split::List);
                if let Err(e) = app.create_card(&a, deck, &format!("q{n}"), BACK, None, None, multi).await {
                    failures.push(format!("{e:?}"));
                }
            }
            failures
        }));
    }
    let mut failures = Vec::new();
    for t in tasks {
        failures.extend(t.await.unwrap());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{} of 200 cards failed, first: {}", failures.len(), failures[0]);
}
