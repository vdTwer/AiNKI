//! Passwords, sessions and server settings.

use super::*;

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path).header(header::COOKIE, cookie).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn changing_the_password_logs_out_other_browsers() {
    let app = app().await;
    user(&app, "kolja").await;
    let laptop = login_cookie(&app, "kolja").await;
    let phone = login_cookie(&app, "kolja").await;

    // A wrong current password changes nothing and says so.
    let res = submit(&app, &laptop, "/settings/password", "current=wrong-one&new=a-new-password").await;
    assert_eq!(location(&res), "/settings#password");
    assert_eq!(flash(&res), "Current password is wrong");
    // Too short a new one neither.
    let res = submit(&app, &laptop, "/settings/password", "current=password123&new=short").await;
    assert_eq!(location(&res), "/settings#password");
    assert!(app.authenticate("kolja", "password123").await.is_ok());

    let res = submit(&app, &laptop, "/settings/password", "current=password123&new=a-new-password").await;
    assert_eq!(location(&res), "/settings");
    assert_eq!(flash(&res), "Password changed.");
    // This browser gets a fresh session; the phone is logged out.
    let fresh = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_string())
        .find(|c| c.starts_with(app.config.session_cookie_name()))
        .expect("a new session cookie");
    assert_eq!(send(&app, get("/", &fresh)).await.status(), StatusCode::OK);
    for old in [&laptop, &phone] {
        let res = send(&app, get("/", old)).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert!(location(&res).starts_with("/login"));
    }
    assert!(app.authenticate("kolja", "password123").await.is_err());
    assert!(app.authenticate("kolja", "a-new-password").await.is_ok());
}

#[tokio::test]
async fn logging_out_ends_only_this_session() {
    let app = app().await;
    user(&app, "kolja").await;
    let here = login_cookie(&app, "kolja").await;
    let there = login_cookie(&app, "kolja").await;
    let res = submit(&app, &here, "/logout", "").await;
    assert_eq!(location(&res), "/login");
    let cleared = res.headers().get_all(header::SET_COOKIE).iter().any(|c| {
        let c = c.to_str().unwrap();
        c.starts_with(app.config.session_cookie_name()) && c.contains("Max-Age=0")
    });
    assert!(cleared, "the cookie is removed");
    assert_eq!(send(&app, get("/", &here)).await.status(), StatusCode::SEE_OTHER);
    assert_eq!(send(&app, get("/", &there)).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn deleting_a_deck_takes_its_cards_and_goes_home() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let deck = app.create_deck(&a, "Old", "", None).await.unwrap();
    for i in 0..3 {
        app.create_card(&a, deck.id, &format!("q{i}"), "a", None, None, None).await.unwrap();
    }
    let cookie = login_cookie(&app, "kolja").await;
    let res = submit(&app, &cookie, &format!("/decks/{}/delete", deck.id), "").await;
    assert_eq!(location(&res), "/");
    assert_eq!(flash(&res), "Deck deleted, with its 3 cards.");
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cards").fetch_one(&app.db).await.unwrap();
    assert_eq!(left, 0);
    // Somebody else's deck can't be deleted.
    app.create_user("anna", "password123", "UTC").await.unwrap();
    let theirs = {
        let anna = Access::web(app.find_user("anna").await.unwrap());
        app.create_deck(&anna, "Anna's", "", None).await.unwrap()
    };
    let res = submit(&app, &cookie, &format!("/decks/{}/delete", theirs.id), "").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert!(app.deck(&Access::web(app.find_user("anna").await.unwrap()), theirs.id).await.is_ok());
}

#[tokio::test]
async fn cleanup_drops_expired_sessions_and_leftover_files() {
    let app = app().await;
    let a = user(&app, "kolja").await;
    let live = app.create_session(a.user_id()).await.unwrap();
    let dead = app.create_session(a.user_id()).await.unwrap();
    sqlx::query("UPDATE sessions SET expires_at = 1 WHERE id = ?")
        .bind(crate::auth::sha256_hex(&dead))
        .execute(&app.db)
        .await
        .unwrap();
    let tmp = app.config.data_dir.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let fresh = tmp.join("import-fresh.sqlite");
    std::fs::write(&fresh, b"x").unwrap();
    app.cleanup().await.unwrap();
    assert!(app.session_user(&live).await.unwrap().is_some());
    assert!(app.session_user(&dead).await.unwrap().is_none());
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions").fetch_one(&app.db).await.unwrap();
    assert_eq!(sessions, 1);
    assert!(fresh.exists(), "files younger than an hour stay");
}

#[test]
fn the_public_url_is_just_scheme_and_host() {
    assert_eq!(crate::parse_public_url(" https://Cards.Example.com/ ").unwrap(), "https://cards.example.com");
    assert_eq!(crate::parse_public_url("http://localhost:8080").unwrap(), "http://localhost:8080");
    for bad in ["cards.example.com", "ftp://x.org", "https://", "https://x.org/path", "https://u@x.org", "https://x.org?q"] {
        assert!(crate::parse_public_url(bad).is_err(), "{bad}");
    }
}

#[tokio::test]
async fn https_turns_on_secure_cookies() {
    let plain = app().await;
    assert!(!plain.config.secure());
    assert_eq!(plain.config.session_cookie_name(), "ainki_session");
    let https = app_with(|c| c.public_url = Some("https://cards.example.com".into())).await;
    assert!(https.config.secure());
    assert_eq!(https.config.session_cookie_name(), "__Host-ainki_session");
    let flag = app_with(|c| c.secure_cookies = true).await;
    assert!(flag.config.secure());

    // Over HTTPS the session cookie is Secure and HSTS is sent.
    user(&https, "kolja").await;
    let res = send(
        &https,
        form("/login", "")
            .header(header::ORIGIN, "https://cards.example.com")
            .body(Body::from("username=kolja&password=password123"))
            .unwrap(),
    )
    .await;
    let cookie = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|c| c.to_str().unwrap().to_string())
        .find(|c| c.starts_with("__Host-ainki_session="))
        .expect("session cookie");
    assert!(cookie.contains("Secure") && cookie.contains("HttpOnly") && cookie.contains("Path=/"), "{cookie}");
    assert!(res.headers().contains_key(header::STRICT_TRANSPORT_SECURITY));
}
