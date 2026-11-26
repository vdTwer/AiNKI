//! The Agent access page offers the Firefox extension from its latest
//! GitHub release.

use super::*;

#[tokio::test]
async fn agent_access_links_the_latest_extension_release() {
    let app = app().await;
    user(&app, "kolja").await;
    let cookie = login_cookie(&app, "kolja").await;
    let req = Request::get("/access")
        .header(header::HOST, "cards.example.com")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let page = text(send(&app, req).await).await;
    let link = "href=\"https://github.com/vdTwer/AiNKI-Extension/releases/latest/download/ainki.xpi\"";
    assert!(page.contains(link), "{page}");
}
