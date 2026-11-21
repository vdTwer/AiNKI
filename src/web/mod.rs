//! The website: HTML rendered on the server with maud. Every page works
//! without JavaScript. The simple look serves phones and e-ink readers; wide
//! screens with a mouse get the desktop look on top of it.
//!
//! This file holds the routes and the plumbing every page shares (layout,
//! the signed-in user, forms, redirects with a message). The pages live in
//! `pages/`, one file per area of the site.

use std::{collections::HashMap, time::Duration};

use axum::{
    Router,
    extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, Query, RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::{
    Form,
    cookie::{Cookie, CookieJar, SameSite},
};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::{Deserialize, Serialize};

use crate::{
    App, AppState,
    advice::{self, Advice},
    auth::{Access, User},
    coach,
    error::AppError,
    format::{self, count},
    fuzzy, import, insight, invites, lessons, markdown, mcp, media,
    multi::{self, Piece, Split},
    now, num, oauth,
    outlook::Outlook,
    presets,
    scheduler::{self, CardState, Rating},
    security::{self, ClientIp, Flash},
    service::{Card, CardFilter, CardInput, Deck, DeckStatus, DeckSummary, NewCard, Patch, PresetInput, Settings},
    study,
};

/// One module per area of the site. Each sees everything here and in its
/// siblings, so moving a function between pages changes no imports.
mod pages {
    pub mod admin;
    pub mod auth;
    pub mod cards;
    pub mod coach;
    pub mod decks;
    pub mod find;
    pub mod home;
    pub mod import;
    pub mod invites;
    pub mod lessons;
    pub mod media;
    pub mod presets;
    pub mod settings;
    pub mod sharing;
    pub mod stats;
    pub mod study;
    pub mod tokens;
}
#[allow(clippy::wildcard_imports, reason = "the router and every page reach every handler by name")]
use pages::{
    admin::*, auth::*, cards::*, coach::*, decks::*, find::*, home::*, import::*, invites::*, lessons::*, media::*,
    presets::*, settings::*, sharing::*, stats::*, study::*, tokens::*,
};
// The consent page for OAuth clients asks for permissions the same way.
pub(crate) use pages::tokens::permission_fields;

const CSS: &str = include_str!("../style.css");
const CALM_CSS: &str = include_str!("../static/calm.css");
const THEME_JS: &str = include_str!("../static/theme.js");
const APP_JS: &str = include_str!("../static/app.js");
const DESK_JS: &str = include_str!("../static/desk.js");
// Two cards, Wild Orchid behind Terra Cotta, with the "done" tick.
const FAVICON_SVG: &str = include_str!("../static/favicon.svg");
const FAVICON_PNG: &[u8] = include_bytes!("../static/favicon-32.png");
const TOUCH_ICON_PNG: &[u8] = include_bytes!("../static/apple-touch-icon.png");
const FONTS: &[(&str, &[u8])] = &[
    ("eb-garamond-italic-latin-ext.woff2", include_bytes!("../static/fonts/eb-garamond-italic-latin-ext.woff2")),
    ("eb-garamond-italic-latin.woff2", include_bytes!("../static/fonts/eb-garamond-italic-latin.woff2")),
    ("eb-garamond-normal-latin-ext.woff2", include_bytes!("../static/fonts/eb-garamond-normal-latin-ext.woff2")),
    ("eb-garamond-normal-latin.woff2", include_bytes!("../static/fonts/eb-garamond-normal-latin.woff2")),
];

pub async fn serve(app: AppState) -> anyhow::Result<()> {
    let bind = app.config.bind.clone();
    let router = router(app);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("AiNKI listening on http://{bind}");
    axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Ctrl-C locally, SIGTERM from `docker stop`.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

#[allow(clippy::too_many_lines, reason = "the route table reads best as one list")]
pub fn router(app: AppState) -> Router {
    let mcp_routes = Router::new()
        .nest_service("/mcp", mcp::service(app.clone()))
        // route_layer: only /mcp needs a token, not every unknown URL.
        .route_layer(middleware::from_fn_with_state(app.clone(), mcp::auth));
    // Endpoints MCP clients call directly. They use tokens, not cookies, so
    // cross-origin calls (browser-based clients) are safe to allow.
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
        .expose_headers([header::WWW_AUTHENTICATE, header::HeaderName::from_static("mcp-session-id")]);
    let client_routes = Router::new()
        .route("/.well-known/oauth-protected-resource", get(oauth::protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(oauth::protected_resource))
        .route("/.well-known/oauth-authorization-server", get(oauth::authorization_server))
        .route("/.well-known/openid-configuration", get(oauth::authorization_server))
        .route("/oauth/register", post(oauth::register))
        .route("/oauth/token", post(oauth::token))
        .route("/oauth/revoke", post(oauth::revoke))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .merge(mcp_routes)
        .layer(cors);
    // Form posts are small; uploads get their own limits below.
    let small = DefaultBodyLimit::max(256 * 1024);
    let card_upload = DefaultBodyLimit::max(2 * media::MAX_UPLOAD_BYTES + 256 * 1024);
    let browser_routes = Router::new()
        .route("/", get(home))
        .route("/login", get(login_page).post(login))
        .route("/signup", get(signup_page).post(signup))
        .route("/logout", post(logout))
        .route("/invites", get(invites_page).post(invite_create))
        .route("/invites/{id}/cancel", post(invite_cancel))
        .route("/invite/{token}", get(invite_page).post(invite_accept))
        .route("/admin", get(admin_page))
        .route("/oauth/authorize", get(oauth::authorize_page).post(oauth::authorize_submit))
        .route("/study", get(study))
        .route("/study/answer", post(study_answer))
        .route("/study/undo", post(study_undo))
        .route("/study/anyway", post(study_anyway))
        .route("/study/keep-going", post(study_keep_going))
        .route("/decks/new", get(deck_new_page).post(deck_create))
        .route("/decks/merge", get(deck_merge_page).post(deck_merge))
        .route("/decks/{id}", get(deck_page))
        .route("/decks/{id}/edit", get(deck_edit_page).post(deck_update))
        .route("/decks/{id}/delete", post(deck_delete))
        .route("/decks/{id}/export.csv", get(deck_export))
        .route("/decks/{id}/share", post(deck_share))
        .route("/decks/{id}/{action}", post(deck_action))
        .route("/decks/archived", get(archived_page))
        .route("/shares/{id}/{action}", post(share_action))
        .route("/cards/{id}/split", get(split_page).post(split_submit))
        .route("/cards/{id}/{action}", post(card_action))
        .route("/attention", get(attention_page))
        .route("/coach/not-now", post(coach_not_now))
        .route("/learn", get(learn_page))
        .route("/learn/{slug}", get(lesson_page))
        .route("/learn/{slug}/deck", post(lesson_deck))
        .route("/presets", get(presets_page))
        .route("/presets/new", get(preset_new_page).post(preset_create))
        .route("/presets/{id}", get(preset_page).post(preset_update))
        .route("/presets/{id}/delete", post(preset_delete))
        .route("/settings", get(settings_page).post(settings_update))
        .route("/settings/password", post(password_update))
        .route("/settings/optimize", post(optimize))
        .route("/settings/auto-optimize", post(auto_optimize))
        .route("/settings/study", post(study_settings_update))
        .route("/settings/catch-up", post(catch_up_update))
        .route("/settings/reset-params", post(reset_params))
        .route("/access", get(access_page))
        .route("/access/new", get(|| async { Redirect::to("/access") }).post(token_create))
        .route("/access/{id}/revoke", post(token_revoke))
        // The page used to be called Tokens.
        .route("/tokens", get(|| async { Redirect::permanent("/access") }))
        .route("/stats", get(stats_page))
        .route("/media/{id}", get(media_file))
        .layer(small)
        .route("/cards/preview", post(card_preview_json))
        .route("/find", get(find_json))
        .route("/cards/new", get(card_new_page).post(card_create).layer(card_upload))
        .route("/cards/{id}", get(card_page).post(card_update).layer(card_upload))
        .route(
            "/import",
            get(import_page).post(import_upload).layer(DefaultBodyLimit::max(import::MAX_IMPORT_BYTES + 1024 * 1024)),
        )
        .layer(middleware::from_fn_with_state(app.clone(), security::check_origin))
        .layer(middleware::from_fn_with_state(app.clone(), renew_session));
    let static_routes = Router::new()
        .route("/style.css", get(|q: RawQuery| async move { asset(q, "text/css; charset=utf-8", CSS) }))
        .route("/calm.css", get(|q: RawQuery| async move { asset(q, "text/css; charset=utf-8", CALM_CSS) }))
        .route("/theme.js", get(|q: RawQuery| async move { asset(q, "text/javascript; charset=utf-8", THEME_JS) }))
        .route("/app.js", get(|q: RawQuery| async move { asset(q, "text/javascript; charset=utf-8", APP_JS) }))
        .route("/desk.js", get(|q: RawQuery| async move { asset(q, "text/javascript; charset=utf-8", DESK_JS) }))
        .route("/fonts/{name}", get(font_file))
        .route("/favicon.svg", get(|| async { static_file("image/svg+xml", FAVICON_SVG.as_bytes(), false) }))
        // Browsers ask for /favicon.ico on their own; a PNG there works everywhere.
        .route("/favicon.ico", get(|| async { static_file("image/png", FAVICON_PNG, false) }))
        .route("/apple-touch-icon.png", get(|| async { static_file("image/png", TOUCH_ICON_PNG, false) }))
        .route("/healthz", get(|| async { "ok" }));
    Router::new()
        .merge(browser_routes)
        .merge(static_routes)
        .merge(client_routes)
        .layer(middleware::from_fn(security::flash))
        .layer(middleware::from_fn_with_state(app.clone(), security::headers))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(app)
}

// ----- plumbing -----------------------------------------------------------

/// A logged-in browser user. Redirects to the login page otherwise.
pub struct WebUser(pub User);

impl WebUser {
    fn access(&self) -> Access {
        Access::web(self.0.clone())
    }
}

async fn session_from_parts(parts: &Parts, app: &AppState) -> Option<User> {
    let jar = CookieJar::from_headers(&parts.headers);
    app.session_user(jar.get(app.config.session_cookie_name())?.value()).await.ok().flatten()
}

impl axum::extract::OptionalFromRequestParts<AppState> for WebUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Option<Self>, Self::Rejection> {
        Ok(session_from_parts(parts, app).await.map(WebUser))
    }
}

/// Where the admin account may go: its panel and making invites.
fn admin_may(path: &str) -> bool {
    path == "/admin" || path == "/invites" || path.starts_with("/invites/")
}

/// The login page, coming back here afterwards when this was a page view.
fn login_url(parts: &Parts) -> String {
    let here = parts.uri.path_and_query().map_or("/", http::uri::PathAndQuery::as_str);
    match safe_next(Some(here)) {
        Some(next) if parts.method == axum::http::Method::GET && next != "/" => {
            format!("/login?next={}", urlenc(next))
        }
        _ => "/login".into(),
    }
}

impl FromRequestParts<AppState> for WebUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        match session_from_parts(parts, app).await {
            // The admin account has no decks: it sees the admin panel only.
            Some(user) if user.is_admin && !admin_may(parts.uri.path()) => Err(redirect("/admin")),
            Some(user) => Ok(Self(user)),
            None => Err(Redirect::to(&login_url(parts)).into_response()),
        }
    }
}

pub struct WebError(pub AppError);

impl From<AppError> for WebError {
    fn from(e: AppError) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for WebError {
    fn from(e: sqlx::Error) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Unauthorized => return Redirect::to("/login").into_response(),
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Internal(e) => {
                tracing::error!("internal error: {e:?}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        let msg = match &self.0 {
            AppError::Internal(_) => "Something went wrong on the server.".to_string(),
            e => capitalize(&e.to_string()),
        };
        sorry(status, &msg)
    }
}

/// The page for anything that stops a request, so every refusal looks alike.
pub fn sorry(status: StatusCode, msg: &str) -> Response {
    let body = layout(
        "Sorry",
        None,
        html! {
            h1 { "Sorry" }
            p.notice { (msg) }
            p { a.btn href="/" { "Home" } }
        },
    );
    (status, body).into_response()
}

type WebResult<T = Response> = Result<T, WebError>;

/// The words for a problem the person can fix by changing what they sent.
/// Anything else (not found, server trouble) gets the error page.
fn fixable(e: &AppError) -> Option<String> {
    match e {
        AppError::BadRequest(m) | AppError::Conflict(m) | AppError::Forbidden(m) => Some(capitalize(m)),
        _ => None,
    }
}

/// The problem with a sent form, shown above it.
fn form_error(error: Option<&str>) -> Markup {
    html! { @if let Some(e) = error { p.notice role="alert" { (e) } } }
}

/// A form shown again with its problem: what was typed stays.
fn form_again(page: Markup) -> Response {
    (StatusCode::BAD_REQUEST, page).into_response()
}

/// For one-button actions: a problem the person can fix goes back to `to`
/// as a flash message; anything else is the error page.
fn or_back(to: impl AsRef<str>, result: Result<Response, AppError>) -> WebResult {
    match result {
        Ok(r) => Ok(r),
        Err(e) => match fixable(&e) {
            Some(msg) => Ok(redirect_msg(to, &msg)),
            None => Err(e.into()),
        },
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn redirect(to: impl AsRef<str>) -> Response {
    Redirect::to(to.as_ref()).into_response()
}

fn flash_cookie(msg: &str) -> Cookie<'static> {
    // The cookie jar percent-encodes the value.
    Cookie::build((security::FLASH_COOKIE, msg.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(time::Duration::seconds(60))
        .build()
}

/// Redirect and show `msg` on the next page.
fn redirect_msg(to: impl AsRef<str>, msg: &str) -> Response {
    (CookieJar::new().add(flash_cookie(msg)), Redirect::to(to.as_ref())).into_response()
}

pub fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Changes with every release that changes a stylesheet or script. Added to
/// their URLs so browsers never pair a new page with a cached old file.
fn asset_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let all = [CSS, CALM_CSS, THEME_JS, APP_JS, DESK_JS].concat();
        crate::auth::sha256_hex(&all)[..10].to_string()
    })
}

/// Where the Calm look may apply. theme.js reads it from the stylesheet link.
const DESKTOP_MEDIA: &str = "(min-width: 60em) and (hover: hover) and (pointer: fine)";

/// Nested Anki decks arrive as "`Languages::Spanish`". The parents are shown
/// as a quiet path, the deck's own name last: "Languages › Spanish".
fn deck_parts(name: &str) -> Vec<&str> {
    let parts: Vec<&str> = name.split("::").map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { vec![name] } else { parts }
}

fn deck_title(name: &str) -> Markup {
    let parts = deck_parts(name);
    let Some((leaf, parents)) = parts.split_last() else { return html! { (name) } };
    html! {
        @if !parents.is_empty() {
            span.deck-path { @for p in parents { (p) span.sep aria-hidden="true" { "›" } } }
        }
        span.deck-leaf { (leaf) }
    }
}

/// The same for places that only take text (titles, options, lists).
pub fn deck_plain(name: &str) -> String {
    deck_parts(name).join(" › ")
}

/// "on Friday, 18 December" for a stored exam date.
fn exam_words(user: &User, date: chrono::NaiveDate) -> String {
    match format::day(&user.clock(), date).as_str() {
        w @ ("today" | "tomorrow" | "yesterday") => w.to_string(),
        w => format!("on {w}"),
    }
}

fn forecast_day(user: &User, date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_or_else(|_| date.to_string(), |d| format::short_day(&user.clock(), d))
}

/// The menu entry a page belongs to; marked as the current one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Decks,
    Study,
    Add,
    Stats,
    Learn,
    Settings,
}

/// A page outside the menu (login, errors).
pub fn layout(title: &str, user: Option<&User>, body: Markup) -> Markup {
    page(None, title, user, None, body)
}

/// A page under one of the menu entries.
pub fn layout_in(nav: Nav, title: &str, user: Option<&User>, body: Markup) -> Markup {
    page(Some(nav), title, user, None, body)
}

/// `page_kind` marks special pages for app.js.
#[expect(clippy::needless_pass_by_value, reason = "maud renders by reference; callers build the markup inline")]
fn page(nav: Option<Nav>, title: &str, user: Option<&User>, page_kind: Option<&str>, body: Markup) -> Markup {
    let scale = num::i64_f64(user.map_or(100, |u| u.font_scale)) / 100.0;
    html! {
        (DOCTYPE)
        html lang="en" data-look=[user.map(|u| if u.desktop_look == "simple" { "simple" } else { "calm" })] {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · AiNKI" }
                link rel="icon" href="/favicon.ico" sizes="32x32";
                link rel="icon" href="/favicon.svg" type="image/svg+xml";
                link rel="apple-touch-icon" href="/apple-touch-icon.png";
                link rel="stylesheet" href=(format!("/style.css?v={}", asset_version()));
                // The Calm look: only on desktops, and only once theme.js
                // has switched it on. Everything else gets style.css alone.
                link rel="stylesheet" href=(format!("/calm.css?v={}", asset_version())) media=(DESKTOP_MEDIA) data-calm;
                script src=(format!("/theme.js?v={}", asset_version())) {}
                @if (scale - 1.0).abs() > f64::EPSILON {
                    style { (PreEscaped(format!(":root{{--scale:{scale}}}"))) }
                }
            }
            body data-user=[user.map(|u| u.id)] data-page=[page_kind] {
                @if user.is_some_and(|u| !u.is_admin) { (main_nav(nav)) }
                @if let Some(u) = user { (sleep_wall(u)) }
                main {
                    // The desktop look: a ? in the corner that opens the key sheet.
                    @if user.is_some_and(|u| !u.is_admin) {
                        button.keys-button.calm-only type="button" aria-label="Keys" data-label="Keys" { "?" }
                    }
                    (body)
                }
                script src=(format!("/app.js?v={}", asset_version())) defer {}
                script src=(format!("/desk.js?v={}", asset_version())) defer {}
            }
        }
    }
}

/// On phones a bar across the top; in the Calm look one dot in the top
/// right corner that opens into a row of icons on hover.
fn main_nav(nav: Option<Nav>) -> Markup {
    let current = |n: Nav| (nav == Some(n)).then_some("page");
    html! {
        nav.top aria-label="Main" {
            // The desktop look: the first icon finds anything (desk.js).
            button.find-dot.calm-only type="button" data-label="Find" aria-label="Find" aria-haspopup="dialog" { (nav_icon(ICON_FIND)) }
            a.brand href="/" data-label="AiNKI" aria-current=[current(Nav::Decks)] { "AiNKI" }
            a href="/study" data-label="Study" aria-current=[current(Nav::Study)] { (nav_icon(ICON_STUDY)) "Study" }
            a href="/cards/new" data-label="Add" aria-current=[current(Nav::Add)] { (nav_icon(ICON_ADD)) "Add" }
            a href="/stats" data-label="Stats" aria-current=[current(Nav::Stats)] { (nav_icon(ICON_STATS)) "Stats" }
            // Phones reach the lessons through settings; the bar stays as it was.
            a.rail-only href="/learn" data-label="Learn" aria-current=[current(Nav::Learn)] { (nav_icon(ICON_LEARN)) "Learn" }
            a href="/settings" data-label="Settings" aria-current=[current(Nav::Settings)] { (nav_icon(ICON_SETTINGS)) "Settings" }
            // Last, so it opens right under the dot: hovering the dot offers Home.
            a.rail-only href="/" data-label="Home" aria-current=[current(Nav::Decks)] { (nav_icon(ICON_DECKS)) "Home" }
        }
    }
}

/// The desktop look in quiet hours: one dark screen over everything. The
/// only way past is a faint "study anyway" at the bottom that asks twice.
/// Two folds, so it works without JavaScript.
fn sleep_wall(u: &User) -> Markup {
    html! {
        @if let Some(until) = study::quiet_until(u, now()) {
            div.sleep-wall.calm-only role="dialog" aria-modal="true" aria-labelledby="sleep-title" {
                h1 #sleep-title { "Go to sleep." }
                p { "Your cards open again at " (clock_time(&u.clock(), until)) "." }
                details.sleep-anyway {
                    summary { "study anyway" }
                    p { "Sleep is when today's cards settle. Studying now costs more than it brings." }
                    details {
                        summary { "I know. Study anyway." }
                        form method="post" action="/study/anyway" {
                            button type="submit" { "Yes, open my cards until " (clock_time(&u.clock(), until)) }
                        }
                    }
                }
            }
        }
    }
}

// Icons: line drawings on a 24px grid in the text colour. The rail shows
// them at 26px; in a page, next to a word, at 20px.
const ICON_DECKS: &str = r#"<rect x="4" y="9.5" width="16" height="10.5" rx="2"/><path d="M6.5 6.5h11M9 3.5h6"/>"#;
const ICON_STUDY: &str =
    r#"<rect x="5" y="3.5" width="14" height="17" rx="2.5"/><path d="M9 10.5l2 2 4-4"/><path d="M9 16.5h6"/>"#;
const ICON_FIND: &str = r#"<circle cx="10.5" cy="10.5" r="5.5"/><path d="M14.5 14.5l5 5"/>"#;
const ICON_ADD: &str = r#"<path d="M12 5v14M5 12h14"/>"#;
const ICON_STATS: &str = r#"<path d="M5 20V13M12 20V5M19 20v-9"/>"#;
const ICON_LEARN: &str = r#"<path d="M12 6.5C10 5 7.5 4.5 4.5 4.5v13.5c3 0 5.5.5 7.5 2 2-1.5 4.5-2 7.5-2V4.5c-3 0-5.5.5-7.5 2z"/><path d="M12 6.5v13.5"/>"#;
const ICON_SETTINGS: &str =
    r#"<path d="M5 7h8M17 7h2M5 17h2M11 17h8"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>"#;
const ICON_EDIT: &str = r#"<path d="M14.5 5.5l4 4L9 19H5v-4z"/><path d="M12.5 7.5l4 4"/>"#;
// A paper plane, so Share doesn't look like Export's tray.
const ICON_SHARE: &str = r#"<path d="M20 4L3.5 10.5l7 3 3 7z"/><path d="M20 4l-9.5 9.5"/>"#;
const ICON_IMPORT: &str =
    r#"<path d="M12 4v11M8 11l4 4 4-4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
const ICON_EXPORT: &str =
    r#"<path d="M12 15V4M8 8l4-4 4 4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
const ICON_PAUSE: &str = r#"<path d="M9 6.5v11M15 6.5v11"/>"#;
const ICON_UNDO: &str = r#"<path d="M9 7L5 11l4 4"/><path d="M5 11h9a5 5 0 0 1 0 10h-2"/>"#;
const ICON_IMAGE: &str = r#"<rect x="3.5" y="5" width="17" height="14" rx="2.5"/><circle cx="9" cy="10" r="1.6"/><path d="M4.5 17l4.5-4.2 3.2 3 2.6-2.2 4.7 4"/>"#;
const ICON_FILE: &str = r#"<path d="M7 3.5h6.5L18 8v11a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 19V5a1.5 1.5 0 0 1 1-1.5z"/><path d="M13 3.5V8h5"/>"#;

const RAIL_ICON: u32 = 30;
const PAGE_ICON: u32 = 20;
const DROP_ICON: u32 = 34;

/// An icon as a small solid object: the drawing repeated a little further
/// back each time gives it depth, plus a soft shadow behind. calm.css
/// spaces the layers out and tilts it with its link; elsewhere it is
/// hidden and only the words show. Large icons draw with 2px lines; small
/// ones next to words with finer 1.5px lines, so they don't look heavy.
fn icon(paths: &'static str, size: u32) -> Markup {
    let line = if size < 24 { 1.5 } else { 2.0 };
    let stroke = format!("{:.2}", line * 24.0 / f64::from(size));
    let layer = |class: &'static str, depth: u8| {
        html! {
            svg class=(class) style=(format!("--z:{depth}")) viewBox="0 0 24 24" width=(size) height=(size) fill="none" stroke="currentColor" stroke-width=(stroke) stroke-linecap="round" stroke-linejoin="round" {
                (PreEscaped(paths))
            }
        }
    };
    html! {
        span.tile style=(format!("--size:{size}px")) aria-hidden="true" {
            (layer("shadow", 0))
            @for z in (1..=5).rev() { (layer("side", z)) }
            (layer("face", 0))
        }
    }
}

fn nav_icon(paths: &'static str) -> Markup {
    icon(paths, RAIL_ICON)
}

/// A quiet link with a 3D icon in front of its words.
fn icon_link(href: &str, paths: &'static str, label: &str) -> Markup {
    html! { a.icon-link href=(href) { (icon(paths, PAGE_ICON)) span { (label) } } }
}

/// The "done" tick: Terra, drawn with straight strokes and square ends, like
/// the numbers beside it. pathLength lets calm.css draw it in from 1 to 0
/// at any size.
fn done_check(size: u32, class: &'static str) -> Markup {
    html! {
        svg class=(class) viewBox="0 0 24 24" width=(size) height=(size) fill="none" stroke="currentColor" stroke-width=(format!("{:.2}", 52.0 / f64::from(size))) stroke-linecap="square" stroke-linejoin="miter" aria-hidden="true" {
            path d="M4.5 12.5 9.5 17.5 19.5 6.5" pathLength="1" {}
        }
    }
}

/// The same where the simple look shows a button.
fn icon_btn(href: &str, paths: &'static str, label: &str) -> Markup {
    html! { a.btn.icon-link href=(href) { (icon(paths, PAGE_ICON)) span { (label) } } }
}

/// The page this one belongs to, as one quiet line above the heading:
/// "‹ Spanish".
fn parent(href: &str, label: &str) -> Markup {
    html! { p.parent { a href=(href) { span aria-hidden="true" { "‹ " } (label) } } }
}

fn flash(msg: Option<&str>) -> Markup {
    html! {
        @if let Some(m) = msg.filter(|m| !m.is_empty()) {
            p.notice role="status" { (m) }
        }
    }
}

fn face(text: &str, image: Option<&str>) -> Markup {
    html! {
        @if let Some(img) = image {
            img src=(format!("/media/{img}")) alt="";
        }
        @if !text.is_empty() {
            div.md { (PreEscaped(markdown::render(text))) }
        }
    }
}

fn session_cookie(app: &AppState, token: String) -> Cookie<'static> {
    Cookie::build((app.config.session_cookie_name(), token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(app.config.secure())
        .max_age(time_days(crate::auth::SESSION_DAYS))
        .build()
}

/// Someone who keeps using `AiNKI` stays logged in: once a day their session
/// and its cookie get the full lifetime again.
async fn renew_session(
    State(app): State<AppState>,
    jar: CookieJar,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let mut res = next.run(req).await;
    if let Some(c) = jar.get(app.config.session_cookie_name())
        && app.renew_session(c.value()).await.unwrap_or(false)
        && let Ok(v) = HeaderValue::from_str(&session_cookie(&app, c.value().to_string()).to_string())
    {
        res.headers_mut().append(header::SET_COOKIE, v);
    }
    res
}

const fn time_days(days: i64) -> time::Duration {
    time::Duration::days(days)
}

/// Small static assets. They change only with a new release, so an hour of
/// caching is plenty.
/// Every built-in file goes out through here. Fonts never change under a
/// name and a stylesheet or script asked for with the current ?v= never
/// changes either, so both are cached for good; anything else revalidates.
fn static_file(content_type: &'static str, body: &'static [u8], forever: bool) -> Response {
    let cache = if forever { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, cache)], body).into_response()
}

fn asset(RawQuery(q): RawQuery, content_type: &'static str, body: &'static str) -> Response {
    let current = q.and_then(|q| q.strip_prefix("v=").map(|v| v == asset_version())).unwrap_or(false);
    static_file(content_type, body.as_bytes(), current)
}

async fn font_file(Path(name): Path<String>) -> Response {
    match FONTS.iter().find(|(n, _)| *n == name) {
        Some((_, bytes)) => static_file("font/woff2", bytes, true),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
