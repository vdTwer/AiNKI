//! The website: server-rendered HTML, no JavaScript required, high contrast,
//! no animations, big tap targets. Designed for phones and e-ink readers.

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
use serde::Deserialize;

use crate::{
    App, AppState,
    auth::{Access, User},
    coach,
    error::AppError,
    import, insight, lessons, markdown, mcp, media, multi::{self, Piece, Split}, now, oauth, presets,
    security::{self, ClientIp, Flash},
    scheduler::{self, CardState, Rating},
    study,
    service::{Card, CardFilter, CardInput, Deck, PresetInput},
    format::{self, count},
};

const CSS: &str = include_str!("style.css");
const CALM_CSS: &str = include_str!("static/calm.css");
const THEME_JS: &str = include_str!("static/theme.js");
const APP_JS: &str = include_str!("static/app.js");
const DESK_JS: &str = include_str!("static/desk.js");
// Two cards, Wild Orchid behind Terra Cotta, with the "done" tick.
const FAVICON_SVG: &str = include_str!("static/favicon.svg");
const FAVICON_PNG: &[u8] = include_bytes!("static/favicon-32.png");
const TOUCH_ICON_PNG: &[u8] = include_bytes!("static/apple-touch-icon.png");
const FONTS: &[(&str, &[u8])] = &[
    ("fraunces-italic-latin-ext.woff2", include_bytes!("static/fonts/fraunces-italic-latin-ext.woff2")),
    ("fraunces-italic-latin.woff2", include_bytes!("static/fonts/fraunces-italic-latin.woff2")),
    ("schibsted-grotesk-normal-latin-ext.woff2", include_bytes!("static/fonts/schibsted-grotesk-normal-latin-ext.woff2")),
    ("schibsted-grotesk-normal-latin.woff2", include_bytes!("static/fonts/schibsted-grotesk-normal-latin.woff2")),
    ("source-serif-4-italic-latin-ext.woff2", include_bytes!("static/fonts/source-serif-4-italic-latin-ext.woff2")),
    ("source-serif-4-italic-latin.woff2", include_bytes!("static/fonts/source-serif-4-italic-latin.woff2")),
    ("source-serif-4-normal-latin-ext.woff2", include_bytes!("static/fonts/source-serif-4-normal-latin-ext.woff2")),
    ("source-serif-4-normal-latin.woff2", include_bytes!("static/fonts/source-serif-4-normal-latin.woff2")),
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
        _ = ctrl_c => {}
        _ = term => {}
    }
}

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
        .route("/oauth/authorize", get(oauth::authorize_page).post(oauth::authorize_submit))
        .route("/study", get(study))
        .route("/study/answer", post(study_answer))
        .route("/study/undo", post(study_undo))
        .route("/study/anyway", post(study_anyway))
        .route("/study/keep-going", post(study_keep_going))
        .route("/decks/new", get(deck_new_page).post(deck_create))
        .route("/decks/{id}", get(deck_page))
        .route("/decks/{id}/edit", get(deck_edit_page).post(deck_update))
        .route("/decks/{id}/delete", post(deck_delete))
        .route("/decks/{id}/export.csv", get(deck_export))
        .route("/decks/{id}/share", post(deck_share))
        .route("/shares/{id}/{action}", post(share_action))
        .route("/cards/{id}/split", get(split_page).post(split_submit))
        .route("/cards/{id}/{action}", post(card_action))
        .route("/attention", get(attention_page))
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
        .route("/cards/new", get(card_new_page).post(card_create).layer(card_upload))
        .route("/cards/{id}", get(card_page).post(card_update).layer(card_upload))
        .route(
            "/import",
            get(import_page)
                .post(import_upload)
                .layer(DefaultBodyLimit::max(import::MAX_IMPORT_BYTES + 1024 * 1024)),
        )
        .layer(middleware::from_fn_with_state(app.clone(), security::check_origin))
        .layer(middleware::from_fn_with_state(app.clone(), renew_session));
    let static_routes = Router::new()
        .route("/style.css", get(|q: RawQuery| asset(q, "text/css; charset=utf-8", CSS)))
        .route("/calm.css", get(|q: RawQuery| asset(q, "text/css; charset=utf-8", CALM_CSS)))
        .route("/theme.js", get(|q: RawQuery| asset(q, "text/javascript; charset=utf-8", THEME_JS)))
        .route("/app.js", get(|q: RawQuery| asset(q, "text/javascript; charset=utf-8", APP_JS)))
        .route("/desk.js", get(|q: RawQuery| asset(q, "text/javascript; charset=utf-8", DESK_JS)))
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

/// The login page, coming back here afterwards when this was a page view.
fn login_url(parts: &Parts) -> String {
    let here = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
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
            Some(user) => Ok(WebUser(user)),
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

/// Nested Anki decks arrive as "Languages::Spanish". The parents are shown
/// as a quiet path, the deck's own name last: "Languages › Spanish".
fn deck_parts(name: &str) -> Vec<&str> {
    let parts: Vec<&str> = name.split("::").map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { vec![name] } else { parts }
}

fn deck_title(name: &str) -> Markup {
    let parts = deck_parts(name);
    let (leaf, parents) = parts.split_last().expect("at least one part");
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
fn exam_words(user: &User, date: &str) -> String {
    match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(d) => match format::day(&user.clock(), d).as_str() {
            w @ ("today" | "tomorrow" | "yesterday") => w.to_string(),
            w => format!("on {w}"),
        },
        Err(_) => date.to_string(),
    }
}

fn forecast_day(user: &User, date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| format::short_day(&user.clock(), d))
        .unwrap_or_else(|_| date.to_string())
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
fn page(nav: Option<Nav>, title: &str, user: Option<&User>, page_kind: Option<&str>, body: Markup) -> Markup {
    let current = |n: Nav| (nav == Some(n)).then_some("page");
    let scale = user.map(|u| u.font_scale).unwrap_or(100) as f64 / 100.0;
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
                // On phones a bar across the top; in the Calm look a rail of
                // dots on the left that become icons as the pointer nears.
                @if user.is_some() {
                    nav.top aria-label="Main" {
                        a.brand href="/" data-label="AiNKI" aria-current=[current(Nav::Decks)] { "AiNKI" }
                        a.rail-only href="/" data-label="Decks" aria-current=[current(Nav::Decks)] { (nav_icon(ICON_DECKS)) "Decks" }
                        a href="/study" data-label="Study" aria-current=[current(Nav::Study)] { (nav_icon(ICON_STUDY)) "Study" }
                        a href="/cards/new" data-label="Add" aria-current=[current(Nav::Add)] { (nav_icon(ICON_ADD)) "Add" }
                        a href="/stats" data-label="Stats" aria-current=[current(Nav::Stats)] { (nav_icon(ICON_STATS)) "Stats" }
                        // Phones reach the lessons through settings; the bar stays as it was.
                        a.rail-only href="/learn" data-label="Learn" aria-current=[current(Nav::Learn)] { (nav_icon(ICON_LEARN)) "Learn" }
                        a href="/settings" data-label="Settings" aria-current=[current(Nav::Settings)] { (nav_icon(ICON_SETTINGS)) "Settings" }
                    }
                }
                main {
                    // The desktop look's top line: the name, and where the keys are.
                    @if user.is_some() {
                        header.masthead.calm-only {
                            a.brand href="/" { "AiNKI" }
                            p.keys-hint { "Press " kbd { "?" } " for keys" }
                        }
                    }
                    (body)
                }
                script src=(format!("/app.js?v={}", asset_version())) defer {}
                script src=(format!("/desk.js?v={}", asset_version())) defer {}
            }
        }
    }
}

// Icons: line drawings on a 24px grid in the text colour. The rail shows
// them at 26px; in a page, next to a word, at 20px.
const ICON_DECKS: &str = r#"<rect x="4" y="9.5" width="16" height="10.5" rx="2"/><path d="M6.5 6.5h11M9 3.5h6"/>"#;
const ICON_STUDY: &str = r#"<rect x="5" y="3.5" width="14" height="17" rx="2.5"/><path d="M9 10.5l2 2 4-4"/><path d="M9 16.5h6"/>"#;
const ICON_ADD: &str = r#"<path d="M12 5v14M5 12h14"/>"#;
const ICON_STATS: &str = r#"<path d="M5 20V13M12 20V5M19 20v-9"/>"#;
const ICON_LEARN: &str = r#"<path d="M12 6.5C10 5 7.5 4.5 4.5 4.5v13.5c3 0 5.5.5 7.5 2 2-1.5 4.5-2 7.5-2V4.5c-3 0-5.5.5-7.5 2z"/><path d="M12 6.5v13.5"/>"#;
const ICON_SETTINGS: &str = r#"<path d="M5 7h8M17 7h2M5 17h2M11 17h8"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>"#;
const ICON_EDIT: &str = r#"<path d="M14.5 5.5l4 4L9 19H5v-4z"/><path d="M12.5 7.5l4 4"/>"#;
// A paper plane, so Share doesn't look like Export's tray.
const ICON_SHARE: &str = r#"<path d="M20 4L3.5 10.5l7 3 3 7z"/><path d="M20 4l-9.5 9.5"/>"#;
const ICON_IMPORT: &str = r#"<path d="M12 4v11M8 11l4 4 4-4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
const ICON_EXPORT: &str = r#"<path d="M12 15V4M8 8l4-4 4 4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
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
    let stroke = format!("{:.2}", line * 24.0 / size as f64);
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
        svg class=(class) viewBox="0 0 24 24" width=(size) height=(size) fill="none" stroke="currentColor" stroke-width=(format!("{:.2}", 52.0 / size as f64)) stroke-linecap="square" stroke-linejoin="miter" aria-hidden="true" {
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

fn flash(msg: &Option<String>) -> Markup {
    html! {
        @if let Some(m) = msg.as_deref().filter(|m| !m.is_empty()) {
            p.notice role="status" { (m) }
        }
    }
}

fn face(text: &str, image: &Option<String>) -> Markup {
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

/// Someone who keeps using AiNKI stays logged in: once a day their session
/// and its cookie get the full lifetime again.
async fn renew_session(State(app): State<AppState>, jar: CookieJar, req: axum::extract::Request, next: middleware::Next) -> Response {
    let mut res = next.run(req).await;
    if let Some(c) = jar.get(app.config.session_cookie_name()) {
        if app.renew_session(c.value()).await.unwrap_or(false) {
            if let Ok(v) = HeaderValue::from_str(&session_cookie(&app, c.value().to_string()).to_string()) {
                res.headers_mut().append(header::SET_COOKIE, v);
            }
        }
    }
    res
}

fn time_days(days: i64) -> time::Duration {
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

async fn asset(RawQuery(q): RawQuery, content_type: &'static str, body: &'static str) -> Response {
    let current = q.and_then(|q| q.strip_prefix("v=").map(|v| v == asset_version())).unwrap_or(false);
    static_file(content_type, body.as_bytes(), current)
}

async fn font_file(Path(name): Path<String>) -> Response {
    match FONTS.iter().find(|(n, _)| *n == name) {
        Some((_, bytes)) => static_file("font/woff2", bytes, true),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

// ----- auth pages ---------------------------------------------------------

/// Only same-site paths are allowed as a post-login target.
fn safe_next(next: Option<&str>) -> Option<&str> {
    // Browsers ignore tabs/newlines and treat "\" like "/" in URLs, so
    // "/\t/evil.example" would become "//evil.example".
    next.filter(|n| {
        n.starts_with('/')
            && n.len() < 4096
            && !n.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\')
            && !n.starts_with("//")
    })
}

fn auth_form(signup: bool, error: Option<&str>, allow_signup: bool, next: Option<&str>) -> Markup {
    let title = if signup { "Sign up" } else { "Log in" };
    let action = if signup { "/signup" } else { "/login" };
    let next_q = next.map(|n| format!("?next={}", urlenc(n))).unwrap_or_default();
    layout(
        title,
        None,
        html! {
            h1.brand-big { "AiNKI" }
            p.muted { "Flashcards with FSRS spaced repetition." }
            p.tagline.calm-only { "Flashcards that come back " span.it { "just before" } " you forget them." }
            h2 { (title) }
            @if let Some(e) = error { p.notice role="alert" { (e) } }
            @if next.is_some_and(|n| n.starts_with("/oauth/")) {
                p.notice { "Log in to connect an app to your AiNKI account." }
            }
            form method="post" action=(action) {
                label {
                    "Username"
                    @if !signup { span.muted { " (name or name#1234)" } }
                    input type="text" name="username" required autocomplete="username" autocapitalize="none" autocorrect="off";
                }
                label {
                    "Password"
                    input type="password" name="password" required minlength=(if signup { "8" } else { "1" })
                        autocomplete=(if signup { "new-password" } else { "current-password" });
                }
                @if let Some(n) = next { input type="hidden" name="next" value=(n); }
                @if signup {
                    input type="hidden" name="timezone" id="tz" value="";
                }
                button.btn.primary type="submit" { (title) }
            }
            @if signup {
                p { "Already have an account? " a href=(format!("/login{next_q}")) { "Log in" } }
            } @else if allow_signup {
                p { "New here? " a href=(format!("/signup{next_q}")) { "Create an account" } }
            }
        },
    )
}

#[derive(Deserialize)]
struct NextQuery {
    next: Option<String>,
}

async fn login_page(State(app): State<AppState>, user: Option<WebUser>, Query(q): Query<NextQuery>) -> Response {
    let next = safe_next(q.next.as_deref());
    if user.is_some() {
        return redirect(next.unwrap_or("/"));
    }
    auth_form(false, None, app.signup_open().await, next).into_response()
}

const LOGIN_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Failed logins allowed per client IP, and per account, per window.
const LOGIN_FAILURES_PER_IP: u32 = 20;
const LOGIN_FAILURES_PER_ACCOUNT: u32 = 50;

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    timezone: String,
    next: Option<String>,
}

async fn login(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(f): Form<LoginForm>,
) -> Response {
    let next = safe_next(f.next.as_deref());
    let ip_key = format!("login-ip:{ip}");
    let user_key = format!("login-user:{}", f.username.trim().to_lowercase());
    if app.limiter.exceeded(&ip_key, LOGIN_FAILURES_PER_IP, LOGIN_WINDOW)
        || app.limiter.exceeded(&user_key, LOGIN_FAILURES_PER_ACCOUNT, LOGIN_WINDOW)
    {
        tracing::warn!("login throttled for {ip} / {}", f.username.trim());
        let page = auth_form(
            false,
            Some("Too many failed attempts. Wait 15 minutes and try again."),
            app.signup_open().await,
            next,
        );
        return (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "900")], page).into_response();
    }
    match app.authenticate(&f.username, &f.password).await {
        Ok(user) => match app.create_session(user.id).await {
            Ok(token) => (jar.add(session_cookie(&app, token)), Redirect::to(next.unwrap_or("/"))).into_response(),
            Err(e) => WebError(e).into_response(),
        },
        Err(e) => {
            app.limiter.hit(&ip_key, LOGIN_WINDOW);
            app.limiter.hit(&user_key, LOGIN_WINDOW);
            (
                StatusCode::UNAUTHORIZED,
                auth_form(false, Some(&capitalize(&e.to_string())), app.signup_open().await, next),
            )
                .into_response()
        }
    }
}

async fn signup_page(State(app): State<AppState>, Query(q): Query<NextQuery>) -> Response {
    if !app.signup_open().await {
        return redirect("/login");
    }
    auth_form(true, None, true, safe_next(q.next.as_deref())).into_response()
}

async fn signup(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(f): Form<LoginForm>,
) -> Response {
    if !app.signup_open().await {
        return redirect("/login");
    }
    if app.limiter.check_and_hit(&format!("signup-ip:{ip}"), 10, Duration::from_secs(3600)) {
        return security::too_many_requests(3600);
    }
    let next = safe_next(f.next.as_deref());
    let result = async {
        let user = app.create_user(&f.username, &f.password, &f.timezone).await?;
        let token = app.create_session(user.id).await?;
        Ok::<_, AppError>((user, token))
    }
    .await;
    match result {
        Ok((user, token)) => {
            let welcome = format!("Welcome! Your username is {}. Use it to log in.", user.handle());
            (
                jar.add(session_cookie(&app, token)).add(flash_cookie(&welcome)),
                Redirect::to(next.unwrap_or("/")),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            auth_form(true, Some(&capitalize(&e.to_string())), true, next),
        )
            .into_response(),
    }
}

async fn logout(State(app): State<AppState>, jar: CookieJar) -> Response {
    let name = app.config.session_cookie_name();
    if let Some(c) = jar.get(name) {
        let _ = app.delete_session(c.value()).await;
    }
    (jar.remove(Cookie::build(name).path("/")), Redirect::to("/login")).into_response()
}

// ----- home ---------------------------------------------------------------

async fn home(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let decks = app.deck_summaries(&access).await?;
    let shares = app.incoming_shares(&access).await?;
    let stats = app.stats(&access, None).await?;
    let total_due: i64 = decks.iter().map(|d| d.due_total()).sum();
    let attention = app.attention(&access, None).await?.len() as i64;
    let overdue = app.overdue(&access).await?;
    let quiet = study::quiet_until(&user.0, now());
    let week = app.week(&access).await?;
    // A rough guide; real thinking time once there's enough history.
    let minutes = ((total_due as f64 * stats.seconds_per_review.unwrap_or(12.0)) / 60.0).ceil().max(1.0) as i64;
    let mut facts = Vec::new();
    if total_due > 0 {
        facts.push(format!("About {}", count(minutes, "minute")));
    }
    if let Some(r) = stats.true_retention_30d {
        facts.push(format!("{} recalled when due, last 30 days", format::percent(Some(r))));
    }
    if stats.streak_days > 1 {
        facts.push(format!("{} in a row", count(stats.streak_days, "day")));
    }
    Ok(page(
        Some(Nav::Decks),
        "Decks",
        Some(&user.0),
        Some("home"),
        html! {
            (flash(&msg))
            // On desktops the week sits beside today, as a sidebar.
            div.hero {
            section.today aria-label="Today" {
                p.date { @if user.0.catch_up { span.calm-only { "Catching up · " } } (format::long_date(&user.0.clock())) }
                @if total_due > 0 {
                    h1 { span.count { (total_due) (if total_due == 1 { " card" } else { " cards" }) } " to study today" }
                } @else if !decks.is_empty() {
                    h1 { "All done for today" }
                } @else {
                    h1 { "Welcome to AiNKI" }
                }
                @if !facts.is_empty() { p.facts { (facts.join(" · ")) } }
                @if let Some(t) = quiet {
                    p.facts { "Quiet hours until " (format::when(&user.0.clock(), t)) "." }
                } @else if total_due > 0 {
                    p { a.btn.primary.start href="/study" { "Start studying " span.arrow aria-hidden="true" { "→" } } }
                }
                (catch_up_note(&user.0, overdue))
            }
            @if !decks.is_empty() {
                aside.today-side.calm-only {
                    (week_note(&week))
                    @if user.0.catch_up && overdue >= 20 {
                        @let days = (overdue + user.0.catch_up_reviews - 1) / user.0.catch_up_reviews.max(1);
                        div.pace {
                            p.kicker { "At this pace" }
                            p.pace-days { span.n { (days) } (if days == 1 { " day" } else { " days" }) " to catch up" }
                            p.small.muted { (format::number(overdue)) " overdue, " (format::number(user.0.catch_up_reviews)) " a day." }
                        }
                    }
                    @if attention > 0 && !matches!(week.tip, insight::Tip::Attention) {
                        p.small { a href="/attention" { (count(attention, "card")) " " (if attention == 1 { "needs" } else { "need" }) " a look" } }
                    }
                }
            }
            }
            @if !shares.is_empty() {
                h2 { "Shared with you" }
                ul.list {
                    @for sh in &shares {
                        li.item {
                            div.grow {
                                strong { (deck_title(&sh.deck_name)) } br;
                                span.small { (sh.cards) (if sh.cards == 1 { " card" } else { " cards" }) " from " (sh.from) }
                            }
                            form method="post" action=(format!("/shares/{}/accept", sh.id)) {
                                button.btn type="submit" { "Accept" }
                                " "
                                button.btn type="submit" formaction=(format!("/shares/{}/decline", sh.id)) { "Decline" }
                            }
                        }
                    }
                }
                p.muted.small { "Accepting gives you your own copy. Your progress is yours; later changes by the sender don't carry over." }
            }
            @if decks.is_empty() {
                p { "No decks yet. Create one, or bring your Anki decks along." }
            } @else {
                div.table-head aria-hidden="true" {
                    span.grow { "Decks" }
                    span.num { "Due" }
                    span.num { "New" }
                    span.chev {}
                }
                ul.list.decks {
                    @for d in &decks {
                        @let due = d.learning_due + d.review_due;
                        li {
                            a.row href=(format!("/decks/{}", d.deck.id)) {
                                span.grow {
                                    span.deck-name { (deck_title(&d.deck.name)) }
                                    @if let Some(exam) = &d.deck.exam_date {
                                        span.sub { "Exam " (exam_words(&user.0, exam)) }
                                    }
                                }
                                @if d.due_total() == 0 && d.total > d.suspended {
                                    span.num.due.done title="Done for today" { (done_check(13, "done-check")) span.unit { "done for today" } }
                                } @else {
                                    span.num.due title="Due" { @if due > 0 { (format::number(due)) span.unit { " due" } } @else { (format::NONE) } }
                                }
                                span.num.new title="New" { @if d.new_today > 0 { span.dot aria-hidden="true" {} (format::number(d.new_today)) span.unit { " new" } } @else { (format::NONE) } }
                                span.chev aria-hidden="true" { "›" }
                            }
                        }
                    }
                }
            }
            @if attention > 0 {
                p.small.attention-link { a href="/attention" { (count(attention, "card")) " " (if attention == 1 { "needs" } else { "need" }) " a look" } }
            }
            p.actions.icon-links.home-actions {
                (icon_btn("/cards/new", ICON_ADD, "Add card"))
                (icon_btn("/decks/new", ICON_DECKS, "New deck"))
                (icon_btn("/import", ICON_IMPORT, "Import"))
            }
        },
    )
    .into_response())
}

// ----- studying -----------------------------------------------------------

#[derive(Deserialize)]
struct StudyQuery {
    deck: Option<i64>,
    card: Option<i64>,
    show: Option<u8>,
    t: Option<i64>,
    /// From the reveal form: felt sure (1) or not (0), and what was typed.
    sure: Option<u8>,
    typed: Option<String>,
}

/// How a card is on the page: since when, whether turned over, and what
/// came with turning it (the sure check, a typed answer).
struct Shown {
    at: i64,
    revealed: bool,
    sure: Option<bool>,
    typed: Option<String>,
}

impl Shown {
    fn front(at: i64) -> Self {
        Shown { at, revealed: false, sure: None, typed: None }
    }
}

/// What a card's study page adds around the card itself.
struct StudyExtras {
    /// The plain "Show answer" link, inside a multicard's form; empty when
    /// the reveal is a form of its own (typed answers, the sure check).
    reveal_inside: Markup,
    sure_field: Markup,
    echo: Markup,
    timed_out: bool,
}

fn study_url(deck: Option<i64>) -> String {
    match deck {
        Some(d) => format!("/study?deck={d}"),
        None => "/study".into(),
    }
}

/// The thin line and "14 of 32" under the card. `done` is in half cards
/// (see `App::session_progress`).
fn study_foot(deck_name: &str, deck_id: i64, done: i64, total: i64, q_deck: Option<i64>, card_id: Option<i64>, leech: bool) -> Markup {
    let pct = if total > 0 { (done * 50 / total).clamp(0, 100) } else { 0 };
    let total = total.max(1);
    let position = (done / 2 + 1).min(total);
    html! {
        div.study-foot {
            div.progress role="progressbar" aria-label="Today's progress" aria-valuemin="0" aria-valuemax=(total) aria-valuenow=(done / 2) {
                div.bar data-progress=(pct) style=(format!("width:{pct}%")) {}
            }
            div.study-meta {
                a href=(format!("/decks/{deck_id}")) { (deck_title(deck_name)) }
                span.count { (position) " of " (total) }
            }
            div.study-tools {
                form method="post" action="/study/undo" {
                    @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
                    button.link.icon-link type="submit" { (icon(ICON_UNDO, PAGE_ICON)) span { "Undo last answer" } }
                }
                @if let Some(id) = card_id { (icon_link(&format!("/cards/{id}{}", CardFrom::study(q_deck).query()), ICON_EDIT, "Edit card")) }
            }
            @if let (true, Some(id)) = (leech, card_id) {
                p.leech-note.small { "This card keeps slipping. " a href=(format!("/cards/{id}{}", CardFrom::study(q_deck).query())) { "Have a look after this answer" } }
            }
        }
    }
}

/// What people see for a card's state. Relearning counts as learning,
/// as in the filter and the stats.
fn state_label(state: CardState) -> &'static str {
    match state {
        CardState::New => "New",
        CardState::Learning | CardState::Relearning => "Learning",
        CardState::Review => "Review",
    }
}

/// A card's state, or "Suspended", for lists.
fn card_status(card: &Card) -> &'static str {
    if card.suspended { "Suspended" } else { state_label(card.card_state()) }
}

/// Stored rating names as button words.
fn rating_label(stored: &str) -> &str {
    match stored {
        "again" => "Again",
        "hard" => "Hard",
        "good" => "Good",
        "easy" => "Easy",
        "reset" => "Progress forgotten",
        other => other,
    }
}

/// Where a card or answer came from, in words.
fn source_label(stored: &str) -> &str {
    match stored {
        "web" => "On the website",
        "agent" => "By an agent",
        "import" => "Imported",
        "shared" => "From a shared deck",
        "manual" => "On the website",
        other => other,
    }
}

fn state_tag(state: CardState) -> Markup {
    let class = match state {
        CardState::New => "new",
        CardState::Learning | CardState::Relearning => "learning",
        CardState::Review => "review",
    };
    html! { p.card-state { span.dot.(class) aria-hidden="true" {} (state_label(state)) } }
}

/// One card. The answer side and the rating buttons are always on the page;
/// before the reveal they are hidden, and app.js shows them without a round
/// trip. Without JavaScript "Show answer" loads the revealed page instead.
#[allow(clippy::too_many_arguments)]
async fn study_card(
    app: &App,
    access: &Access,
    user: &User,
    msg: &Option<String>,
    card: &Card,
    deck: &Deck,
    q_deck: Option<i64>,
    shown: &Shown,
) -> Result<Markup, AppError> {
    let revealed = shown.revealed;
    let previews = app.previews(access, card, deck).await?;
    let preset = app.preset(access, deck.preset_id).await?;
    // Time limits, typing and the sure check are for cards seen before;
    // a new card can't be known yet.
    let seen = card.card_state() != CardState::New;
    let limit = if seen { preset.time_limit } else { 0 };
    let typed = seen && preset.typed_answers;
    let ask_sure = seen && user.confidence_check;
    // Without JavaScript the time counts until the answer is shown.
    let timed_out = revealed && limit > 0 && now() - shown.at > limit + 1;
    let (done, total) = app.session_progress(access, q_deck).await?;
    let leech = app.leech_note(card).await?.is_some();
    let four = user.four_buttons;
    let buttons: Vec<(Rating, &str, &str)> = if four {
        vec![
            (Rating::Again, "Again", "1"),
            (Rating::Hard, "Hard", "2"),
            (Rating::Good, "Good", "3"),
            (Rating::Easy, "Easy", "4"),
        ]
    } else {
        vec![(Rating::Again, "Again", "1"), (Rating::Good, "Good", "3")]
    };
    let deck_param = q_deck.map(|d| format!("&deck={d}")).unwrap_or_default();
    let show_href = format!("/study?card={}&show=1&t={}{}#answer", card.id, shown.at, deck_param);
    let reveal_form = typed || ask_sure;
    let reveal = if reveal_form {
        reveal_controls(card.id, shown.at, q_deck, revealed, typed, ask_sure)
    } else {
        reveal_button(&show_href, revealed)
    };
    let sure_value = match shown.sure {
        Some(true) => "1",
        Some(false) => "0",
        None => "",
    };
    let sure_field = html! { input.sure-field type="hidden" name="sure" value=(sure_value); };
    let echo = typed_echo(typed, shown.typed.as_deref());
    let timer = timer_box(limit, revealed);
    let timeout = html! {
        form.timeout-form method="post" action="/study/answer" hidden[!timed_out] data-stay {
            input type="hidden" name="card" value=(card.id);
            input type="hidden" name="reps" value=(card.reps);
            input type="hidden" name="t" value=(shown.at);
            input type="hidden" name="timeout" value="1";
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            (sure_field)
            p.timeout-note { "Time's up. This one counts as forgotten." }
            div.answers.cols-1 {
                button.btn.again type="submit" data-key=" " { span.label { "Continue" " " span.key { "space" } } }
            }
        }
    };
    if let Some(how) = card.multi {
        let items = app.card_items(card).await?;
        let extras = StudyExtras {
            reveal_inside: if reveal_form { html! {} } else { reveal.clone() },
            sure_field: sure_field.clone(),
            echo: echo.clone(),
            timed_out,
        };
        let body = multi_study(card, how, &items, q_deck, shown, &extras);
        return Ok(page(
            Some(Nav::Study),
            "Study",
            Some(user),
            Some("study"),
            html! {
                (flash(msg))
                section.study.revealed[revealed].timed-out[timed_out] {
                    (body)
                    (timer)
                    @if reveal_form { (reveal) }
                    (timeout)
                    (study_foot(&deck.name, deck.id, done, total, q_deck, Some(card.id), leech))
                }
            },
        ));
    }
    Ok(page(
        Some(Nav::Study),
        "Study",
        Some(user),
        Some("study"),
        html! {
            (flash(msg))
            section.study.revealed[revealed].timed-out[timed_out] {
                div.card-area {
                    (state_tag(card.card_state()))
                    section.face.front { (face(&card.front, &card.front_image)) }
                    hr.divider hidden[!revealed];
                    section.face.back id="answer" hidden[!revealed] {
                        (echo)
                        (face(&card.back, &card.back_image))
                    }
                }
                (timer)
                (reveal)
                (timeout)
                form.answers.cols-2[!four].cols-4[four] method="post" action="/study/answer" hidden[!revealed || timed_out] {
                    input type="hidden" name="card" value=(card.id);
                    input type="hidden" name="reps" value=(card.reps);
                    input type="hidden" name="t" value=(shown.at);
                    @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
                    (sure_field)
                    @for (rating, label, key) in &buttons {
                        @let preview = previews.iter().find(|(r, _)| r == rating).map(|(_, p)| p.as_str()).unwrap_or("");
                        button.btn.primary[*rating == Rating::Good].again[*rating == Rating::Again] type="submit" name="rating" value=(*rating as i64) data-key=(key) {
                            span.label { (label) " " span.key { (key) } }
                            span.ivl { (preview) }
                        }
                    }
                }
                (study_foot(&deck.name, deck.id, done, total, q_deck, Some(card.id), leech))
            }
        },
    ))
}

/// The countdown for a timed card. Without JavaScript it only names the
/// limit, and the time counts until the answer is shown.
fn timer_box(limit: i64, revealed: bool) -> Markup {
    if limit <= 0 {
        return html! {};
    }
    html! {
        div.timer data-limit=(limit) hidden[revealed] data-stay {
            div.timer-line aria-hidden="true" { div.timer-fill {} }
            div.timer-meta {
                span.timer-secs role="timer" { (limit) " s" }
                button.link.extend type="button" data-key="+" hidden { "+10 s" }
            }
        }
    }
}

/// "Show answer" as a form: with a box for the typed answer, or as the
/// sure check's two buttons. Without JavaScript it loads the answer side.
fn reveal_controls(card_id: i64, at: i64, q_deck: Option<i64>, revealed: bool, typed: bool, ask_sure: bool) -> Markup {
    html! {
        form.reveal.reveal-form method="get" action="/study#answer" hidden[revealed] {
            input type="hidden" name="card" value=(card_id);
            input type="hidden" name="show" value="1";
            input type="hidden" name="t" value=(at);
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            @if typed {
                label.typed-answer {
                    span.face-label { "Your answer" }
                    input type="text" name="typed" autocomplete="off" autocapitalize="off" spellcheck="false" maxlength="500" placeholder="Type it, then Enter";
                }
            }
            @if ask_sure {
                p.sure-ask { "Do you know it?" }
                div.sure-buttons {
                    button.btn.primary type="submit" name="sure" value="1" data-key="s" { span.label { "Sure" " " span.key { "S" } } }
                    button.btn type="submit" name="sure" value="0" data-key="u" { span.label { "Not sure" " " span.key { "U" } } }
                }
            } @else {
                button.btn.primary.show type="submit" data-key=" " { "Show answer" span.key { "space" } }
            }
        }
    }
}

/// What was typed, shown above the answer to compare. Filled in by app.js
/// when it reveals the card.
fn typed_echo(typed: bool, text: Option<&str>) -> Markup {
    if !typed {
        return html! {};
    }
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    html! {
        div.typed-echo hidden[text.is_none()] data-stay {
            span.face-label { "You wrote" }
            p.typed-text { (text.unwrap_or("")) }
        }
    }
}

fn reveal_button(href: &str, revealed: bool) -> Markup {
    html! {
        p.reveal hidden[revealed] {
            a.btn.primary.show data-key=" " href=(href) { "Show answer" span.key { "space" } }
        }
    }
}

/// One item's Markdown, without the paragraph around it when it is a
/// single line, so it sits inside a row.
fn item_md(text: &str) -> Markup {
    let html = markdown::render(text);
    let t = html.trim();
    let inner = t
        .strip_prefix("<p>")
        .and_then(|x| x.strip_suffix("</p>"))
        .filter(|x| !x.contains("<p>"));
    PreEscaped(inner.unwrap_or(t).to_string())
}

/// A multicard's back as it is studied: everything on it in order, every
/// item a row. Asked items are rows to tap when you remembered them (when
/// `tap`; the editor's preview only shows them), the others show as known.
fn multi_back(pieces: &[Piece], asked: &[bool], tap: bool) -> Markup {
    // Items next to each other share one list: a block is text, or items
    // with their numbers.
    type Block<'a> = (Option<&'a str>, Vec<(usize, &'a str)>);
    let mut blocks: Vec<Block> = Vec::new();
    let mut n = 0;
    for p in pieces {
        match p {
            Piece::Text(t) => blocks.push((Some(t), vec![])),
            Piece::Item(t) => {
                match blocks.last_mut() {
                    Some((None, items)) => items.push((n, t)),
                    _ => blocks.push((None, vec![(n, t)])),
                }
                n += 1;
            }
        }
    }
    html! {
        @for (text, items) in &blocks {
            @if let Some(t) = text {
                div.md { (PreEscaped(markdown::render(t))) }
            } @else {
                ol.list-items {
                    @for (n, text) in items {
                        @let n = *n;
                        @if asked.get(n).copied().unwrap_or(true) && tap {
                            li {
                                input type="hidden" name="asked" value=(n);
                                label.row.item data-key=[(n < 9).then(|| (n + 1).to_string())] {
                                    input type="checkbox" name="recalled" value=(n);
                                    span.num { (n + 1) }
                                    span.text { (item_md(text)) }
                                    span.mark { "Remembered" }
                                }
                            }
                        } @else {
                            li.known[tap].shown[!tap] {
                                span.num { (n + 1) }
                                span.text { (item_md(text)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The back's pieces for its stored items. Should the back no longer split
/// into those (it always does after a save), the items alone are shown.
fn pieces_for(card: &Card, how: Split, items: &[multi::CardItem]) -> Vec<Piece> {
    match multi::split(&card.back, how) {
        Some(p) if p.iter().filter(|p| matches!(p, Piece::Item(_))).count() == items.len() => p,
        _ => items.iter().map(|i| Piece::Item(i.text.clone())).collect(),
    }
}

/// A multicard: the front (with the items you already know while
/// relearning), then the whole back with every item as a row to tap when
/// you remembered it, and one Done. It is all one form, so it also works
/// without JavaScript.
fn multi_study(card: &Card, how: Split, items: &[multi::CardItem], q_deck: Option<i64>, shown: &Shown, extras: &StudyExtras) -> Markup {
    let revealed = shown.revealed;
    let asked = multi::asked(card, items, now());
    let n_asked = asked.iter().filter(|a| **a).count();
    let known: Vec<&multi::CardItem> = items.iter().zip(&asked).filter(|(_, a)| !**a).map(|(i, _)| i).collect();
    let pieces = pieces_for(card, how, items);
    html! {
        form.list-answer method="post" action="/study/answer" {
            input type="hidden" name="card" value=(card.id);
            input type="hidden" name="reps" value=(card.reps);
            input type="hidden" name="t" value=(shown.at);
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            (extras.sure_field)
            div.card-area {
                (state_tag(card.card_state()))
                section.face.front { (face(&card.front, &card.front_image)) }
                @if !known.is_empty() {
                    div.list-known {
                        p.known-label { "You know" }
                        ul { @for i in &known { li { (item_md(multi::title(&i.text))) } } }
                        p.more { "and " (n_asked) " more?" }
                    }
                }
                hr.divider hidden[!revealed];
                section.face.back.list id="answer" hidden[!revealed] {
                    (extras.echo)
                    @if let Some(img) = &card.back_image { img src=(format!("/media/{img}")) alt=""; }
                    p.list-how { "Tap the ones you remembered." }
                    (multi_back(&pieces, &asked, true))
                }
            }
            (extras.reveal_inside)
            div.answers.cols-1 hidden[!revealed || extras.timed_out] {
                button.btn.primary type="submit" data-key=" " {
                    span.label { "Done" " " span.key { "space" } }
                    span.ivl data-remembered=(n_asked) { "0 of " (n_asked) " remembered" }
                }
            }
        }
    }
}

/// The card being written as it will look when studied.
fn card_preview(front: &str, back: &str, how: Option<Split>) -> Markup {
    let pieces = how.and_then(|h| multi::split(back, h));
    html! {
        section.face.front { (face(front, &None)) }
        hr.divider;
        @match &pieces {
            Some(p) => section.face.back.list { (multi_back(p, &[], false)) },
            None => section.face.back { (face(back, &None)) },
        }
    }
}

#[derive(Deserialize)]
struct PreviewForm {
    /// The card being edited, so it doesn't count as like itself.
    card: Option<i64>,
    #[serde(default)]
    front: String,
    #[serde(default)]
    back: String,
    #[serde(default)]
    multi: String,
}

/// For the card form while you write: how the card will look, and which
/// ways the back can be learned item by item.
async fn card_preview_json(State(app): State<AppState>, user: WebUser, Form(f): Form<PreviewForm>) -> WebResult {
    let splits: Vec<&str> = multi::splits(&f.back).into_iter().map(Split::as_str).collect();
    let how = Split::parse(&f.multi);
    let html = card_preview(&f.front, &f.back, how).into_string();
    let hints = write_hints(&app, &user.access(), &f.front, &f.back, how, f.card).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({ "splits": splits, "html": html, "hints": hint_lines(&hints).into_string() }).to_string(),
    )
        .into_response())
}

async fn study(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Query(q): Query<StudyQuery>) -> WebResult {
    let access = user.access();

    // Answer side of a specific card.
    if let (Some(card_id), Some(1)) = (q.card, q.show) {
        let card = app.card(&access, card_id).await?;
        let deck = app.deck(&access, card.deck_id).await?;
        let shown = Shown {
            at: q.t.unwrap_or_else(now),
            revealed: true,
            sure: q.sure.map(|s| s == 1),
            typed: q.typed.clone(),
        };
        return Ok(study_card(&app, &access, &user.0, &msg, &card, &deck, q.deck, &shown).await?.into_response());
    }

    if let Some(until) = study::quiet_until(&user.0, now()) {
        return Ok(rest_page(&user.0, &msg, q.deck, until).into_response());
    }
    if let Some(minutes) = app.break_due(&access).await? {
        return Ok(break_page(&user.0, &msg, q.deck, minutes).into_response());
    }

    let Some(sc) = app.next_card(&access, q.deck).await? else {
        let decks = app.deck_summaries(&access).await?;
        let any_cards = decks.iter().any(|d| d.total > 0);
        let stats = app.stats(&access, q.deck).await?;
        let next = app.next_review_at(&access, q.deck).await?;
        return Ok(page(
            Some(Nav::Study),
            "Study",
            Some(&user.0),
            Some("study-done"),
            html! {
                (flash(&msg))
                section.session-end {
                    @if any_cards {
                        div.end-mark aria-hidden="true" { div.line {} (done_check(26, "check")) }
                        h1 { "All done." }
                        @if stats.cards_today > 0 {
                            p.summary {
                                "Today you studied " (count(stats.cards_today, "card"))
                                @if stats.minutes_today >= 0.5 { " in " (format::minutes(stats.minutes_today)) }
                                "."
                            }
                        }
                        p.summary {
                            @if let Some(t) = next {
                                "Next cards are due " (format::when(&user.0.clock(), t)) "."
                            } @else {
                                "Nothing else is scheduled yet."
                            }
                        }
                    } @else {
                        h1 { "No cards yet" }
                        p.summary { "Add a few cards, or import a deck from Anki, and they'll show up here." }
                        p { a.btn.primary href=(match q.deck { Some(d) => format!("/cards/new?deck={d}"), None => "/cards/new".into() }) { "Add cards" } }
                    }
                    div.end-links {
                        a href="/" { "Back to decks" }
                        form method="post" action="/study/undo" {
                            @if let Some(d) = q.deck { input type="hidden" name="deck" value=(d); }
                            button.link.icon-link type="submit" { (icon(ICON_UNDO, PAGE_ICON)) span { "Undo last answer" } }
                        }
                    }
                }
            },
        )
        .into_response());
    };
    Ok(study_card(&app, &access, &user.0, &msg, &sc.card, &sc.deck, q.deck, &Shown::front(now())).await?.into_response())
}

/// "7:00": the time of day of a moment, in the user's zone.
fn clock_time(clock: &crate::scheduler::Clock, ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&clock.tz).format("%-H:%M").to_string()).unwrap_or_default()
}

/// Quiet hours: a rest screen instead of cards.
fn rest_page(user: &User, msg: &Option<String>, deck: Option<i64>, until: i64) -> Markup {
    page(
        Some(Nav::Study),
        "Quiet hours",
        Some(user),
        Some("study-done"),
        html! {
            (flash(msg))
            section.session-end.rest {
                p.kicker.calm-only { "Quiet hours" }
                h1 {
                    span.simple-only { "Quiet hours" }
                    span.calm-only { "Rest until " (clock_time(&user.clock(), until)) }
                }
                p.summary { "You asked for rest until " (format::when(&user.clock(), until)) ". Sleep helps the day's learning settle." }
                p.summary.calm-only { "Agents won't quiz you now either." }
                div.end-links {
                    a href="/" { "Back to decks" }
                    a.calm-only href="/learn/sleep" { "Why sleep helps" }
                    form method="post" action="/study/anyway" {
                        @if let Some(d) = deck { input type="hidden" name="deck" value=(d); }
                        button.link type="submit" { "Study anyway" }
                    }
                }
            }
        },
    )
}

/// The session has reached the length set in settings.
fn break_page(user: &User, msg: &Option<String>, deck: Option<i64>, minutes: i64) -> Markup {
    page(
        Some(Nav::Study),
        "Time for a break",
        Some(user),
        Some("study-done"),
        html! {
            (flash(msg))
            section.session-end.rest.break {
                p.kicker.calm-only { "Break" }
                h1 { "Time for a break" }
                p.summary { "You have studied for " (study::minutes_words(minutes)) ". Stand up, look out of the window, then carry on if you like." }
                div.end-links {
                    a href="/" { "Stop here" }
                    form id="keep-going" method="post" action="/study/keep-going" {
                        @if let Some(d) = deck { input type="hidden" name="deck" value=(d); }
                        button.link type="submit" { "Keep going" }
                    }
                }
                // On desktops a break can run here: desk.js counts it down
                // and sends the form above when it is over.
                div.break-start.calm-only {
                    button.btn.primary type="button" data-break="300" { "Start a 5 minute break" }
                    button.btn type="submit" form="keep-going" { "Skip break" }
                }
                div.break-run.calm-only hidden {
                    p.break-clock role="timer" aria-live="off" { "5:00" }
                    div.break-line aria-hidden="true" { div.break-fill {} }
                    p.summary { "The next card comes back by itself when the time is up." }
                    p { button.btn type="submit" form="keep-going" { "End break now" } }
                }
            }
        },
    )
}

async fn study_anyway(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    app.lift_quiet(&user.0).await?;
    Ok(redirect(study_url(f.deck)))
}

async fn study_keep_going(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    app.skip_break(user.0.id).await?;
    Ok(redirect(study_url(f.deck)))
}

#[derive(Deserialize)]
struct AnswerForm {
    card: i64,
    /// Plain cards: the button pressed.
    rating: Option<i64>,
    /// List cards: positions of the items asked, and of those tapped as
    /// remembered; the rest of the asked ones were missed.
    #[serde(default)]
    asked: Vec<usize>,
    #[serde(default)]
    recalled: Vec<usize>,
    reps: i64,
    t: Option<i64>,
    deck: Option<i64>,
    /// The sure check: "1" sure, "0" not, empty when not asked.
    sure: Option<String>,
    /// Time ran out on a timed card: it counts as Again.
    timeout: Option<u8>,
}

async fn study_answer(State(app): State<AppState>, user: WebUser, Form(f): Form<AnswerForm>) -> WebResult {
    // Like Anki, cap the counted thinking time at 60 seconds.
    let duration = f.t.map(|t| ((now() - t).clamp(0, 60)) * 1000);
    let access = user.access();
    let timed_out = f.timeout == Some(1);
    let sure = match f.sure.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    let answered = match f.rating {
        // A timeout misses a multicard's asked items all at once too.
        _ if timed_out => app.answer_card(&access, f.card, Rating::Again, duration, Some(f.reps)).await,
        Some(r) => {
            let rating = Rating::from_i64(r).ok_or_else(|| AppError::bad("invalid rating"))?;
            app.answer_card(&user.access(), f.card, rating, duration, Some(f.reps)).await
        }
        None => {
            let missed = f.asked.into_iter().filter(|n| !f.recalled.contains(n)).collect();
            app.answer_list(&user.access(), f.card, missed, duration, Some(f.reps)).await
        }
    };
    if answered.is_ok() {
        app.note_answer(&access, f.card, sure, timed_out).await?;
    }
    match answered {
        Ok(_) | Err(AppError::Conflict(_)) => Ok(redirect(study_url(f.deck))),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
struct DeckForm {
    deck: Option<i64>,
}

async fn study_undo(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    let restored = app.undo_last(&user.access()).await?;
    let base = study_url(f.deck);
    let sep = if base.contains('?') { '&' } else { '?' };
    Ok(match restored {
        Some(card) => redirect(format!("{base}{sep}card={}&show=1&t={}", card.id, now())),
        None => redirect_msg(base, "Nothing to undo."),
    })
}

/// After time away: offer catch-up mode, or say what it does while on.
fn catch_up_note(user: &User, overdue: i64) -> Markup {
    html! {
        @if user.catch_up {
            form.catch-up method="post" action="/settings/catch-up" {
                p.small.muted {
                    @if overdue < 20 {
                        "You have caught up. "
                        button.link type="submit" { "Turn catch-up mode off" }
                    } @else {
                        "Catch-up mode: no new cards, at most " (user.catch_up_reviews) " reviews a day, the ones you most likely still know first. "
                        a href="/learn/backlog" { "Why" }
                        button.link.calm-only type="submit" { "Turn off catch-up mode" }
                    }
                }
            }
        } @else if overdue >= study::BACKLOG_HINT {
            form.catch-up method="post" action="/settings/catch-up" {
                input type="hidden" name="on" value="1";
                p.small.muted {
                    (format::number(overdue)) " cards piled up while you were away. "
                    button.link type="submit" { "Catch up over a few days" }
                }
            }
        }
    }
}

// ----- lessons ------------------------------------------------------------

fn evidence_tag(e: lessons::Evidence) -> Markup {
    html! { span.evidence.(e.class()) { (e.label()) } }
}

async fn learn_page(user: WebUser) -> WebResult {
    let all = lessons::all();
    Ok(layout_in(
        Nav::Learn,
        "Learn",
        Some(&user.0),
        html! {
            h1 { "Learn" }
            p.lead { "Short lessons on how AiNKI schedules your cards, what research says about memory, and how to write cards that stick. Each says how strong the evidence is, and each can be added as a deck." }
            @for (key, name, sub) in lessons::GROUPS {
                h2 { (name) }
                p.muted.small { (sub) }
                ul.list.links.lessons {
                    @for l in all.iter().filter(|l| l.group == *key) {
                        li {
                            a.row href=(format!("/learn/{}", l.slug)) {
                                span.grow {
                                    span { (l.title) }
                                    span.sub.muted.small { (l.summary) }
                                }
                                (evidence_tag(l.evidence))
                                span.chev aria-hidden="true" { "›" }
                            }
                        }
                    }
                }
            }
        },
    )
    .into_response())
}

async fn lesson_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Path(slug): Path<String>) -> WebResult {
    let lesson = lessons::find(&slug).ok_or(AppError::NotFound)?;
    let access = user.access();
    let deck = app.deck_by_name(&access, &lessons::deck_name(lesson)).await?;
    let all = lessons::all();
    let i = all.iter().position(|l| l.slug == lesson.slug).unwrap_or(0);
    let next = all.get(i + 1);
    Ok(layout_in(
        Nav::Learn,
        &lesson.title,
        Some(&user.0),
        html! {
            (flash(&msg))
            (parent("/learn", "Learn"))
            article.lesson {
                h1 { (lesson.title) }
                p.lesson-meta { (evidence_tag(lesson.evidence)) }
                div.md.prose { (PreEscaped(markdown::render(&lesson.body))) }
            }
            div.lesson-deck {
                @if let Some(d) = deck {
                    p.muted.small { "This lesson is in your decks. " a href=(format!("/decks/{}", d.id)) { "Open it" } }
                } @else {
                    form method="post" action=(format!("/learn/{}/deck", lesson.slug)) {
                        p.muted.small { "Learn this lesson with " (count(lesson.cards.len() as i64, "card")) "." }
                        button.btn type="submit" { "Add as a deck" }
                    }
                }
            }
            @if let Some(n) = next {
                p.next-lesson { "Next: " a href=(format!("/learn/{}", n.slug)) { (n.title) } }
            }
        },
    )
    .into_response())
}

async fn lesson_deck(State(app): State<AppState>, user: WebUser, Path(slug): Path<String>) -> WebResult {
    let lesson = lessons::find(&slug).ok_or(AppError::NotFound)?;
    let access = user.access();
    let name = lessons::deck_name(lesson);
    if let Some(d) = app.deck_by_name(&access, &name).await? {
        return Ok(redirect_msg(format!("/decks/{}", d.id), "You already have this deck."));
    }
    or_back(format!("/learn/{slug}"), async {
        let deck = app.create_deck(&access, &name, &lesson.summary, None).await?;
        for (front, back) in &lesson.cards {
            app.create_card(&access, deck.id, front, back, None, None, None).await?;
        }
        Ok(redirect_msg(format!("/decks/{}", deck.id), "Lesson added as a deck."))
    }
    .await)
}

// ----- decks --------------------------------------------------------------

fn preset_select(presets: &[crate::service::Preset], selected: Option<i64>) -> Markup {
    html! {
        select name="preset_id" {
            @for p in presets {
                option value=(p.id) selected[Some(p.id) == selected] {
                    (p.name) " · " (format!("{:.0}%", p.desired_retention * 100.0))
                }
            }
        }
    }
}

/// What a built-in preset is for, as a goal: "Pass an exam on a date".
fn goal_words(template: Option<&str>) -> Option<&'static str> {
    Some(match template? {
        "standard" => "Learn it for the long run",
        "exam" => "Pass an exam on a date",
        "language" => "Many small cards, like vocabulary",
        "relaxed" => "Learn on a small time budget",
        "maintenance" => "Keep what I already know",
        _ => return None,
    })
}

/// A new deck picks a preset by what it is for. Presets made by hand are
/// offered by name after the goals.
fn goal_picker(presets: &[crate::service::Preset], selected: Option<i64>) -> Markup {
    let order = ["standard", "exam", "language", "relaxed", "maintenance"];
    let mut sorted: Vec<&crate::service::Preset> = presets.iter().collect();
    sorted.sort_by_key(|p| p.template.as_deref().and_then(|t| order.iter().position(|o| *o == t)).unwrap_or(order.len()));
    html! {
        fieldset.goals {
            legend { "What is this deck for?" }
            @for p in sorted {
                label.goal {
                    input type="radio" name="preset_id" value=(p.id) checked[Some(p.id) == selected];
                    span.grow {
                        span.goal-name { (goal_words(p.template.as_deref()).unwrap_or(&p.name)) }
                        span.muted.small {
                            (format!("{:.0}%", p.desired_retention * 100.0)) " remembered · "
                            @if p.new_per_day == 0 { "no new cards" } @else { (count(p.new_per_day, "new card")) " a day" }
                            @if goal_words(p.template.as_deref()).is_some() { " · " (p.name) }
                        }
                    }
                }
            }
        }
        p.hint.muted.small { "You can change this later. " a href="/presets" { "About presets" } }
    }
}

/// A deck written as a sheet: its name large, a description, then its
/// preset and exam date as lines. The same sheet creates and edits.
fn deck_sheet(
    deck: Option<&crate::service::Deck>,
    presets: &[crate::service::Preset],
    selected: Option<i64>,
) -> Markup {
    html! {
        div.sheet.form {
            label.title { span.face-label { "Name" }
                input type="text" name="name" required autofocus[deck.is_none()] placeholder="e.g. Spanish verbs" value=[deck.map(|d| &d.name)];
            }
            label.title { span.face-label { "Description " span.muted.optional { "(optional)" } }
                textarea name="description" rows="2" placeholder="What it's for" { @if let Some(d) = deck { (d.description) } }
            }
            hr.divider;
            @if deck.is_none() {
                (goal_picker(presets, selected))
            } @else {
                label.line { "Preset" span.select { (preset_select(presets, selected)) } }
                p.hint.muted.small { "How much you want to remember and how many cards a day. " a href="/presets" { "About presets" } }
            }
            label.line { span { "Exam date " span.muted.optional { "(optional)" } }
                input type="date" name="exam_date" value=(deck.and_then(|d| d.exam_date.clone()).unwrap_or_default());
            }
            p.hint.muted.small { "With an exam date, no card in this deck is scheduled after the exam." }
        }
    }
}

/// The new-deck page; `draft` and `error` when a sent form comes back.
fn deck_new_view(user: &User, presets: &[crate::service::Preset], draft: Option<&Deck>, error: Option<&str>) -> Markup {
    let selected = draft
        .map(|d| d.preset_id)
        .filter(|id| *id > 0)
        .or_else(|| presets.iter().find(|p| p.template.as_deref() == Some("standard")).map(|p| p.id))
        .or_else(|| presets.first().map(|p| p.id));
    layout_in(
        Nav::Add,
        "New deck",
        Some(user),
        html! {
            (add_tabs("deck"))
            h1 { "New deck" }
            (form_error(error))
            form method="post" action="/decks/new" {
                (deck_sheet(draft, presets, selected))
                div.form-foot.end { button.btn.primary type="submit" { "Create deck" } }
            }
        },
    )
}

async fn deck_new_page(State(app): State<AppState>, user: WebUser) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(deck_new_view(&user.0, &presets, None, None).into_response())
}

#[derive(Deserialize)]
struct DeckCreateForm {
    name: String,
    #[serde(default)]
    description: String,
    preset_id: Option<i64>,
    exam_date: Option<String>,
}

impl DeckCreateForm {
    /// What was typed, to show the form again.
    fn draft(&self, id: i64, user_id: i64) -> Deck {
        Deck {
            id,
            user_id,
            name: self.name.clone(),
            description: self.description.clone(),
            preset_id: self.preset_id.unwrap_or(0),
            exam_date: self.exam_date.clone().filter(|e| !e.trim().is_empty()),
            created_at: 0,
        }
    }
}

async fn deck_create(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckCreateForm>) -> WebResult {
    let access = user.access();
    let result = async {
        let deck = app.create_deck(&access, &f.name, &f.description, f.preset_id).await?;
        if let Some(exam) = f.exam_date.as_deref().filter(|e| !e.trim().is_empty()) {
            app.update_deck(&access, deck.id, None, None, None, Some(exam)).await?;
        }
        Ok::<_, AppError>(deck)
    }
    .await;
    match result {
        // Straight on to filling it.
        Ok(deck) => Ok(redirect_msg(format!("/cards/new?deck={}", deck.id), "Deck created. Add its first card.")),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let presets = app.list_presets(&access).await?;
            Ok(form_again(deck_new_view(&user.0, &presets, Some(&f.draft(0, user.0.id)), Some(&msg))))
        }
    }
}

#[derive(Deserialize, Default)]
struct DeckQuery {
    q: Option<String>,
    state: Option<String>,
    page: Option<i64>,
}

async fn deck_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
    Query(q): Query<DeckQuery>,
) -> WebResult {
    let access = user.access();
    let summary = app
        .deck_summaries(&access)
        .await?
        .into_iter()
        .find(|d| d.deck.id == id)
        .ok_or(AppError::NotFound)?;
    let page = q.page.unwrap_or(0).max(0);
    let per_page = 50;
    let (cards, total) = app
        .list_cards(
            &access,
            &CardFilter {
                deck_id: Some(id),
                query: q.q.clone(),
                state: q.state.clone(),
                limit: Some(per_page),
                offset: Some(page * per_page),
            },
        )
        .await?;
    let d = &summary.deck;
    let attention = app.attention(&access, Some(id)).await?.len() as i64;
    let base = format!(
        "/decks/{id}?q={}&state={}",
        urlenc(q.q.as_deref().unwrap_or("")),
        urlenc(q.state.as_deref().unwrap_or(""))
    );
    Ok(layout_in(
        Nav::Decks,
        &deck_plain(&d.name),
        Some(&user.0),
        html! {
            (flash(&msg))
            // On desktops: the deck and its one action on the left, its
            // settings beside it, the rest in a menu.
            div.deck-hero {
            div.deck-main {
            div.deck-title {
                h1 { (deck_title(&d.name)) }
                button.more-toggle.calm-only type="button" aria-label="More" aria-expanded="false" aria-controls="deck-menu" { span {} span {} span {} }
            }
            @if !d.description.is_empty() { p { (d.description) } }
            dl.stats-inline {
                div { dt { "New today" } dd { (format::number(summary.new_today)) } }
                div { dt { "Due" } dd { (format::number(summary.learning_due + summary.review_due)) } }
                div { dt { "Cards" } dd { (format::number(summary.total)) } }
            }
            @if let Some(exam) = &d.exam_date { p.muted { "Exam " (exam_words(&user.0, exam)) } }
            @if summary.due_total() > 0 {
                p { a.btn.primary.start href=(format!("/study?deck={id}")) { "Start studying " span.arrow aria-hidden="true" { "→" } } }
            }
            p.actions.icon-links.deck-menu id="deck-menu" {
                (icon_btn(&format!("/cards/new?deck={id}"), ICON_ADD, "Add card"))
                (icon_btn(&format!("/decks/{id}/edit"), ICON_EDIT, "Edit deck"))
                (icon_btn(&format!("/decks/{id}/edit#share"), ICON_SHARE, "Share"))
                (icon_btn(&format!("/stats?deck={id}"), ICON_STATS, "Stats"))
                a.btn.icon-link.calm-only href=(format!("/decks/{id}/export.csv")) { span { "Export as CSV" } }
            }
            }
            div.deck-side {
            p.muted.small { "Preset: " a href=(format!("/presets/{}", d.preset_id)) { (summary.preset_name) } }
            @if attention > 0 {
                p.small { a href=(format!("/attention?deck={id}")) { (count(attention, "card")) " " (if attention == 1 { "needs" } else { "need" }) " a look" } }
            }
            }
            }

            div.cards-head {
                h2 { "Cards" }
                a.calm-only href=(format!("/cards/new?deck={id}")) { "Add card" }
            }
            form.search method="get" action=(format!("/decks/{id}")) {
                input type="search" name="q" value=(q.q.clone().unwrap_or_default()) placeholder="Search text" aria-label="Search";
                select name="state" aria-label="Filter" {
                    @for (v, l) in [("", "All"), ("new", "New"), ("learning", "Learning"), ("review", "Review"), ("due", "Due"), ("suspended", "Suspended")] {
                        option value=(v) selected[q.state.as_deref().unwrap_or("") == v] { (l) }
                    }
                }
                button.btn type="submit" { "Find" }
            }
            @let filtered = q.q.as_deref().is_some_and(|t| !t.trim().is_empty()) || q.state.as_deref().is_some_and(|t| !t.is_empty());
            @if total == 0 && !filtered {
                p.empty.muted { "No cards yet. " a href=(format!("/cards/new?deck={id}")) { "Add the first card." } }
            } @else if total == 0 {
                p.empty.muted {
                    @if let Some(t) = q.q.as_deref().filter(|t| !t.trim().is_empty()) { "No cards match “" (t) "”. " } @else { "No cards match this filter. " }
                    a href=(format!("/decks/{id}")) { "Show all cards." }
                }
            } @else {
                p.muted.small { (count(total, "card")) }
            }
            div.table-head.calm-only aria-hidden="true" hidden[cards.is_empty()] { span.grow { "Card" } span.num { "State" } span.chev {} }
            ul.list.cards hidden[cards.is_empty()] {
                @for c in &cards {
                    li {
                        a.row href=(format!("/cards/{}", c.id)) {
                            span.grow {
                                @if c.front.is_empty() { "[image]" } @else { (truncate(&c.front, 80)) }
                                br;
                                span.muted.small { (truncate(&c.back, 80)) }
                            }
                            span.counts.small {
                                (card_status(c))
                            }
                            span.chev.calm-only aria-hidden="true" { "›" }
                        }
                    }
                }
            }
            @if page > 0 || (page + 1) * per_page < total {
                nav.actions.quiet-links aria-label="Pages" {
                    @if page > 0 { a.btn href=(format!("{base}&page={}", page - 1)) { "‹ Previous" } }
                    @if (page + 1) * per_page < total { a.btn href=(format!("{base}&page={}", page + 1)) { "Next ›" } }
                }
            }
            p.icon-links.deck-export { (icon_link(&format!("/decks/{id}/export.csv"), ICON_EXPORT, "Export as CSV")) }
        },
    )
    .into_response())
}

/// One-line plain preview of Markdown text for lists.
fn truncate(s: &str, n: usize) -> String {
    let flat: String = s
        .replace('\n', " ")
        .chars()
        .filter(|c| !matches!(c, '*' | '`' | '#' | '_'))
        .collect();
    if flat.chars().count() > n {
        flat.chars().take(n).collect::<String>() + "…"
    } else {
        flat
    }
}

/// The edit-deck page; `draft` and `error` when a sent form comes back.
async fn deck_edit_view(
    app: &App,
    user: &User,
    id: i64,
    msg: &Option<String>,
    draft: Option<&Deck>,
    error: Option<&str>,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let deck = app.deck(&access, id).await?;
    let presets = app.list_presets(&access).await?;
    let shares = app.outgoing_shares(&access, id).await?;
    let shown = draft.unwrap_or(&deck);
    Ok(layout_in(
        Nav::Decks,
        "Edit deck",
        Some(user),
        html! {
            (flash(msg))
            (parent(&format!("/decks/{id}"), &deck_plain(&deck.name)))
            h1 { "Edit deck" }
            (form_error(error))
            form method="post" action=(format!("/decks/{id}/edit")) {
                (deck_sheet(Some(shown), &presets, Some(shown.preset_id)))
                div.form-foot.end { button.btn.primary type="submit" { "Save" } }
            }
            h2 id="share" { "Share" }
            p { "Send a copy of this deck to someone. They get the cards with fresh progress; your deck stays as it is." }
            form method="post" action=(format!("/decks/{id}/share")) {
                div.sheet.form {
                    label.line { "Their username"
                        input type="text" name="user" required autocapitalize="none" autocorrect="off" placeholder="name#1234";
                    }
                }
                div.form-foot.end { button.btn type="submit" { "Share" } }
            }
            @if !shares.is_empty() {
                p { "Waiting for them to accept:" }
                ul.list {
                    @for sh in &shares {
                        li.item {
                            span.grow { (sh.to) }
                            form method="post" action=(format!("/shares/{}/cancel", sh.id)) {
                                button.btn type="submit" { "Cancel" }
                            }
                        }
                    }
                }
            }
            details.danger {
                summary { "Delete this deck…" }
                form method="post" action=(format!("/decks/{id}/delete")) {
                    label.check { input type="checkbox" name="confirm" value="1" required; " Yes, delete it and all its cards" }
                    button.btn.danger type="submit" { "Delete deck" }
                }
            }
        },
    ))
}

async fn deck_edit_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
) -> WebResult {
    Ok(deck_edit_view(&app, &user.0, id, &msg, None, None).await?.into_response())
}

async fn deck_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<DeckCreateForm>,
) -> WebResult {
    let result = app
        .update_deck(
            &user.access(),
            id,
            Some(&f.name),
            Some(&f.description),
            f.preset_id,
            Some(f.exam_date.as_deref().unwrap_or("")),
        )
        .await;
    match result {
        Ok(_) => Ok(redirect_msg(format!("/decks/{id}/edit"), "Deck saved.")),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let page = deck_edit_view(&app, &user.0, id, &None, Some(&f.draft(id, user.0.id)), Some(&msg)).await?;
            Ok(form_again(page))
        }
    }
}

async fn deck_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(format!("/decks/{id}/edit"), async {
        let n = app.delete_deck(&user.access(), id).await?;
        Ok(redirect_msg("/", &format!("Deck deleted, with its {}.", count(n, "card"))))
    }
    .await)
}

async fn deck_export(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    let access = user.access();
    let deck = app.deck(&access, id).await?;
    let csv = app.export_csv(&access, id).await?;
    let name = deck_plain(&deck.name);
    // Plain ASCII for old browsers, the real name (UTF-8) for the rest.
    let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{ascii}.csv\"; filename*=UTF-8''{}.csv", urlenc(&name)),
            ),
        ],
        csv,
    )
        .into_response())
}

// ----- cards --------------------------------------------------------------

#[derive(Default)]
struct CardFormData {
    deck: Option<i64>,
    /// Set when the picker says "New deck…": the name to create.
    new_deck: Option<String>,
    front: String,
    back: String,
    front_upload: Option<Vec<u8>>,
    back_upload: Option<Vec<u8>>,
    remove_front_image: bool,
    remove_back_image: bool,
    /// How to learn it item by item, if at all.
    multi: Option<Split>,
    /// An image was picked; it can't be refilled if the form comes back.
    had_upload: bool,
}

/// Reads a multipart form into field name → bytes. Card and import forms
/// both come through here, so upload problems read the same.
async fn read_multipart(mut mp: Multipart) -> Result<HashMap<String, Vec<u8>>, AppError> {
    let failed = |e: axum::extract::multipart::MultipartError| AppError::bad(format!("upload failed: {e}"));
    let mut fields = HashMap::new();
    while let Some(field) = mp.next_field().await.map_err(failed)? {
        let name = field.name().unwrap_or("").to_string();
        let data = field.bytes().await.map_err(failed)?;
        fields.insert(name, data.to_vec());
    }
    Ok(fields)
}

fn field_text(fields: &HashMap<String, Vec<u8>>, k: &str) -> String {
    fields.get(k).map(|v| String::from_utf8_lossy(v).to_string()).unwrap_or_default()
}

async fn read_card_form(mp: Multipart) -> Result<CardFormData, AppError> {
    let mut fields = read_multipart(mp).await?;
    let text = |k: &str| field_text(&fields, k);
    let deck = text("deck");
    let mut f = CardFormData {
        // "New deck…" in the picker means: create the deck named alongside.
        new_deck: (deck == NEW_DECK).then(|| text("new_deck")),
        deck: deck.parse().ok(),
        front: text("front"),
        back: text("back"),
        remove_front_image: fields.contains_key("remove_front_image"),
        remove_back_image: fields.contains_key("remove_back_image"),
        // "list" is what the form sent before multicards.
        multi: Split::parse(&text("multi")).or(fields.contains_key("list").then_some(Split::List)),
        ..Default::default()
    };
    f.front_upload = fields.remove("front_image").filter(|d| !d.is_empty());
    f.back_upload = fields.remove("back_image").filter(|d| !d.is_empty());
    f.had_upload = f.front_upload.is_some() || f.back_upload.is_some();
    for d in [&f.front_upload, &f.back_upload].into_iter().flatten() {
        media::check_size(d)?;
    }
    Ok(f)
}

/// The "Add" section: one click to a new card, a new deck, or an import.
fn add_tabs(current: &str) -> Markup {
    html! {
        nav.tabs aria-label="Add" {
            @for (key, href, label) in [("card", "/cards/new", "Card"), ("deck", "/decks/new", "Deck"), ("import", "/import", "Import")] {
                a href=(href) aria-current=[(key == current).then_some("page")] { (label) }
            }
        }
    }
}

/// Value of the deck picker's "New deck…" option.
const NEW_DECK: &str = "new";

/// One side of the card being written: its text, and an optional image
/// that can be picked, dropped or pasted.
fn face_field(side: &str, title: &str, placeholder: &str, text: &str, image: &Option<String>, autofocus: bool) -> Markup {
    html! {
        div.face-edit data-face=(side) {
            label.face-label for=(side) { (title) }
            textarea id=(side) name=(side) rows="3" placeholder=(placeholder) autofocus[autofocus] { (text) }
            @if let Some(img) = image {
                div.face-image.current {
                    img.thumb src=(format!("/media/{img}")) alt=(format!("{title} image"));
                    label.check { input type="checkbox" name=(format!("remove_{side}_image")) value="1"; " Remove image" }
                }
            }
            div.face-image.preview hidden {
                img alt="";
                button.link type="button" data-clear-image { "Remove" }
            }
            label.file.attach.icon-link {
                (icon(ICON_IMAGE, PAGE_ICON))
                span.attach-text { @if image.is_some() { "Replace image" } @else { "Add image" } }
                " " span.muted.optional { "(optional)" }
                input type="file" name=(format!("{side}_image")) accept="image/*";
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// What the card form shows: a new card's empty sides, a card being
/// edited, or what was sent when it comes back with a problem.
struct CardDraft<'a> {
    /// The card being edited; none for a new one.
    id: Option<i64>,
    deck: Option<i64>,
    new_deck: &'a str,
    front: &'a str,
    back: &'a str,
    front_image: &'a Option<String>,
    back_image: &'a Option<String>,
    multi: Option<Split>,
}

/// A hint from the coach while writing, with a link when it points at
/// another card.
#[derive(serde::Serialize)]
struct WriteHint {
    text: String,
    href: Option<String>,
}

/// What the coach says about a card being written: a long list, or a front
/// like one you already have.
async fn write_hints(app: &App, access: &Access, front: &str, back: &str, how: Option<Split>, except: Option<i64>) -> Result<Vec<WriteHint>, AppError> {
    let mut out: Vec<WriteHint> = coach::write_hints(back, how).into_iter().map(|text| WriteHint { text, href: None }).collect();
    if let Some(s) = app.similar_cards(access, front, except).await?.into_iter().next() {
        out.push(WriteHint {
            text: format!("You have a card like this: “{}”. Cards this alike get mixed up.", truncate(&s.front, 60)),
            href: Some(format!("/cards/{}", s.card_id)),
        });
    }
    Ok(out)
}

fn hint_lines(hints: &[WriteHint]) -> Markup {
    html! {
        @for h in hints {
            p.coach-hint {
                (h.text)
                @if let Some(href) = &h.href { " " a href=(href) { "Open it" } }
            }
        }
    }
}

fn card_form(action: &str, decks: &[crate::service::Deck], c: &CardDraft, submit: &str, allow_new_deck: bool, hints: &[WriteHint]) -> Markup {
    let new_selected = allow_new_deck && (decks.is_empty() || c.deck.is_none());
    html! {
        form.card-form method="post" action=(action) enctype="multipart/form-data" data-card=[c.id] {
            div.deck-line {
                label.deck-pick { span.deck-label { "Deck" }
                    span.select {
                        select name="deck" required data-new-deck-select=(NEW_DECK) {
                            @for d in decks { option value=(d.id) selected[!new_selected && Some(d.id) == c.deck] { (deck_plain(&d.name)) } }
                            @if allow_new_deck {
                                option value=(NEW_DECK) selected[new_selected] { "New deck…" }
                            }
                        }
                    }
                }
                @if allow_new_deck {
                    label.new-deck id="new-deck-field" {
                        span.deck-label { "New deck name" }
                        input type="text" name="new_deck" maxlength="200" placeholder="e.g. Spanish verbs" value=(c.new_deck);
                    }
                }
            }
            div.sheet {
                (face_field("front", "Front", "Question", c.front, c.front_image, true))
                hr.divider;
                (face_field("back", "Back", "Answer", c.back, c.back_image, false))
                // The ways the back can be split are offered; app.js keeps
                // them up to date while you write.
                @let splits = multi::splits(c.back);
                fieldset.line.multi-pick hidden[splits.is_empty() && c.multi.is_none()] {
                    legend { "Learn" }
                    span.choice {
                        label.check { input type="radio" name="multi" value="" checked[c.multi.is_none()]; " As one card" }
                        @for k in Split::ALL {
                            label.check data-split=(k.as_str()) hidden[!splits.contains(&k) && c.multi != Some(k)] {
                                input type="radio" name="multi" value=(k.as_str()) checked[c.multi == Some(k)];
                                " " (k.label())
                            }
                        }
                    }
                    p.hint-inline { "Item by item: you mark what you remembered, and each item gets its own schedule." }
                }
            }
            div.sheet.card-preview hidden {}
            div.write-hints aria-live="polite" { (hint_lines(hints)) }
            div.form-foot {
                p.muted.small {
                    "Text supports Markdown: **bold**, *italic*, lists, # headings, `code`."
                    " " button.link.preview-toggle type="button" hidden { "Preview" }
                }
                button.btn.primary.wide type="submit" { (submit) span.key.submit-key hidden { "Ctrl ⏎" } }
            }
        }
    }
}

#[derive(Deserialize)]
struct NewCardQuery {
    deck: Option<i64>,
}

fn card_new_view(user: &User, decks: &[Deck], c: &CardDraft, msg: &Option<String>, error: Option<&str>) -> Markup {
    layout_in(
        Nav::Add,
        "Add card",
        Some(user),
        html! {
            (add_tabs("card"))
            (flash(msg))
            h1 { "Add card" }
            (form_error(error))
            (card_form("/cards/new", decks, c, "Add card", true, &[]))
        },
    )
}

async fn card_new_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Query(q): Query<NewCardQuery>) -> WebResult {
    let access = user.access();
    let decks = app.decks(&access).await?;
    // The deck from the link, else the one last added to, else the first.
    let deck = match q.deck {
        Some(id) => Some(id),
        None => app.last_used_deck(&access).await?.or_else(|| decks.first().map(|d| d.id)),
    };
    let draft = CardDraft { id: None, deck, new_deck: "", front: "", back: "", front_image: &None, back_image: &None, multi: None };
    Ok(card_new_view(&user.0, &decks, &draft, &msg, None).into_response())
}

/// A picked image can't be put back into a form; say so with the problem.
fn with_image_hint(msg: String, f: &CardFormData) -> String {
    if f.had_upload { format!("{msg} Choose the image again.") } else { msg }
}

async fn card_create(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let mut f = read_card_form(mp).await?;
    let result = async {
        // A taken name adds to that deck instead of failing; the message says so.
        let mut msg = "Card added.".to_string();
        let deck = match (&f.new_deck, f.deck) {
            (Some(name), _) => {
                if name.trim().is_empty() {
                    return Err(AppError::bad("give the new deck a name"));
                }
                match app.deck_by_name(&access, name).await? {
                    Some(existing) => {
                        msg = format!("Card added to your existing deck {}.", deck_plain(&existing.name));
                        existing.id
                    }
                    None => {
                        msg = "Deck created and card added.".into();
                        app.create_deck(&access, name, "", None).await?.id
                    }
                }
            }
            (None, Some(id)) => id,
            (None, None) => return Err(AppError::bad("pick a deck")),
        };
        let front_image = match f.front_upload.take() {
            Some(d) => Some(app.store_image(&access, d).await?.id),
            None => None,
        };
        let back_image = match f.back_upload.take() {
            Some(d) => Some(app.store_image(&access, d).await?.id),
            None => None,
        };
        app.create_card(&access, deck, &f.front, &f.back, front_image, back_image, f.multi).await?;
        Ok((deck, msg))
    }
    .await;
    match result {
        Ok((deck, msg)) => Ok(redirect_msg(format!("/cards/new?deck={deck}"), &msg)),
        Err(e) => {
            let msg = with_image_hint(fixable(&e).ok_or(e)?, &f);
            let decks = app.decks(&access).await?;
            let draft = CardDraft {
                id: None,
                deck: if f.new_deck.is_some() { None } else { f.deck },
                new_deck: f.new_deck.as_deref().unwrap_or(""),
                front: &f.front,
                back: &f.back,
                front_image: &None,
                back_image: &None,
                multi: f.multi,
            };
            Ok(form_again(card_new_view(&user.0, &decks, &draft, &None, Some(&msg))))
        }
    }
}

/// Where the card page was opened from. Opened while studying, it leads
/// back to the session, and saving returns there too.
#[derive(Deserialize, Default)]
struct CardFrom {
    study: Option<u8>,
    deck: Option<i64>,
}

impl CardFrom {
    fn study(deck: Option<i64>) -> Self {
        CardFrom { study: Some(1), deck }
    }

    fn query(&self) -> String {
        match (self.study, self.deck) {
            (Some(1), Some(d)) => format!("?study=1&deck={d}"),
            (Some(1), None) => "?study=1".into(),
            _ => String::new(),
        }
    }

    fn study_url(&self) -> Option<String> {
        (self.study == Some(1)).then(|| study_url(self.deck))
    }
}

/// The card page; `sent` and `error` when an edit comes back with a problem.
async fn card_view(
    app: &App,
    user: &User,
    id: i64,
    from: &CardFrom,
    msg: &Option<String>,
    sent: Option<&CardFormData>,
    error: Option<&str>,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let card = app.card(&access, id).await?;
    let decks = app.decks(&access).await?;
    let history = app.card_history(&access, id).await?;
    let items = app.card_items(&card).await?;
    let flags = app.card_flags(&access, &card).await?;
    // The coach box already names a duplicate; the form only adds the list
    // hint, unless the box says the same.
    let hints: Vec<WriteHint> = coach::write_hints(&card.back, card.multi)
        .into_iter()
        .filter(|_| !flags.iter().any(|f| f.kind == coach::Kind::Split))
        .map(|text| WriteHint { text, href: None })
        .collect();
    let clock = user.clock();
    let params = app.deck_params(&access, card.deck_id).await?;
    let r = scheduler::retrievability(&card.sched(), &params, &clock);
    let draft = match sent {
        Some(f) => CardDraft {
            id: Some(card.id),
            deck: f.deck.or(Some(card.deck_id)),
            new_deck: "",
            front: &f.front,
            back: &f.back,
            front_image: if f.remove_front_image { &None } else { &card.front_image },
            back_image: if f.remove_back_image { &None } else { &card.back_image },
            multi: f.multi,
        },
        None => CardDraft {
            id: Some(card.id),
            deck: Some(card.deck_id),
            new_deck: "",
            front: &card.front,
            back: &card.back,
            front_image: &card.front_image,
            back_image: &card.back_image,
            multi: card.multi,
        },
    };
    Ok(layout_in(
        Nav::Decks,
        "Edit card",
        Some(user),
        html! {
            (flash(msg))
            @if let Some(url) = from.study_url() {
                (parent(&url, "Back to studying"))
            } @else if let Some(d) = decks.iter().find(|d| d.id == card.deck_id) {
                (parent(&format!("/decks/{}", d.id), &deck_plain(&d.name)))
            }
            h1 { "Edit card" }
            (form_error(error))
            (coach_box(&card, &flags))
            (card_form(&format!("/cards/{id}{}", from.query()), &decks, &draft, "Save", false, &hints))
            h2 { "Scheduling" }
            dl.kv {
                dt { "State" } dd { (state_label(card.card_state())) @if card.suspended { " (suspended)" } }
                @if card.card_state() != CardState::New {
                    dt { "Due" } dd { (capitalize(&format::when(&clock, card.due))) }
                }
                @if let Some(r) = r { dt { @if card.multi.is_some() { "Weakest item now" } @else { "Recall chance now" } } dd { (format::percent(Some(r as f64))) } }
            }
            @if !items.is_empty() {
                h2 { "Items" }
                ol.item-recall {
                    @for i in &items {
                        li {
                            span.text { (multi::title(&i.text)) }
                            span.chance {
                                @match scheduler::retrievability(&i.sched(), &params, &clock) {
                                    Some(r) => (format::percent(Some(r as f64))),
                                    None => "New",
                                }
                            }
                        }
                    }
                }
            }
            details.more {
                summary { "Scheduling details" }
                dl.kv {
                    @if let Some(s) = card.stability { dt { "Stability" } dd { (format!("{s:.1} days")) } }
                    @if let Some(d) = card.difficulty { dt { "Difficulty" } dd { (format!("{d:.1} / 10")) } }
                    dt { "Reviews / lapses" } dd { (card.reps) " / " (card.lapses) }
                    dt { "Added" } dd { (source_label(&card.created_by)) }
                }
            }
            form.actions method="post" action=(format!("/cards/{id}/{}", if card.suspended { "unsuspend" } else { "suspend" })) {
                button.btn type="submit" { @if card.suspended { "Unsuspend" } @else { "Suspend" } }
            }
            details.danger {
                summary { "Forget progress or delete this card…" }
                form method="post" action=(format!("/cards/{id}/delete")) {
                    p { "Forgetting makes it a new card again. Deleting removes it and its history." }
                    div.actions {
                        button.btn.danger type="submit" formaction=(format!("/cards/{id}/forget")) { "Forget progress" }
                        button.btn.danger type="submit" { "Delete card" }
                    }
                }
            }
            @if !history.is_empty() {
                h2 { "History" }
                table.history {
                    thead { tr { th { "When" } th { "Answer" } th { "Next" } } }
                    tbody {
                        @for h in history.iter().rev() {
                            tr {
                                td { (capitalize(&format::when(&clock, h["timestamp"].as_i64().unwrap_or(0)))) }
                                td {
                                    @if let (Some(asked), Some(recalled)) = (h["items_asked"].as_i64(), h["items_recalled"].as_i64()) {
                                        (recalled) " of " (asked) " remembered"
                                    } @else {
                                        (rating_label(h["rating"].as_str().unwrap_or("")))
                                    }
                                    @if h["source"] == "agent" { " (by an agent)" }
                                }
                                td { (h["next_interval"].as_str().unwrap_or("")) }
                            }
                        }
                    }
                }
            }
        },
    ))
}

async fn card_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(from): Query<CardFrom>,
    Flash(msg): Flash,
) -> WebResult {
    Ok(card_view(&app, &user.0, id, &from, &msg, None, None).await?.into_response())
}

async fn card_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(from): Query<CardFrom>,
    mp: Multipart,
) -> WebResult {
    let access = user.access();
    let mut f = read_card_form(mp).await?;
    let result = async {
        let front_image = match f.front_upload.take() {
            Some(d) => Some(Some(app.store_image(&access, d).await?.id)),
            None if f.remove_front_image => Some(None),
            None => None,
        };
        let back_image = match f.back_upload.take() {
            Some(d) => Some(Some(app.store_image(&access, d).await?.id)),
            None if f.remove_back_image => Some(None),
            None => None,
        };
        app.update_card(
            &access,
            id,
            CardInput {
                front: Some(f.front.clone()),
                back: Some(f.back.clone()),
                front_image,
                back_image,
                multi: Some(f.multi),
            },
        )
        .await?;
        if let Some(deck) = f.deck {
            if deck != app.card(&access, id).await?.deck_id {
                app.move_card(&access, id, deck).await?;
            }
        }
        Ok::<_, AppError>(())
    }
    .await;
    match result {
        Ok(()) => Ok(redirect_msg(from.study_url().unwrap_or_else(|| format!("/cards/{id}")), "Card saved.")),
        Err(e) => {
            let msg = with_image_hint(fixable(&e).ok_or(e)?, &f);
            Ok(form_again(card_view(&app, &user.0, id, &from, &None, Some(&f), Some(&msg)).await?))
        }
    }
}

async fn card_action(
    State(app): State<AppState>,
    user: WebUser,
    Path((id, action)): Path<(i64, String)>,
) -> WebResult {
    let access = user.access();
    or_back(format!("/cards/{id}"), async {
        let msg = match action.as_str() {
            "suspend" => {
                app.set_suspended(&access, id, true).await?;
                "Card suspended."
            }
            "unsuspend" => {
                app.set_suspended(&access, id, false).await?;
                "Card unsuspended."
            }
            "forget" => {
                app.reset_card(&access, id).await?;
                "Card progress forgotten; it's new again."
            }
            "delete" => {
                let card = app.card(&access, id).await?;
                app.delete_card(&access, id).await?;
                return Ok(redirect_msg(format!("/decks/{}", card.deck_id), "Card deleted."));
            }
            "learn-items" => {
                app.update_card(&access, id, CardInput { multi: Some(Some(Split::List)), ..Default::default() }).await?;
                "Now learned item by item: you mark what you remembered, and each item keeps its own schedule."
            }
            keep if keep.starts_with("keep-") => {
                let kind = coach::Kind::parse(&keep["keep-".len()..]).ok_or_else(|| AppError::bad("unknown action"))?;
                app.dismiss_flag(&access, id, kind).await?;
                "Kept as is."
            }
            _ => return Err(AppError::bad("unknown action")),
        };
        Ok(redirect_msg(format!("/cards/{id}"), msg))
    }
    .await)
}

// ----- the card coach -----------------------------------------------------

/// Where to read why a flag matters.
fn coach_lesson(kind: coach::Kind) -> &'static str {
    match kind {
        coach::Kind::Leech => "/learn/leeches",
        coach::Kind::Split => "/learn/good-cards",
        coach::Kind::Duplicate => "/learn/interference",
    }
}

/// Whether the back offers a split into cards.
fn splittable(card: &Card) -> bool {
    card.multi.is_some() || !multi::splits(&card.back).is_empty()
}

/// What the coach noticed about a card, each with what can be done. The
/// card is never changed until the user picks something.
fn coach_box(card: &Card, flags: &[coach::Flag]) -> Markup {
    let id = card.id;
    html! {
        @if !flags.is_empty() {
            section.coach aria-label="The coach noticed" {
                @for f in flags {
                    div.coach-flag {
                        p.coach-label { (f.kind.label()) }
                        p { (f.reason) " " a href=(coach_lesson(f.kind)) { "Why this matters" } }
                        @if f.kind == coach::Kind::Leech {
                            p.muted.small { "Rewrite it so it asks one clear thing, add an image or a memory hook, or split it. Suspending takes it out of your reviews." }
                        }
                        form.actions method="post" action=(format!("/cards/{id}/keep-{}", f.kind.as_str())) {
                            @if f.kind != coach::Kind::Duplicate && card.multi.is_none() && coach::list_items(&card.back) >= 2 {
                                button.btn type="submit" formaction=(format!("/cards/{id}/learn-items")) { "Learn item by item" }
                            }
                            @if f.kind != coach::Kind::Duplicate && splittable(card) {
                                a.btn href=(format!("/cards/{id}/split")) { "Split into cards…" }
                            }
                            @if let Some(other) = f.other_card {
                                a.btn href=(format!("/cards/{other}")) { "Open the other card" }
                            }
                            @if f.kind == coach::Kind::Leech && !card.suspended {
                                button.btn type="submit" formaction=(format!("/cards/{id}/suspend")) { "Suspend" }
                            }
                            button.btn type="submit" { "Keep as is" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
struct AttentionQuery {
    deck: Option<i64>,
}

/// Every card the coach flagged, in one deck or all of them.
async fn attention_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Query(q): Query<AttentionQuery>) -> WebResult {
    let access = user.access();
    let found = app.attention(&access, q.deck).await?;
    let deck = match q.deck {
        Some(id) => Some(app.deck(&access, id).await?),
        None => None,
    };
    Ok(layout_in(
        Nav::Decks,
        "Needs a look",
        Some(&user.0),
        html! {
            (flash(&msg))
            @if let Some(d) = &deck { (parent(&format!("/decks/{}", d.id), &deck_plain(&d.name))) }
            h1 { "Needs a look" }
            @if found.is_empty() {
                p { "Nothing right now. The coach looks for cards you keep forgetting, cards that ask for several things at once, and cards that look alike." }
            } @else {
                p.muted {
                    "The coach noticed " (count(found.len() as i64, "card")) ". Open one to see why and what you can do. Nothing changes unless you pick something. "
                    a href="/learn/good-cards" { "What makes a good card" }
                }
                ul.list.cards {
                    @for a in &found {
                        li {
                            a.row href=(format!("/cards/{}", a.card.id)) {
                                span.grow {
                                    @if a.card.front.is_empty() { "[image]" } @else { (truncate(&a.card.front, 80)) }
                                    br;
                                    span.muted.small { (a.flags.iter().map(|f| f.kind.label()).collect::<Vec<_>>().join(" · ")) }
                                }
                                span.chev aria-hidden="true" { "›" }
                            }
                        }
                    }
                }
            }
        },
    )
    .into_response())
}

/// The cards a split starts from: one per item of the back, each with the
/// original front. Without items, the card itself and an empty one.
fn split_drafts(card: &Card, items: &[multi::CardItem]) -> Vec<(String, String)> {
    let texts: Vec<String> = if !items.is_empty() {
        items.iter().map(|i| i.text.clone()).collect()
    } else {
        multi::splits(&card.back)
            .first()
            .and_then(|&how| multi::items(&card.back, how))
            .unwrap_or_default()
    };
    if texts.len() >= 2 {
        texts.into_iter().map(|t| (card.front.clone(), t)).collect()
    } else {
        vec![(card.front.clone(), card.back.clone()), (String::new(), String::new())]
    }
}

#[derive(Deserialize, Default)]
struct SplitQuery {
    /// How many cards to show, when more than the back gives.
    n: Option<usize>,
}

fn split_view(user: &User, card: &Card, drafts: &[(String, String)], keep: bool, error: Option<&str>) -> Markup {
    let id = card.id;
    layout_in(
        Nav::Decks,
        "Split card",
        Some(user),
        html! {
            (parent(&format!("/cards/{id}"), "Back to the card"))
            h1 { "Split into cards" }
            p.muted { "One thing per card is easier to remember. Each new card starts fresh. Change the fronts so each asks for exactly its answer." }
            (form_error(error))
            form.split-form method="post" action=(format!("/cards/{id}/split")) {
                @for (n, (front, back)) in drafts.iter().enumerate() {
                    div.sheet {
                        div.face-edit data-face="front" {
                            label.face-label for=(format!("front_{n}")) { "Card " (n + 1) ": front" }
                            textarea id=(format!("front_{n}")) name=(format!("front_{n}")) rows="2" { (front) }
                        }
                        hr.divider;
                        div.face-edit data-face="back" {
                            label.face-label for=(format!("back_{n}")) { "Back" }
                            textarea id=(format!("back_{n}")) name=(format!("back_{n}")) rows="2" { (back) }
                        }
                    }
                }
                p.small { a href=(format!("/cards/{id}/split?n={}", drafts.len() + 1)) { "One more card" } " · Leave a card empty to skip it." }
                label.check { input type="checkbox" name="keep" value="1" checked[keep]; " Keep the original card, suspended" }
                div.form-foot { button.btn.primary.wide type="submit" { "Make " (drafts.len()) " cards" } }
            }
        },
    )
}

async fn split_page(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>, Query(q): Query<SplitQuery>) -> WebResult {
    let access = user.access();
    let card = app.card(&access, id).await?;
    let items = app.card_items(&card).await?;
    let mut drafts = split_drafts(&card, &items);
    if let Some(n) = q.n.filter(|n| *n > drafts.len()) {
        drafts.resize(n.min(50), (String::new(), String::new()));
    }
    Ok(split_view(&user.0, &card, &drafts, false, None).into_response())
}

async fn split_submit(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>, Form(f): Form<HashMap<String, String>>) -> WebResult {
    let access = user.access();
    let card = app.card(&access, id).await?;
    let mut drafts = Vec::new();
    for n in 0..50 {
        match (f.get(&format!("front_{n}")), f.get(&format!("back_{n}"))) {
            (None, None) => break,
            (front, back) => drafts.push((
                front.cloned().unwrap_or_default(),
                back.cloned().unwrap_or_default(),
            )),
        }
    }
    let keep = f.contains_key("keep");
    match app.split_card(&access, id, drafts.clone(), keep).await {
        Ok(cards) => Ok(redirect_msg(
            format!("/decks/{}", card.deck_id),
            &format!("Split into {}.", count(cards.len() as i64, "card")),
        )),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            Ok(form_again(split_view(&user.0, &card, &drafts, keep, Some(&msg))))
        }
    }
}

// ----- presets ------------------------------------------------------------

async fn presets_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout_in(
        Nav::Settings,
        "Presets",
        Some(&user.0),
        html! {
            (flash(&msg))
            (parent("/settings", "Settings"))
            h1 { "Presets" }
            p {
                "A preset sets how much you want to remember (desired retention) and your daily limits. "
                "Scheduling uses FSRS-6, which predicts when you would forget each card."
            }
            ul.list {
                @for p in &presets {
                    li {
                        a.row href=(format!("/presets/{}", p.id)) {
                            span.grow {
                                (p.name) br;
                                span.muted.small { (p.description) }
                            }
                            span.counts { (format!("{:.0}%", p.desired_retention * 100.0)) }
                        }
                    }
                }
            }
            h2 { "New preset" }
            form method="get" action="/presets/new" {
                div.sheet.form {
                    label.line { "Start from"
                        span.select {
                            select name="template" {
                                @for t in presets::TEMPLATES { option value=(t.key) { (t.name) } }
                            }
                        }
                    }
                }
                div.form-foot.end { button.btn type="submit" { "New preset" } }
            }
            h2 { "How to choose" }
            ul.prose {
                li { strong { "Desired retention" } " is the chance you still remember a card when it comes up. 90% suits most people. Higher costs a lot more time: 95% needs about twice the reviews of 90%. Below 80% you relearn too much." }
                li { strong { "Learning steps" } " are short waits for new cards, such as 10m. Keep them under a day; longer gaps are FSRS's job." }
                li { strong { "Daily limits" } ": each new card leads to about 10 reviews in its first month, so keep the review limit at least 10 times the new card limit." }
                li { "After about 1,000 reviews, run " a href="/settings#fsrs" { "Optimize" } " in settings to fit FSRS to your own memory." }
            }
        },
    )
    .into_response())
}

/// A preset written as a sheet: its name large, a description, then
/// its numbers as lines.
fn preset_sheet(p: &crate::service::Preset, used_by: &[String]) -> Markup {
    html! {
        div.sheet.form {
            label.title { span.face-label { "Name" } input type="text" name="name" required value=(p.name); }
            label.title { span.face-label { "Description " span.muted.optional { "(optional)" } }
                textarea name="description" rows="2" placeholder="When to use it" { (p.description) }
            }
            @if !used_by.is_empty() { p.hint.muted.small { "Used by " (used_by.iter().map(|d| deck_plain(d)).collect::<Vec<_>>().join(", ")) } }
            hr.divider;
            label.line { "How much to remember"
                span.field { input type="number" name="desired_retention" min="70" max="99" step="1" required value=(format!("{:.0}", p.desired_retention * 100.0)); span.unit { "%" } }
            }
            label.line { "New cards per day" input type="number" name="new_per_day" min="0" required value=(p.new_per_day); }
            label.line { "Reviews per day, at most" input type="number" name="reviews_per_day" min="0" required value=(p.reviews_per_day); }
            hr.divider;
            label.line { "Time to answer"
                span.select {
                    select name="time_limit" {
                        @for secs in TIME_LIMITS {
                            option value=(secs) selected[*secs == p.time_limit] { (time_limit_words(*secs)) }
                        }
                        @if !TIME_LIMITS.contains(&p.time_limit) {
                            option value=(p.time_limit) selected { (time_limit_words(p.time_limit)) }
                        }
                    }
                }
            }
            p.hint.muted.small { "Only for cards you have seen before. When time runs out the card counts as forgotten. You can add 10 seconds once per card." }
            label.check { input type="checkbox" name="typed_answers" value="1" checked[p.typed_answers]; " Type the answer before it's shown" }
            // The hard values sit under a fold; most people never need them.
            details.more {
                summary { "Learning steps and longest gap" }
                label.line { span { "Learning steps " span.muted { "(e.g. 10m or 1m 10m)" } }
                    input type="text" name="learning_steps" value=(p.learning_steps);
                }
                label.line { "Relearning steps" input type="text" name="relearning_steps" value=(p.relearning_steps); }
                label.line { "Longest gap between reviews"
                    span.field { input type="number" name="max_interval" min="1" required value=(p.max_interval); span.unit { "days" } }
                }
            }
        }
    }
}

/// The time limits offered for a preset, in seconds; 0 is none.
const TIME_LIMITS: &[i64] = &[0, 5, 8, 10, 15, 20, 30, 45, 60];

fn time_limit_words(secs: i64) -> String {
    if secs == 0 { "No limit".into() } else { format!("{secs} seconds") }
}

#[derive(Deserialize)]
struct TemplateQuery {
    template: Option<String>,
}

/// A preset as a template would make it, not saved yet.
fn preset_draft(t: &presets::PresetTemplate, user_id: i64) -> crate::service::Preset {
    crate::service::Preset {
        id: 0,
        user_id,
        name: format!("{} (custom)", t.name),
        description: t.description.to_string(),
        desired_retention: t.desired_retention,
        new_per_day: t.new_per_day,
        reviews_per_day: t.reviews_per_day,
        learning_steps: t.learning_steps.to_string(),
        relearning_steps: t.relearning_steps.to_string(),
        max_interval: t.max_interval,
        created_at: 0,
        template: None,
        fsrs_params: "[]".into(),
        fsrs_params_updated_at: None,
        time_limit: 0,
        typed_answers: false,
    }
}

/// A new preset, filled in from a template. Nothing is saved until the
/// form is sent, like a new deck.
fn preset_new_view(user: &User, template: &str, draft: &crate::service::Preset, error: Option<&str>) -> Markup {
    layout_in(
        Nav::Settings,
        "New preset",
        Some(user),
        html! {
            (parent("/presets", "Presets"))
            h1 { "New preset" }
            (form_error(error))
            form method="post" action="/presets/new" {
                input type="hidden" name="template" value=(template);
                (preset_sheet(draft, &[]))
                div.form-foot.end { button.btn.primary type="submit" { "Create preset" } }
            }
        },
    )
}

async fn preset_new_page(user: WebUser, Query(q): Query<TemplateQuery>) -> WebResult {
    let t = presets::template(q.template.as_deref().unwrap_or("standard")).ok_or(AppError::NotFound)?;
    Ok(preset_new_view(&user.0, t.key, &preset_draft(t, user.0.id), None).into_response())
}

async fn preset_create(State(app): State<AppState>, user: WebUser, Form(f): Form<PresetForm>) -> WebResult {
    let draft = f.draft(0, user.0.id);
    let template = f.template.clone();
    match app.create_preset(&user.access(), template.as_deref(), f.into_input()).await {
        Ok(p) => Ok(redirect_msg(format!("/presets/{}", p.id), "Preset created.")),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            Ok(form_again(preset_new_view(&user.0, template.as_deref().unwrap_or("standard"), &draft, Some(&msg))))
        }
    }
}

/// The preset page; `draft` and `error` when a sent form comes back.
#[allow(clippy::too_many_arguments)]
async fn preset_view(
    app: &App,
    user: &User,
    id: i64,
    msg: &Option<String>,
    draft: Option<&crate::service::Preset>,
    error: Option<&str>,
    days: usize,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let p = app.preset(&access, id).await?;
    let workload = app.workload(&access, id, days).await?;
    let decks: Vec<String> = app
        .decks(&access)
        .await?
        .into_iter()
        .filter(|d| d.preset_id == id)
        .map(|d| d.name)
        .collect();
    Ok(layout_in(
        Nav::Settings,
        "Edit preset",
        Some(user),
        html! {
            (flash(msg))
            (parent("/presets", "Presets"))
            h1 { "Edit preset" }
            (form_error(error))
            form method="post" action=(format!("/presets/{id}")) {
                (preset_sheet(draft.unwrap_or(&p), &decks))
                div.form-foot.end { button.btn.primary type="submit" { "Save" } }
            }
            h2 id="cost" { "What it costs" }
            p.muted.small {
                "A simulation of the next "
                @for (n, (d, label)) in [(30, "30 days"), (90, "90 days"), (365, "year")].into_iter().enumerate() {
                    @if n > 0 { " · " }
                    @if d == days { (label) } @else { a href=(format!("/presets/{id}?days={d}#cost")) { (label) } }
                }
                ", with your memory model and "
                @if p.new_per_day == 0 { "no new cards" } @else { (count(p.new_per_day, "new card")) " a day" }
                ". Higher retention means more reviews but more remembered. "
                a href="/learn/desired-retention" { "How to choose" }
            }
            table.workload {
                thead { tr { th { "Remember" } th.num { "Minutes a day" } th.num { "Reviews a day" } th.num { "Known at the end" } } }
                tbody {
                    @for w in &workload {
                        tr.current[w.current] {
                            th scope="row" { (format!("{:.0}%", w.retention * 100.0)) @if w.current { span.muted.small { " this preset" } } }
                            td.num { (format!("{:.0}", w.minutes_per_day.max(0.0))) }
                            td.num { (format::number(w.reviews_per_day.round() as i64)) }
                            td.num { (format::number(w.remembered.round() as i64)) }
                        }
                    }
                }
            }
            p.muted.small {
                @if p.has_own_params() {
                    "This preset has FSRS parameters of its own, fitted to its reviews"
                    @if let Some(t) = p.fsrs_params_updated_at { " " (format::when(&user.clock(), t)) }
                    "."
                } @else {
                    "This preset uses your FSRS parameters. Once its decks have about 400 reviews, optimizing fits it on its own."
                }
            }
            @if decks.is_empty() {
                details.danger {
                    summary { "Delete this preset…" }
                    form method="post" action=(format!("/presets/{id}/delete")) {
                        p { "No deck uses it, so nothing else changes." }
                        button.btn.danger type="submit" { "Delete preset" }
                    }
                }
            }
        },
    ))
}

#[derive(Deserialize, Default)]
struct PresetQuery {
    days: Option<usize>,
}

async fn preset_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
    Query(q): Query<PresetQuery>,
) -> WebResult {
    let days = q.days.filter(|d| [30, 90, 365].contains(d)).unwrap_or(30);
    Ok(preset_view(&app, &user.0, id, &msg, None, None, days).await?.into_response())
}

#[derive(Deserialize)]
struct PresetForm {
    #[serde(default)]
    template: Option<String>,
    name: String,
    description: String,
    desired_retention: f64,
    new_per_day: i64,
    reviews_per_day: i64,
    learning_steps: String,
    relearning_steps: String,
    max_interval: i64,
    #[serde(default)]
    time_limit: i64,
    typed_answers: Option<String>,
}

impl PresetForm {
    fn into_input(self) -> PresetInput {
        PresetInput {
            name: Some(self.name),
            description: Some(self.description),
            // Typed as a percent, stored as a probability.
            desired_retention: Some(self.desired_retention / 100.0),
            new_per_day: Some(self.new_per_day),
            reviews_per_day: Some(self.reviews_per_day),
            learning_steps: Some(self.learning_steps),
            relearning_steps: Some(self.relearning_steps),
            max_interval: Some(self.max_interval),
            time_limit: Some(self.time_limit),
            typed_answers: Some(self.typed_answers.is_some()),
        }
    }

    /// What was typed, to show the form again.
    fn draft(&self, id: i64, user_id: i64) -> crate::service::Preset {
        crate::service::Preset {
            id,
            user_id,
            name: self.name.clone(),
            description: self.description.clone(),
            desired_retention: self.desired_retention / 100.0,
            new_per_day: self.new_per_day,
            reviews_per_day: self.reviews_per_day,
            learning_steps: self.learning_steps.clone(),
            relearning_steps: self.relearning_steps.clone(),
            max_interval: self.max_interval,
            created_at: 0,
            template: None,
            fsrs_params: "[]".into(),
            fsrs_params_updated_at: None,
            time_limit: self.time_limit,
            typed_answers: self.typed_answers.is_some(),
        }
    }
}

async fn preset_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<PresetForm>,
) -> WebResult {
    let draft = f.draft(id, user.0.id);
    match app.update_preset(&user.access(), id, f.into_input()).await {
        Ok(_) => Ok(redirect_msg(format!("/presets/{id}"), "Preset saved.")),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            Ok(form_again(preset_view(&app, &user.0, id, &None, Some(&draft), Some(&msg), 30).await?))
        }
    }
}

async fn preset_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(format!("/presets/{id}"), async {
        app.delete_preset(&user.access(), id).await?;
        Ok(redirect_msg("/presets", "Preset deleted."))
    }
    .await)
}

// ----- settings -----------------------------------------------------------

async fn settings_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let u = &user.0;
    let reviews = app.optimizable_reviews(&access).await?;
    let custom = !u.params().is_empty();
    Ok(layout_in(
        Nav::Settings,
        "Settings",
        Some(u),
        html! {
            (flash(&msg))
            h1 { "Settings" }
            p.muted { "Logged in as " strong { (u.handle()) } }
            ul.list.links {
                @for (href, name, sub) in [
                    ("/presets", "Presets", "How much to remember, daily limits"),
                    ("/access", "Agent access", "Connect Claude and other agents"),
                    ("/import", "Import", "From Anki or a CSV file"),
                    ("/learn", "Learn", "How memory and scheduling work, and how to write good cards"),
                ] {
                    li { a.row href=(href) { span.grow { span { (name) } span.sub.muted.small { (sub) } } span.chev aria-hidden="true" { "›" } } }
                }
            }
            h2 { "Preferences" }
            form method="post" action="/settings" {
                div.sheet.form {
                    label.line { "Timezone"
                        input type="text" name="timezone" value=(u.timezone) list="tzs" required autocapitalize="none";
                        datalist id="tzs" { @for tz in chrono_tz::TZ_VARIANTS.iter() { option value=(tz.name()); } }
                    }
                    label.line { "New day starts at" span.field { input type="number" name="rollover_hour" min="0" max="23" value=(u.rollover_hour); span.unit { ":00" } } }
                    label.line { "Text size" span.field { input type="number" name="font_scale" min="80" max="200" step="10" value=(u.font_scale); span.unit { "%" } } }
                    label.check { input type="checkbox" name="four_buttons" value="1" checked[u.four_buttons]; " Show Hard and Easy buttons" }
                    p.hint.muted.small { "Hard means you remembered, with effort. If you forgot, it's Again; FSRS gets that wrong otherwise. " a href="/learn/two-buttons" { "Two buttons or four?" } }
                    label.check { input type="checkbox" name="optimize_include_agent" value="1" checked[u.optimize_include_agent]; " Use reviews done through agents when optimizing" }
                    fieldset.line {
                        legend { "Look on desktop computers" }
                        span.choice {
                            label.check { input type="radio" name="desktop_look" value="calm" checked[u.desktop_look != "simple"]; " Calm" }
                            label.check { input type="radio" name="desktop_look" value="simple" checked[u.desktop_look == "simple"]; " Simple" }
                        }
                    }
                    p.hint.muted.small { "Phones, e-readers and browsers without JavaScript always use the simple, high-contrast look." }
                }
                div.form-foot.end { button.btn.primary type="submit" { "Save" } }
            }
            h2 id="studying" { "Studying" }
            form method="post" action="/settings/study" {
                div.sheet.form {
                    div.line { "Quiet hours"
                        span.field.quiet {
                            span.select { select name="quiet_from" aria-label="Quiet from" {
                                option value="" selected[u.quiet_from.is_none()] { "Off" }
                                @for h in 0..24 { option value=(h) selected[u.quiet_from == Some(h)] { (h) ":00" } }
                            } }
                            span.unit { "to" }
                            span.select { select name="quiet_to" aria-label="Quiet until" {
                                option value="" selected[u.quiet_to.is_none()] { "Off" }
                                @for h in 0..24 { option value=(h) selected[u.quiet_to == Some(h)] { (h) ":00" } }
                            } }
                        }
                    }
                    p.hint.muted.small { "No cards then, on the website or through agents. You can still study anyway." }
                    label.line { "Suggest a break after"
                        span.select { select name="session_minutes" {
                            @for m in study::SESSION_LENGTHS {
                                option value=(m) selected[*m == u.session_minutes] { (if *m == 0 { "Never".to_string() } else { study::minutes_words(*m) }) }
                            }
                            @if !study::SESSION_LENGTHS.contains(&u.session_minutes) {
                                option value=(u.session_minutes) selected { (study::minutes_words(u.session_minutes)) }
                            }
                        } }
                    }
                    label.check { input type="checkbox" name="confidence_check" value="1" checked[u.confidence_check]; " Ask whether I'm sure before the answer" }
                    p.hint.muted.small { "It doesn't change the schedule. Stats show how often you were right when you felt sure." }
                    label.check { input type="checkbox" name="catch_up" value="1" checked[u.catch_up]; " Catch-up mode" }
                    label.line { "Reviews a day while catching up" input type="number" name="catch_up_reviews" min="1" max="9999" value=(u.catch_up_reviews); }
                    p.hint.muted.small { "For after time away: no new cards, and the cards you most likely still know come first, so less is lost. " a href="/learn/backlog" { "More" } }
                }
                div.form-foot.end { button.btn.primary type="submit" { "Save" } }
            }
            h2 id="fsrs" { "FSRS" }
            div.sheet.form {
                div.line { "Parameters"
                    span.value {
                        @if custom {
                            "Fitted to your reviews"
                            @if let Some(t) = u.fsrs_params_updated_at { span.muted.small { " · " (format::when(&u.clock(), t)) } }
                        } @else {
                            "FSRS-6 defaults"
                        }
                    }
                }
                div.line { "Reviews so far" span.value { (format::number(reviews)) } }
                @if let (Some(note), Some(t)) = (&u.optimize_note, u.optimized_at) {
                    div.line { "Last optimized" span.value { (capitalize(&format::when(&u.clock(), t))) } }
                    p.hint.muted.small { (note) }
                }
                form method="post" action="/settings/auto-optimize" data-autosubmit {
                    label.check {
                        input type="checkbox" name="on" value="1" checked[u.auto_optimize];
                        " Optimize every month by itself"
                    }
                    noscript { button.btn type="submit" { "Save" } }
                }
                p.hint.muted.small {
                    "Optimizing fits FSRS to your own memory. It helps once you have a few hundred reviews. New parameters are only kept when they predict your reviews better. "
                    a href="/learn/optimizing" { "More" }
                }
            }
            div.form-foot.end {
                form method="post" action="/settings/optimize" {
                    button.btn type="submit" { "Optimize now" }
                }
            }
            @if custom {
                details.danger {
                    summary { "Go back to the default parameters…" }
                    form method="post" action="/settings/reset-params" {
                        p { "Your fitted parameters are dropped. You can optimize again any time." }
                        button.btn.danger type="submit" { "Use defaults" }
                    }
                }
            }
            h2 id="password" { "Change password" }
            form method="post" action="/settings/password" {
                div.sheet.form {
                    label.line { "Current password" input type="password" name="current" required autocomplete="current-password"; }
                    label.line { span { "New password " span.muted { "(8 or more characters)" } } input type="password" name="new" required minlength="8" autocomplete="new-password"; }
                }
                div.form-foot.end { button.btn type="submit" { "Change password" } }
            }
            form.logout method="post" action="/logout" { button.btn type="submit" { "Log out" } }
            p.muted.small.build { "Build " (env!("AINKI_BUILD")) }
        },
    )
    .into_response())
}

#[derive(Deserialize)]
struct SettingsForm {
    timezone: String,
    /// Numbers as typed: a cleared field keeps the current value instead of
    /// failing the whole form.
    #[serde(default)]
    rollover_hour: String,
    #[serde(default)]
    font_scale: String,
    four_buttons: Option<String>,
    optimize_include_agent: Option<String>,
    desktop_look: Option<String>,
}

async fn settings_update(State(app): State<AppState>, user: WebUser, Form(f): Form<SettingsForm>) -> WebResult {
    or_back("/settings", async {
        app.update_settings(
            user.0.id,
            f.timezone.trim(),
            f.rollover_hour.trim().parse().unwrap_or(user.0.rollover_hour),
            f.four_buttons.is_some(),
            f.font_scale.trim().parse().unwrap_or(user.0.font_scale),
            f.optimize_include_agent.is_some(),
            f.desktop_look.as_deref().unwrap_or("calm"),
        )
        .await?;
        Ok(redirect_msg("/settings", "Settings saved."))
    }
    .await)
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    new: String,
}

async fn password_update(
    State(app): State<AppState>,
    user: WebUser,
    jar: CookieJar,
    Form(f): Form<PasswordForm>,
) -> WebResult {
    // Passwords are never written back into a form; the problem shows
    // as a message above it.
    if let Err(e) = app.change_password(&user.0, &f.current, &f.new).await {
        return or_back("/settings#password", Err(e));
    }
    let token = app.create_session(user.0.id).await?;
    Ok((
        jar.add(session_cookie(&app, token)).add(flash_cookie("Password changed.")),
        Redirect::to("/settings"),
    )
        .into_response())
}

/// The last seven days in a sentence or two, and one thing worth doing.
fn week_note(w: &insight::Week) -> Markup {
    let pct = |v: f64| format!("{:.0}%", v * 100.0);
    html! {
        section.week aria-label="This week" {
            h2 { "This week" }
            @if w.reviews == 0 {
                p { "No reviews in the last seven days." }
            } @else {
                p {
                    (count(w.reviews, "review")) " on " (count(w.days_studied, "day")) ", "
                    (format::minutes(w.minutes))
                    @if w.new_cards > 0 { ", " (count(w.new_cards, "new card")) }
                    "."
                    @if let (Some(r), Some(t)) = (w.retention, w.target) {
                        @if w.retention_sample >= insight::MIN_SAMPLE {
                            " You remembered " (pct(r)) " of the cards due; you aim for " (pct(t)) "."
                        }
                    }
                }
            }
            @match w.tip {
                insight::Tip::Attention => p.tip { span { (count(w.attention, "card")) " keep slipping or ask too much at once. Fixing them saves more time than reviewing them. " a href="/attention" { "Have a look" } } },
                insight::Tip::BelowTarget => p.tip { span { "You forgot more than you aim for. Fewer new cards for a while, or optimizing FSRS, usually brings it back. " a href="/learn/desired-retention" { "Why" } } },
                insight::Tip::AboveTarget => p.tip { span { "You remember more than you aim for. Optimizing lets FSRS space your cards further apart. " a href="/settings#fsrs" { "Optimize" } } },
                insight::Tip::Spread => p.tip { span { "A little every day works better than a lot on a few days. " a href="/learn/spacing" { "Why" } } },
                insight::Tip::Nothing => {},
            }
        }
    }
}

#[derive(Deserialize)]
struct StudySettingsForm {
    #[serde(default)]
    quiet_from: String,
    #[serde(default)]
    quiet_to: String,
    #[serde(default)]
    session_minutes: String,
    confidence_check: Option<String>,
    catch_up: Option<String>,
    #[serde(default)]
    catch_up_reviews: String,
}

async fn study_settings_update(State(app): State<AppState>, user: WebUser, Form(f): Form<StudySettingsForm>) -> WebResult {
    let u = &user.0;
    let quiet = match (f.quiet_from.trim().parse::<i64>(), f.quiet_to.trim().parse::<i64>()) {
        (Ok(a), Ok(b)) => Some((a, b)),
        _ => None,
    };
    or_back("/settings#studying", async {
        app.update_study_settings(
            u.id,
            quiet,
            f.session_minutes.trim().parse().unwrap_or(u.session_minutes),
            f.catch_up.is_some(),
            f.catch_up_reviews.trim().parse().unwrap_or(u.catch_up_reviews),
            f.confidence_check.is_some(),
        )
        .await?;
        Ok(redirect_msg("/settings#studying", "Settings saved."))
    }
    .await)
}

#[derive(Deserialize)]
struct CatchUpForm {
    on: Option<String>,
}

async fn catch_up_update(State(app): State<AppState>, user: WebUser, Form(f): Form<CatchUpForm>) -> WebResult {
    app.set_catch_up(user.0.id, f.on.is_some()).await?;
    Ok(redirect_msg(
        "/",
        if f.on.is_some() { "Catch-up mode is on. No new cards until you have caught up." } else { "Catch-up mode is off." },
    ))
}

#[derive(Deserialize)]
struct AutoOptimizeForm {
    on: Option<String>,
}

async fn auto_optimize(State(app): State<AppState>, user: WebUser, Form(f): Form<AutoOptimizeForm>) -> WebResult {
    app.set_auto_optimize(user.0.id, f.on.is_some()).await?;
    Ok(redirect_msg(
        "/settings#fsrs",
        if f.on.is_some() { "FSRS will be optimized every month." } else { "Automatic optimization is off." },
    ))
}

async fn optimize(State(app): State<AppState>, user: WebUser) -> WebResult {
    let result = app.optimize_parameters(&user.access()).await;
    let msg = match result {
        Ok(v) => v["note"].as_str().unwrap_or("Done.").to_string(),
        Err(AppError::BadRequest(m)) => capitalize(&m),
        Err(e) => return Err(e.into()),
    };
    Ok(redirect_msg("/settings#fsrs", &msg))
}

async fn reset_params(State(app): State<AppState>, user: WebUser) -> WebResult {
    or_back("/settings#fsrs", async {
        app.reset_parameters(&user.access()).await?;
        Ok(redirect_msg("/settings#fsrs", "FSRS parameters reset to the defaults."))
    }
    .await)
}

// ----- tokens -------------------------------------------------------------

// What an agent may do, worded once for the token form and the OAuth
// consent page.
pub const PERM_WRITE: &str = "Add, edit and delete cards and decks";
pub const PERM_REVIEW: &str = "Quiz you and record your answers";
pub const PERM_ALL_DECKS: &str = "All decks (also lets it create decks, change presets and share)";
pub const PERM_SOME_DECKS: &str = "Only these:";

fn mcp_url(app: &AppState, headers: &HeaderMap) -> String {
    format!("{}/mcp", oauth::base_url(app, headers))
}

async fn access_page(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Flash(msg): Flash,
) -> WebResult {
    let access = user.access();
    let tokens = app.list_tokens(user.0.id).await?;
    let decks = app.decks(&access).await?;
    let clock = user.0.clock();
    let url = mcp_url(&app, &headers);
    let https = url.starts_with("https://");
    Ok(layout_in(
        Nav::Settings,
        "Agent access",
        Some(&user.0),
        html! {
            (flash(&msg))
            (parent("/settings", "Settings"))
            h1 { "Agent access" }
            h2 { "Connect Claude" }
            ol.prose {
                li { "In Claude, open " strong { "Settings → Connectors" } " and choose " strong { "Add custom connector" } "." }
                li { "Name it AiNKI and paste this URL:" pre.copy { (url) } }
                li { "Click " strong { "Connect" } ", log in here if asked, and click " strong { "Allow" } "." }
            }
            @if !https {
                p.notice {
                    "This address isn't HTTPS. Claude on the web and in the apps needs AiNKI on a public HTTPS address; "
                    "set AINKI_PUBLIC_URL to it. Claude Code on this computer can use it as is: "
                    code { "claude mcp add --transport http ainki " (url) }
                }
            }
            p.muted.small { "Any MCP client that supports OAuth connects the same way. Agents can never change your password or settings." }
            h2 { "Connected" }
            @if tokens.is_empty() { p.muted { "Nothing connected yet." } }
            ul.list hidden[tokens.is_empty()] {
                @for (t, deck_names) in &tokens {
                    li.item {
                        div.grow {
                            strong { (t.name) } " "
                            @if t.oauth_client_id.is_some() { span.small { "(connected app)" } } @else { code { (t.prefix) "…" } }
                            br;
                            span.small {
                                (if t.can_write { "read & write" } else { "read only" })
                                (if t.can_review { " · can review" } else { "" })
                                " · "
                                @if t.all_decks { "all decks" } @else { (deck_names.iter().map(|n| deck_plain(n)).collect::<Vec<_>>().join(", ")) }
                            }
                            br;
                            span.muted.small {
                                "Last used "
                                (t.last_used_at.map(|ts| format::when(&clock, ts)).unwrap_or_else(|| "never".into()))
                            }
                        }
                        form method="post" action=(format!("/access/{}/revoke", t.id)) {
                            button.btn.danger type="submit" {
                                (if t.oauth_client_id.is_some() { "Disconnect" } else { "Revoke" })
                            }
                        }
                    }
                }
            }
            details {
                summary { "Create a token by hand" }
                p.small { "For agents without OAuth support: they send the token as " code { "Authorization: Bearer <token>" } "." }
                form method="post" action="/access/new" {
                    label { "Name" input type="text" name="name" required placeholder="e.g. Claude"; }
                    label.check { input type="checkbox" name="can_write" value="1" checked; " " (PERM_WRITE) }
                    label.check { input type="checkbox" name="can_review" value="1" checked; " " (PERM_REVIEW) }
                    fieldset {
                        legend { "Decks" }
                        label.check { input type="radio" name="deck_scope" value="all" checked; " " (PERM_ALL_DECKS) }
                        label.check { input type="radio" name="deck_scope" value="some"; " " (PERM_SOME_DECKS) }
                        @for d in &decks {
                            label.check.indent { input type="checkbox" name="decks" value=(d.id); " " (deck_plain(&d.name)) }
                        }
                    }
                    button.btn type="submit" { "Create token" }
                }
            }
        },
    )
    .into_response())
}

#[derive(Deserialize)]
struct TokenForm {
    name: String,
    can_write: Option<String>,
    can_review: Option<String>,
    deck_scope: String,
    #[serde(default)]
    decks: Vec<i64>,
}

async fn token_create(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> WebResult {
    let decks = if f.deck_scope == "all" { None } else { Some(f.decks) };
    let (token, raw) = match app
        .create_token(user.0.id, &f.name, f.can_write.is_some(), f.can_review.is_some(), decks)
        .await
    {
        Ok(t) => t,
        Err(e) => return or_back("/access", Err(e)),
    };
    let url = mcp_url(&app, &headers);
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "ainki": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer {raw}") }
            }
        }
    }))
    .unwrap_or_default();
    Ok(layout_in(
        Nav::Settings,
        "Token created",
        Some(&user.0),
        html! {
            h1 { "Token created" }
            p.notice { "Copy it now. It won't be shown again." }
            p { strong { (token.name) } }
            pre.copy { (raw) }
            h2 { "Connect an agent" }
            p { "MCP server URL:" }
            pre.copy { (url) }
            p { "Send the token as a header: " code { "Authorization: Bearer <token>" } }
            p { "Claude Code:" }
            pre.copy { "claude mcp add --transport http ainki " (url) " --header \"Authorization: Bearer " (raw) "\"" }
            p { "JSON config (Claude Desktop, Cursor and others):" }
            pre.copy { (config) }
            p { a.btn href="/access" { "Done" } }
        },
    )
    .into_response())
}

async fn token_revoke(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back("/access", async {
        app.revoke_token(user.0.id, id).await?;
        Ok(redirect_msg("/access", "Access removed."))
    }
    .await)
}

// ----- import -------------------------------------------------------------

async fn import_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let decks = app.decks(&user.access()).await?;
    Ok(layout_in(
        Nav::Add,
        "Import",
        Some(&user.0),
        html! {
            (add_tabs("import"))
            (flash(&msg))
            h1 { "Import" }
            h2 { "From Anki (.apkg)" }
            p { "In Anki, use " strong { "File → Export…" } ", choose " em { "Anki Deck Package (*.apkg)" } " and tick " em { "Include scheduling information" } " to keep your progress. Upload the file here and its decks are created for you." }
            p.muted.small { "Each card keeps the first two fields of its note, the first image of each side and its review history. Cloze notes become one card per gap. " code { ".colpkg" } " backups work too." }
            form method="post" action="/import" enctype="multipart/form-data" {
                input type="hidden" name="kind" value="apkg";
                label.file.drop.icon-link {
                    (icon(ICON_IMPORT, DROP_ICON))
                    span.drop-text { "Choose an Anki file" }
                    span.muted.small { ".apkg or .colpkg, or drop it here" }
                    input type="file" name="file" accept=".apkg,.colpkg" required;
                }
                div.form-foot.end { button.btn.primary type="submit" { "Import Anki deck" } }
            }
            h2 { "From a CSV file" }
            p { "One card per line: front, back. Commas, semicolons or tabs work. A header line \"front,back\" is skipped." }
            @if decks.is_empty() {
                p { "CSV files go into an existing deck. " a href="/decks/new" { "Create a deck first." } }
            } @else {
                form method="post" action="/import" enctype="multipart/form-data" {
                    input type="hidden" name="kind" value="csv";
                    div.deck-line {
                        label.deck-pick { span.deck-label { "Into deck" }
                            span.select { select name="deck" { @for d in &decks { option value=(d.id) { (deck_plain(&d.name)) } } } }
                        }
                    }
                    div.sheet.paste {
                        label.face-label for="csv-text" { "Paste the cards" }
                        textarea.csv id="csv-text" name="text" rows="5" placeholder="Hund,dog\nKatze,cat" {}
                        label.file.attach.icon-link {
                            (icon(ICON_FILE, PAGE_ICON))
                            span.attach-text { "Or choose a file" }
                            " " span.muted.optional { "(.csv, .tsv, .txt)" }
                            input type="file" name="file" accept=".csv,.tsv,.txt";
                        }
                    }
                    div.form-foot.end { button.btn type="submit" { "Import CSV" } }
                }
            }
        },
    )
    .into_response())
}

async fn import_upload(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let mut fields = match read_multipart(mp).await {
        Ok(f) => f,
        Err(e) => return or_back("/import", Err(e)),
    };
    let file = fields.remove("file");
    let text = |k: &str| field_text(&fields, k);
    // The file has to be picked again anyway, so problems come back as a
    // message on the import page.
    let report = async { Ok::<_, AppError>(match text("kind").as_str() {
        "apkg" => {
            let data = file.filter(|d| !d.is_empty()).ok_or_else(|| AppError::bad("choose a file"))?;
            app.import_apkg(&access, data).await?
        }
        "csv" => {
            let deck: i64 = text("deck").parse().map_err(|_| AppError::bad("pick a deck"))?;
            let mut body = String::from_utf8_lossy(file.as_deref().unwrap_or_default()).to_string();
            if body.trim().is_empty() {
                body = text("text");
            }
            app.import_csv(&access, deck, &body).await?
        }
        _ => return Err(AppError::bad("unknown import type")),
    }) }
    .await;
    let report = match report {
        Ok(r) => r,
        Err(e) => return or_back("/import", Err(e)),
    };
    let mut msg = format!("Imported {}", count(report.cards_imported, "card"));
    if report.reviews_imported > 0 {
        msg += &format!(", {}", count(report.reviews_imported, "review"));
    }
    if report.images_imported > 0 {
        msg += &format!(", {}", count(report.images_imported, "image"));
    }
    if !report.decks_created.is_empty() {
        let names: Vec<String> = report.decks_created.iter().map(|n| deck_plain(n)).collect();
        msg += &format!(". New decks: {}", names.join(", "));
    }
    if report.skipped > 0 {
        msg += &format!(". Skipped {} empty", report.skipped);
    }
    msg.push('.');
    Ok(redirect_msg("/", &msg))
}

// ----- stats --------------------------------------------------------------

#[derive(Deserialize)]
struct StatsQuery {
    deck: Option<i64>,
}

async fn stats_page(State(app): State<AppState>, user: WebUser, Query(q): Query<StatsQuery>) -> WebResult {
    let access = user.access();
    let s = app.stats(&access, q.deck).await?;
    let cal = app.calibration(&access, q.deck).await?;
    let sure = app.confidence(&access, q.deck).await?;
    let week = if q.deck.is_none() { Some(app.week(&access).await?) } else { None };
    let retention: Vec<_> = app
        .deck_retention(&access)
        .await?
        .into_iter()
        .filter(|d| d.reviews > 0 && q.deck.is_none_or(|id| id == d.deck_id))
        .collect();
    let deck = match q.deck {
        Some(id) => Some(app.deck(&access, id).await?),
        None => None,
    };
    let max = s.forecast.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    let history = app.history(&access, q.deck).await?;
    let desk = DeskStats { user: &user.0, deck: q.deck, s: &s, cal: &cal, sure: &sure, retention: &retention, history: &history };
    Ok(layout_in(
        Nav::Stats,
        "Stats",
        Some(&user.0),
        html! {
            @if let Some(d) = &deck { (parent(&format!("/decks/{}", d.id), &deck_plain(&d.name))) }
            div.stats-title {
                h1 { "Stats" }
                label.numbers-toggle.calm-only { "I love numbers" input type="checkbox" role="switch"; }
            }
            (desk_stats(&desk))
            // On desktops these are the exact numbers, under the charts, for
            // those who switch them on.
            div.numbers {
            @if let Some(w) = &week { (week_note(w)) }
            h2 { "Today" }
            dl.stats-inline {
                div { dt { (format::plural(s.cards_today, "Card")) " studied" } dd { (format::number(s.cards_today)) } }
                div { dt { (format::plural(s.minutes_today.round() as i64, "Minute")) } dd { (s.minutes_today.round() as i64) } }
                div { dt { (format::plural(s.streak_days, "Day")) " in a row" } dd { (s.streak_days) } }
            }
            h2 { "Memory" }
            dl.kv {
                dt { "Reviews, last 30 days" } dd { (format::number(s.reviews_30d)) }
                dt { "Recalled when due, last 30 days" } dd { (format::percent(s.true_retention_30d)) " (" (count(s.retention_sample_30d, "review")) ")" }
                dt { "Average recall chance now" } dd { (format::percent(s.average_retrievability)) }
            }
            @if !retention.is_empty() {
                table.retention {
                    thead { tr { th { "Deck" } th.num { "Aim" } th.num { "Remembered" } th.num { "Reviews" } } }
                    tbody {
                        @for d in &retention {
                            tr.short[d.short()] {
                                th scope="row" { a href=(format!("/decks/{}", d.deck_id)) { (deck_plain(&d.deck_name)) } }
                                td.num { (format::percent(Some(d.target))) }
                                td.num { (format::percent(d.actual)) }
                                td.num { (format::number(d.reviews)) }
                            }
                        }
                    }
                }
                p.muted.small { "Reviews of learned cards in the last 30 days. Fewer than " (insight::MIN_SAMPLE) " say little." }
            }
            h2 { "Cards" }
            dl.kv {
                dt { "New" } dd { (format::number(s.new)) }
                dt { "Learning" } dd { (format::number(s.learning)) }
                dt { "Review" } dd { (format::number(s.review)) }
                dt { "Suspended" } dd { (format::number(s.suspended)) }
                dt { "Total" } dd { (format::number(s.total)) }
            }
            h2 id="model" { "How well FSRS knows you" }
            @if cal.reviews < 50 {
                p.muted { "After a few dozen reviews of learned cards, this shows how well FSRS predicts your memory." }
            } @else {
                p {
                    "Over the last year, FSRS expected you to remember " (format::percent(Some(cal.predicted)))
                    " of your reviews; you remembered " (format::percent(Some(cal.actual))) "."
                    @if (cal.predicted - cal.actual).abs() <= 0.03 { " That's a close fit." }
                    @else if cal.actual < cal.predicted { " You forget a little more than it thinks; optimizing helps." }
                    @else { " You remember more than it thinks; optimizing helps." }
                }
                table.calibration {
                    thead { tr { th { "Expected" } th.num { "Remembered" } th.num { "Reviews" } } }
                    tbody {
                        @for b in &cal.bins {
                            tr {
                                th scope="row" { (b.from) "–" (b.to) "%" }
                                td.num { (format::percent(Some(b.actual))) }
                                td.num { (format::number(b.reviews)) }
                            }
                        }
                    }
                }
                p.muted.small { a href="/learn/optimizing" { "What this means" } }
            }
            @if sure.sure + sure.unsure > 0 {
                h2 id="sure" { "When you felt sure" }
                p {
                    @if let Some(p) = sure.sure_pct() {
                        "When you felt sure, you remembered " (format!("{p:.0}%")) " (" (count(sure.sure, "answer")) "). "
                    }
                    @if let Some(p) = sure.unsure_pct() {
                        "When you weren't, " (format!("{p:.0}%")) " (" (count(sure.unsure, "answer")) ")."
                    }
                }
                @if let (Some(a), Some(b)) = (sure.sure_pct(), sure.unsure_pct()) {
                    p.muted.small {
                        @if a - b >= 15.0 { "Your sense of what you know is a good guide." }
                        @else { "Feeling sure says little about whether you remember. Testing yourself is the better guide." }
                    }
                }
            }
            h2 { "Due in the next two weeks" }
            table.forecast {
                tbody {
                    @for (day, n) in &s.forecast {
                        tr {
                            th scope="row" { (forecast_day(&user.0, day)) }
                            td.bar-cell {
                                @if *n > 0 { span.bar style=(format!("width:{}%", n * 100 / max)) {} }
                            }
                            td.num { (format::number(*n)) }
                        }
                    }
                }
            }
            }
        },
    )
    .into_response())
}

struct DeskStats<'a> {
    user: &'a User,
    deck: Option<i64>,
    s: &'a crate::service::Stats,
    cal: &'a crate::tuning::Calibration,
    sure: &'a study::Confidence,
    retention: &'a [insight::DeckRetention],
    history: &'a crate::service::History,
}

/// The desktop stats: three numbers to act on, then charts, each with a
/// note beside it. desk.js draws the charts from the data attributes.
fn desk_stats(d: &DeskStats) -> Markup {
    let s = d.s;
    let study = match d.deck { Some(id) => format!("/study?deck={id}"), None => "/study".into() };
    let (week_total, week_passed) = d.history.weeks.last().copied().unwrap_or((0, 0));
    let this_week = (week_total > 0).then(|| week_passed as f64 / week_total as f64);
    let aims: Vec<f64> = d.retention.iter().map(|r| r.target).collect();
    let aim = (!aims.is_empty()).then(|| aims.iter().sum::<f64>() / aims.len() as f64);
    let worst = d.retention.iter().filter(|r| r.short()).min_by(|a, b| {
        (a.actual.unwrap_or(1.0) - a.target).partial_cmp(&(b.actual.unwrap_or(1.0) - b.target)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let last7 = d.history.days.iter().rev().take(7).filter(|(_, n)| *n > 0).count();
    let minutes = s.seconds_per_review.map(|sec| sec * s.due_now as f64 / 60.0);
    let weeks: Vec<String> = d.history.weeks.iter().map(|(t, p)| format!("{t},{p}")).collect();
    let days: Vec<String> = d.history.days.iter().map(|(_, n)| n.to_string()).collect();
    let fc: Vec<String> = s.forecast.iter().map(|(_, n)| n.to_string()).collect();
    let fc_labels: Vec<String> = s.forecast.iter().map(|(day, _)| forecast_day(d.user, day)).collect();
    let decks: Vec<String> = d
        .retention
        .iter()
        .filter_map(|r| r.actual.map(|a| format!("{}|{}|{}|{}", deck_plain(&r.deck_name).replace(['|', ';'], " "), r.target, a, r.deck_id)))
        .collect();
    let bins: Vec<String> = d.cal.bins.iter().map(|b| format!("{},{},{}", b.predicted, b.actual, b.reviews)).collect();
    html! {
        div.desk-stats.calm-only {
            section.big3 {
                div {
                    p.kicker { "Due today" }
                    p.n.due { (format::number(s.due_now)) }
                    p.small.muted {
                        @if let Some(m) = minutes.filter(|_| s.due_now > 0) { (capitalize(&format::minutes(m))) "." } @else if s.due_now == 0 { "Nothing waiting." }
                    }
                    @if s.due_now > 0 { a.small href=(study) { "Start studying" } }
                }
                div {
                    p.kicker { "Remembered this week" }
                    p.n { (format::percent(this_week)) }
                    p.small.muted {
                        @if this_week.is_none() { "No reviews of learned cards this week. " }
                        @if let Some(a) = aim { "You aim for " (format::percent(Some(a))) "." }
                        @if let Some(w) = worst { " " (deck_plain(&w.deck_name)) " pulls it down." }
                    }
                    @if let Some(w) = worst { a.small href=(format!("/decks/{}", w.deck_id)) { "Look at that deck" } }
                }
                div {
                    p.kicker { "Days in a row" }
                    p.n.it { (s.streak_days) }
                    p.small.muted { "You studied on " (last7) " of the last 7 days." }
                }
            }
            section.chart-sec {
                div.chart-main {
                    h2 { "Are you remembering what you aim for?" }
                    p.small.muted { "Share of due cards you remembered, week by week." }
                    div.chart data-chart="retention" data-weeks=(weeks.join(";")) data-aim=[aim] role="img" aria-label="Recall by week, last twelve weeks" {}
                }
                aside.chart-side {
                    p.kicker { "Study days" }
                    div.chart data-chart="days" data-days=(days.join(",")) data-first=(d.history.days.first().map(|(day, _)| day.as_str()).unwrap_or("")) role="img" aria-label="Reviews per day, last twelve weeks" {}
                    p.small.muted { "Darker means more reviews. " (last7) " of the last 7 days." }
                }
            }
            section.chart-sec {
                div.chart-main {
                    h2 { "Next two weeks" }
                    p.small.muted { "Cards due each day." }
                    div.chart data-chart="forecast" data-values=(fc.join(",")) data-labels=(fc_labels.join(",")) role="img" aria-label="Cards due on each of the next fourteen days" {}
                }
                aside.chart-side {
                    p.kicker { "Cards" }
                    p.serif-note { (count(s.total, "card")) ". " (format::number(s.review)) " in review, " (format::number(s.new)) " still new." }
                    p.small.muted { (format::number(s.learning)) " learning, " (format::number(s.suspended)) " suspended." }
                }
            }
            @if !decks.is_empty() {
                section.chart-sec {
                    div.chart-main {
                        h2 { "By deck" }
                        p.legend.small { span.aim { "aim" } span.ok { "remembered" } span.low { "below aim" } }
                        div.chart data-chart="decks" data-decks=(decks.join(";")) role="img" aria-label="Recall by deck against its aim, last 30 days" {}
                    }
                    aside.chart-side {
                        p.kicker { "What to do" }
                        @if let Some(w) = worst {
                            p.small.warn-note { (deck_plain(&w.deck_name)) " is " (((w.target - w.actual.unwrap_or(w.target)) * 100.0).round() as i64) " points below its aim." }
                            a.small href="/attention" { "Open Needs a look" }
                        } @else {
                            p.small { "Every deck is close to its aim." }
                        }
                    }
                }
            }
            @if d.cal.reviews >= 50 {
                section.chart-sec {
                    div.chart-main.calib {
                        div.chart data-chart="calibration" data-bins=(bins.join(";")) role="img" aria-label="Expected against remembered, last year" {}
                        div {
                            h2 { "How well FSRS knows you" }
                            p.serif-note {
                                "FSRS expected you to remember " (format::percent(Some(d.cal.predicted))) " of your reviews last year. You remembered " (format::percent(Some(d.cal.actual))) "."
                                @if (d.cal.predicted - d.cal.actual).abs() <= 0.03 { " That's a close fit." }
                            }
                            a.small href="/learn/optimizing" { "What this means" }
                        }
                    }
                    @if let (Some(a), Some(b)) = (d.sure.sure_pct(), d.sure.unsure_pct()) {
                        aside.chart-side {
                            p.kicker { "When you felt sure" }
                            div.sure-bars {
                                span { "Sure" } span.track { span.fill style=(format!("width:{a:.0}%")) {} } span.num { (format!("{a:.0}%")) }
                                span { "Not sure" } span.track { span.fill.grey style=(format!("width:{b:.0}%")) {} } span.num { (format!("{b:.0}%")) }
                            }
                            p.small.muted {
                                @if a - b >= 15.0 { "Your sense of what you know is a good guide." }
                                @else { "Feeling sure says little here. Testing yourself is the better guide." }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ----- media --------------------------------------------------------------

async fn media_file(State(app): State<AppState>, user: WebUser, Path(id): Path<String>) -> WebResult {
    let id = id.trim_end_matches(".png");
    let (m, bytes) = app.media(user.0.id, id).await?;
    let mut res = bytes.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&m.mime).unwrap_or(HeaderValue::from_static("image/png")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    Ok(res)
}

// ----- sharing ------------------------------------------------------------

#[derive(Deserialize)]
struct ShareForm {
    user: String,
}

async fn deck_share(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<ShareForm>,
) -> WebResult {
    or_back(format!("/decks/{id}/edit#share"), async {
        let share = app.share_deck(&user.access(), id, &f.user).await?;
        Ok(redirect_msg(
            format!("/decks/{id}/edit#share"),
            &format!("Shared with {}. They'll see it on their home page.", share.to),
        ))
    }
    .await)
}

async fn share_action(
    State(app): State<AppState>,
    user: WebUser,
    Path((id, action)): Path<(i64, String)>,
) -> WebResult {
    let access = user.access();
    or_back("/", async {
        match action.as_str() {
            "accept" => {
                let deck = app.accept_share(&access, id).await?;
                Ok(redirect_msg(format!("/decks/{}", deck.id), "Shared deck added to your decks."))
            }
            "decline" => {
                app.decline_share(&access, id).await?;
                Ok(redirect_msg("/", "Share declined."))
            }
            "cancel" => {
                let deck = app.cancel_share(&access, id).await?;
                Ok(redirect_msg(format!("/decks/{deck}/edit#share"), "Share cancelled."))
            }
            _ => Err(AppError::bad("unknown action")),
        }
    }
    .await)
}
