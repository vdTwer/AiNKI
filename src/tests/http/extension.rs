//! The browser extension is offered from the Agent access page once its
//! signed file is in the data folder.

use super::*;

async fn get(app: &Arc<App>, cookie: &str, path: &str) -> axum::response::Response {
    let req = Request::get(path)
        .header(header::HOST, "cards.example.com")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    send(app, req).await
}

#[tokio::test]
async fn the_extension_is_offered_only_when_its_file_is_there() {
    let app = app().await;
    user(&app, "kolja").await;
    let cookie = login_cookie(&app, "kolja").await;

    assert!(!text(get(&app, &cookie, "/access").await).await.contains("/extension.xpi"));
    assert_eq!(get(&app, &cookie, "/extension.xpi").await.status(), StatusCode::NOT_FOUND);

    std::fs::create_dir_all(&app.config.data_dir).unwrap();
    std::fs::write(app.config.data_dir.join("ainki.xpi"), b"PK signed").unwrap();
    assert!(text(get(&app, &cookie, "/access").await).await.contains("href=\"/extension.xpi\""));
    let res = get(&app, &cookie, "/extension.xpi").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/x-xpinstall");
    assert_eq!(text(res).await, "PK signed");

    // Only for people with an account.
    let anon = Request::get("/extension.xpi").header(header::HOST, "cards.example.com").body(Body::empty()).unwrap();
    assert_ne!(send(&app, anon).await.status(), StatusCode::OK);
}
