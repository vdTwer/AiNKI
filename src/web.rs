//! The website: server-rendered HTML, no JavaScript required, high contrast,
//! no animations, big tap targets. Designed for phones and e-ink readers.

use std::{collections::HashMap, time::Duration};

use axum::{
    Router,
    extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, Query, State},
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
    error::AppError,
    import, markdown, mcp, media, now, oauth, presets,
    security::{self, ClientIp, Flash},
    scheduler::{self, CardState, Rating},
    service::{Card, CardFilter, CardInput, Deck, PresetInput},
    format::{self, count},
};

const CSS: &str = include_str!("style.css");
const CALM_CSS: &str = include_str!("static/calm.css");
const THEME_JS: &str = include_str!("static/theme.js");
const APP_JS: &str = include_str!("static/app.js");
const FONTS: &[(&str, &[u8])] = &[
    ("newsreader-italic-latin-ext.woff2", include_bytes!("static/fonts/newsreader-italic-latin-ext.woff2")),
    ("newsreader-italic-latin.woff2", include_bytes!("static/fonts/newsreader-italic-latin.woff2")),
    ("newsreader-latin-ext.woff2", include_bytes!("static/fonts/newsreader-latin-ext.woff2")),
    ("newsreader-latin.woff2", include_bytes!("static/fonts/newsreader-latin.woff2")),
    ("source-sans-3-latin-ext.woff2", include_bytes!("static/fonts/source-sans-3-latin-ext.woff2")),
    ("source-sans-3-latin.woff2", include_bytes!("static/fonts/source-sans-3-latin.woff2")),
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
        .route("/decks/new", get(deck_new_page).post(deck_create))
        .route("/decks/{id}", get(deck_page))
        .route("/decks/{id}/edit", get(deck_edit_page).post(deck_update))
        .route("/decks/{id}/delete", post(deck_delete))
        .route("/decks/{id}/export.csv", get(deck_export))
        .route("/decks/{id}/share", post(deck_share))
        .route("/shares/{id}", post(share_action))
        .route("/cards/{id}/action", post(card_action))
        .route("/presets", get(presets_page).post(preset_create))
        .route("/presets/{id}", get(preset_page).post(preset_update))
        .route("/presets/{id}/delete", post(preset_delete))
        .route("/settings", get(settings_page).post(settings_update))
        .route("/settings/password", post(password_update))
        .route("/settings/optimize", post(optimize))
        .route("/settings/reset-params", post(reset_params))
        .route("/tokens", get(tokens_page).post(token_create))
        .route("/tokens/{id}/revoke", post(token_revoke))
        .route("/stats", get(stats_page))
        .route("/media/{id}", get(media_file))
        .layer(small)
        .route("/cards/new", get(card_new_page).post(card_create).layer(card_upload))
        .route("/cards/{id}", get(card_page).post(card_update).layer(card_upload))
        .route(
            "/import",
            get(import_page)
                .post(import_upload)
                .layer(DefaultBodyLimit::max(import::MAX_IMPORT_BYTES + 1024 * 1024)),
        )
        .layer(middleware::from_fn_with_state(app.clone(), security::check_origin));
    let static_routes = Router::new()
        .route("/style.css", get(style))
        .route("/calm.css", get(|| static_file("text/css; charset=utf-8", CALM_CSS.as_bytes())))
        .route("/theme.js", get(|| static_file("text/javascript; charset=utf-8", THEME_JS.as_bytes())))
        .route("/fonts/{name}", get(font_file))
        // Revalidated on every load so pages and their script stay in step.
        .route("/app.js", get(|| async { ([(header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")], APP_JS) }))
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
        let body = layout(
            "Error",
            None,
            html! {
                h1 { "Sorry" }
                p.notice { (msg) }
                p { a.btn href="/" { "Home" } }
            },
        );
        (status, body).into_response()
    }
}

type WebResult<T = Response> = Result<T, WebError>;

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
        let all = [CSS, CALM_CSS, THEME_JS, APP_JS].concat();
        crate::auth::sha256_hex(&all)[..10].to_string()
    })
}

/// Where the Calm look may apply; must match static/theme.js.
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

pub fn layout(title: &str, user: Option<&User>, body: Markup) -> Markup {
    page(title, user, None, body)
}

/// `page_kind` marks special pages for app.js.
fn page(title: &str, user: Option<&User>, page_kind: Option<&str>, body: Markup) -> Markup {
    let scale = user.map(|u| u.font_scale).unwrap_or(100) as f64 / 100.0;
    html! {
        (DOCTYPE)
        html lang="en" data-look=[user.map(|u| if u.desktop_look == "simple" { "simple" } else { "calm" })] {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · AiNKI" }
                link rel="stylesheet" href=(format!("/style.css?v={}", asset_version()));
                // The Calm look: only on desktops, and only once theme.js
                // has switched it on. Everything else gets style.css alone.
                link rel="stylesheet" href=(format!("/calm.css?v={}", asset_version())) media=(DESKTOP_MEDIA);
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
                        a.brand href="/" data-nav="decks" data-label="AiNKI" { "AiNKI" }
                        a.rail-only href="/" data-nav="decks" data-label="Decks" { (nav_icon(ICON_DECKS)) "Decks" }
                        a href="/study" data-nav="study" data-label="Study" { (nav_icon(ICON_STUDY)) "Study" }
                        a href="/cards/new" data-nav="add" data-label="Add" { (nav_icon(ICON_ADD)) "Add" }
                        a href="/stats" data-nav="stats" data-label="Stats" { (nav_icon(ICON_STATS)) "Stats" }
                        a href="/settings" data-nav="settings" data-label="Settings" { (nav_icon(ICON_SETTINGS)) "Settings" }
                    }
                }
                main { (body) }
                script src=(format!("/app.js?v={}", asset_version())) defer {}
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
const ICON_SETTINGS: &str = r#"<path d="M5 7h8M17 7h2M5 17h2M11 17h8"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>"#;
const ICON_EDIT: &str = r#"<path d="M14.5 5.5l4 4L9 19H5v-4z"/><path d="M12.5 7.5l4 4"/>"#;
const ICON_SHARE: &str = r#"<path d="M12 4v11M8 8l4-4 4 4"/><path d="M6 12v6.5A1.5 1.5 0 0 0 7.5 20h9a1.5 1.5 0 0 0 1.5-1.5V12"/>"#;
const ICON_IMPORT: &str = r#"<path d="M12 4v11M8 11l4 4 4-4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
const ICON_EXPORT: &str = r#"<path d="M12 15V4M8 8l4-4 4 4"/><path d="M5 16v2.5A1.5 1.5 0 0 0 6.5 20h11a1.5 1.5 0 0 0 1.5-1.5V16"/>"#;
const ICON_UNDO: &str = r#"<path d="M9 7L5 11l4 4"/><path d="M5 11h9a5 5 0 0 1 0 10h-2"/>"#;
const ICON_IMAGE: &str = r#"<rect x="3.5" y="5" width="17" height="14" rx="2.5"/><circle cx="9" cy="10" r="1.6"/><path d="M4.5 17l4.5-4.2 3.2 3 2.6-2.2 4.7 4"/>"#;
const ICON_FILE: &str = r#"<path d="M7 3.5h6.5L18 8v11a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 19V5a1.5 1.5 0 0 1 1-1.5z"/><path d="M13 3.5V8h5"/>"#;

const RAIL_ICON: u32 = 26;
const PAGE_ICON: u32 = 20;

/// An icon as a small solid object: the drawing repeated a little further
/// back each time gives it depth, plus a soft shadow behind. calm.css
/// spaces the layers out and tilts it with its link; elsewhere it is
/// hidden and only the words show. Lines stay 2px at every size.
fn icon(paths: &'static str, size: u32) -> Markup {
    let stroke = format!("{:.2}", 48.0 / size as f64);
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

/// The same where the simple look shows a button.
fn icon_btn(href: &str, paths: &'static str, label: &str) -> Markup {
    html! { a.btn.icon-link href=(href) { (icon(paths, PAGE_ICON)) span { (label) } } }
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

fn time_days(days: i64) -> time::Duration {
    time::Duration::days(days)
}

/// Small static assets. They change only with a new release, so an hour of
/// caching is plenty.
async fn static_file(content_type: &'static str, body: &'static [u8]) -> Response {
    (
        [(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, "public, max-age=3600")],
        body,
    )
        .into_response()
}

async fn font_file(Path(name): Path<String>) -> Response {
    match FONTS.iter().find(|(n, _)| *n == name) {
        Some((_, bytes)) => (
            [(header::CONTENT_TYPE, "font/woff2"), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
            *bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn style() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        CSS,
    )
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
        "Decks",
        Some(&user.0),
        Some("home"),
        html! {
            (flash(&msg))
            section.today aria-label="Today" {
                p.date { (format::long_date(&user.0.clock())) }
                @if total_due > 0 {
                    h1 { span.count { (total_due) (if total_due == 1 { " card" } else { " cards" }) } " to study today" }
                } @else if !decks.is_empty() {
                    h1 { "All done for today" }
                } @else {
                    h1 { "Welcome to AiNKI" }
                }
                @if !facts.is_empty() { p.facts { (facts.join(" · ")) } }
                @if total_due > 0 {
                    p { a.btn.primary.start href="/study" { "Start studying " span.arrow aria-hidden="true" { "→" } } }
                }
            }
            @if !shares.is_empty() {
                h2 { "Shared with you" }
                ul.list {
                    @for sh in &shares {
                        li.token {
                            div.grow {
                                strong { (deck_title(&sh.deck_name)) } br;
                                span.small { (sh.cards) (if sh.cards == 1 { " card" } else { " cards" }) " from " (sh.from) }
                            }
                            form method="post" action=(format!("/shares/{}", sh.id)) {
                                button.btn.primary type="submit" name="action" value="accept" { "Accept" }
                                " "
                                button.btn type="submit" name="action" value="decline" { "Decline" }
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
                                @if d.due_total() == 0 && d.total > 0 {
                                    span.num.due.done title="Done for today" { span.tick aria-hidden="true" { "✓" } span.unit { " done for today" } }
                                } @else {
                                    span.num.due title="Due" { @if due > 0 { (due) span.unit { " due" } } @else { (format::NONE) } }
                                }
                                span.num.new title="New" { @if d.new_today > 0 { span.dot aria-hidden="true" {} (d.new_today) span.unit { " new" } } @else { (format::NONE) } }
                                span.chev aria-hidden="true" { "›" }
                            }
                        }
                    }
                }
            }
            p.actions.icon-links {
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
}

fn study_url(deck: Option<i64>) -> String {
    match deck {
        Some(d) => format!("/study?deck={d}"),
        None => "/study".into(),
    }
}

/// The thin line and "14 of 32" under the card. `done` is in half cards
/// (see `App::session_progress`).
fn study_foot(deck_name: &str, deck_id: i64, done: i64, total: i64, q_deck: Option<i64>, card_id: Option<i64>) -> Markup {
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
                @if let Some(id) = card_id { (icon_link(&format!("/cards/{id}"), ICON_EDIT, "Edit card")) }
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
    shown: i64,
    revealed: bool,
) -> Result<Markup, AppError> {
    let previews = app.previews(access, card, deck).await?;
    let (done, total) = app.session_progress(access, q_deck).await?;
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
    Ok(page(
        "Study",
        Some(user),
        Some("study"),
        html! {
            (flash(msg))
            section.study.revealed[revealed] {
                div.card-area {
                    (state_tag(card.card_state()))
                    section.face.front { (face(&card.front, &card.front_image)) }
                    hr.divider hidden[!revealed];
                    section.face.back id="answer" hidden[!revealed] { (face(&card.back, &card.back_image)) }
                }
                p.reveal hidden[revealed] {
                    a.btn.primary.show data-key=" " href=(format!("/study?card={}&show=1&t={}{}#answer", card.id, shown, deck_param)) {
                        "Show answer" span.key { "space" }
                    }
                }
                form.answers.cols-2[!four].cols-4[four] method="post" action="/study/answer" hidden[!revealed] {
                    input type="hidden" name="card" value=(card.id);
                    input type="hidden" name="reps" value=(card.reps);
                    input type="hidden" name="t" value=(shown);
                    @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
                    @for (rating, label, key) in &buttons {
                        @let preview = previews.iter().find(|(r, _)| r == rating).map(|(_, p)| p.as_str()).unwrap_or("");
                        button.btn.primary[*rating == Rating::Good].again[*rating == Rating::Again] type="submit" name="rating" value=(*rating as i64) data-key=(key) {
                            span.label { (label) " " span.key { (key) } }
                            span.ivl { (preview) }
                        }
                    }
                }
                (study_foot(&deck.name, deck.id, done, total, q_deck, Some(card.id)))
            }
        },
    ))
}

async fn study(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Query(q): Query<StudyQuery>) -> WebResult {
    let access = user.access();

    // Answer side of a specific card.
    if let (Some(card_id), Some(1)) = (q.card, q.show) {
        let card = app.card(&access, card_id).await?;
        let deck = app.deck(&access, card.deck_id).await?;
        let shown = q.t.unwrap_or_else(now);
        return Ok(study_card(&app, &access, &user.0, &msg, &card, &deck, q.deck, shown, true).await?.into_response());
    }

    let Some(sc) = app.next_card(&access, q.deck).await? else {
        let decks = app.deck_summaries(&access).await?;
        let any_cards = decks.iter().any(|d| d.total > 0);
        let stats = app.stats(&access, q.deck).await?;
        let next = app.next_review_at(&access, q.deck).await?;
        return Ok(page(
            "Study",
            Some(&user.0),
            Some("study-done"),
            html! {
                (flash(&msg))
                section.session-end {
                    @if any_cards {
                        div.end-dot aria-hidden="true" { div {} }
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
    Ok(study_card(&app, &access, &user.0, &msg, &sc.card, &sc.deck, q.deck, now(), false).await?.into_response())
}

#[derive(Deserialize)]
struct AnswerForm {
    card: i64,
    rating: i64,
    reps: i64,
    t: Option<i64>,
    deck: Option<i64>,
}

async fn study_answer(State(app): State<AppState>, user: WebUser, Form(f): Form<AnswerForm>) -> WebResult {
    let rating = Rating::from_i64(f.rating).ok_or_else(|| AppError::bad("invalid rating"))?;
    // Like Anki, cap the counted thinking time at 60 seconds.
    let duration = f.t.map(|t| ((now() - t).clamp(0, 60)) * 1000);
    match app
        .answer_card(&user.access(), f.card, rating, duration, Some(f.reps))
        .await
    {
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

/// A deck written as a sheet: its name large, a description, then its
/// preset and (when editing) the exam date as lines.
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
            label.line { "Preset" span.select { (preset_select(presets, selected)) } }
            p.hint.muted.small { "How much you want to remember and your daily limits. " a href="/presets" { "About presets" } }
            @if let Some(d) = deck {
                label.line { span { "Exam date " span.muted { "(optional)" } }
                    input type="date" name="exam_date" value=(d.exam_date.clone().unwrap_or_default());
                }
                p.hint.muted.small { "With an exam date, no card in this deck is scheduled after the exam." }
            }
        }
    }
}

async fn deck_new_page(State(app): State<AppState>, user: WebUser) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout(
        "New deck",
        Some(&user.0),
        html! {
            (add_tabs("deck"))
            form method="post" action="/decks/new" {
                (deck_sheet(None, &presets, presets.first().map(|p| p.id)))
                div.form-foot.end { button.btn.primary type="submit" { "Create deck" } }
            }
        },
    )
    .into_response())
}

#[derive(Deserialize)]
struct DeckCreateForm {
    name: String,
    #[serde(default)]
    description: String,
    preset_id: Option<i64>,
    exam_date: Option<String>,
}

async fn deck_create(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckCreateForm>) -> WebResult {
    let deck = app
        .create_deck(&user.access(), &f.name, &f.description, f.preset_id)
        .await?;
    // Straight on to filling it.
    Ok(redirect_msg(format!("/cards/new?deck={}", deck.id), "Deck created. Add its first card."))
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
    let base = format!(
        "/decks/{id}?q={}&state={}",
        urlenc(q.q.as_deref().unwrap_or("")),
        urlenc(q.state.as_deref().unwrap_or(""))
    );
    Ok(layout(
        &deck_plain(&d.name),
        Some(&user.0),
        html! {
            (flash(&msg))
            h1 { (deck_title(&d.name)) }
            @if !d.description.is_empty() { p { (d.description) } }
            dl.stats-inline {
                div { dt { "New today" } dd { (summary.new_today) } }
                div { dt { "Due" } dd { (summary.learning_due + summary.review_due) } }
                div { dt { "Cards" } dd { (summary.total) } }
            }
            @if let Some(exam) = &d.exam_date { p.muted { "Exam " (exam_words(&user.0, exam)) } }
            @if summary.due_total() > 0 {
                p { a.btn.primary.start href=(format!("/study?deck={id}")) { "Start studying " span.arrow aria-hidden="true" { "→" } } }
            }
            p.actions.icon-links {
                (icon_btn(&format!("/cards/new?deck={id}"), ICON_ADD, "Add card"))
                (icon_btn(&format!("/decks/{id}/edit"), ICON_EDIT, "Edit deck"))
                (icon_btn(&format!("/decks/{id}/edit#share"), ICON_SHARE, "Share"))
                (icon_btn(&format!("/stats?deck={id}"), ICON_STATS, "Stats"))
            }
            p.muted.small { "Preset: " a href=(format!("/presets/{}", d.preset_id)) { (summary.preset_name) } }

            h2 { "Cards" }
            form.search method="get" action=(format!("/decks/{id}")) {
                input type="search" name="q" value=(q.q.clone().unwrap_or_default()) placeholder="Search text" aria-label="Search";
                select name="state" aria-label="Filter" {
                    @for (v, l) in [("", "All"), ("new", "New"), ("learning", "Learning"), ("review", "Review"), ("due", "Due"), ("suspended", "Suspended")] {
                        option value=(v) selected[q.state.as_deref().unwrap_or("") == v] { (l) }
                    }
                }
                button.btn type="submit" { "Find" }
            }
            p.muted.small { (count(total, "card")) }
            ul.list.cards {
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
                        }
                    }
                }
            }
            p.actions {
                @if page > 0 { a.btn href=(format!("{base}&page={}", page - 1)) { "Previous" } }
                @if (page + 1) * per_page < total { a.btn href=(format!("{base}&page={}", page + 1)) { "Next" } }
            }
            p.icon-links { (icon_link(&format!("/decks/{id}/export.csv"), ICON_EXPORT, "Export as CSV")) }
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

async fn deck_edit_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
) -> WebResult {
    let access = user.access();
    let deck = app.deck(&access, id).await?;
    let presets = app.list_presets(&access).await?;
    let shares = app.outgoing_shares(&access, id).await?;
    Ok(layout(
        "Edit deck",
        Some(&user.0),
        html! {
            (flash(&msg))
            h1 { "Edit deck" }
            form method="post" action=(format!("/decks/{id}/edit")) {
                (deck_sheet(Some(&deck), &presets, Some(deck.preset_id)))
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
                        li.token {
                            span.grow { (sh.to) }
                            form method="post" action=(format!("/shares/{}", sh.id)) {
                                button.btn type="submit" name="action" value="cancel" { "Cancel" }
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
    )
    .into_response())
}

async fn deck_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<DeckCreateForm>,
) -> WebResult {
    app.update_deck(
        &user.access(),
        id,
        Some(&f.name),
        Some(&f.description),
        f.preset_id,
        Some(f.exam_date.as_deref().unwrap_or("")),
    )
    .await?;
    Ok(redirect_msg(format!("/decks/{id}"), "Deck saved."))
}

async fn deck_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    let n = app.delete_deck(&user.access(), id).await?;
    Ok(redirect_msg("/", &format!("Deck deleted, with its {}.", count(n, "card"))))
}

async fn deck_export(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    let access = user.access();
    let deck = app.deck(&access, id).await?;
    let csv = app.export_csv(&access, id).await?;
    let filename: String = deck
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}.csv\""),
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
    pending_new_deck: Option<String>,
    front: String,
    back: String,
    front_upload: Option<Vec<u8>>,
    back_upload: Option<Vec<u8>>,
    remove_front_image: bool,
    remove_back_image: bool,
}

async fn read_card_form(mut mp: Multipart) -> Result<CardFormData, AppError> {
    let mut f = CardFormData::default();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| AppError::bad(format!("bad form: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::bad(format!("upload failed: {e}")))?;
        let text = || String::from_utf8_lossy(&data).to_string();
        match name.as_str() {
            "deck" if text() == NEW_DECK => f.new_deck = Some(String::new()),
            "deck" => f.deck = text().parse().ok(),
            "new_deck" => {
                if let Some(name) = f.new_deck.as_mut() {
                    *name = text();
                } else {
                    // Only used when "New deck…" is picked; remember it in
                    // case the picker comes later in the form.
                    f.pending_new_deck = Some(text());
                }
            }
            "front" => f.front = text(),
            "back" => f.back = text(),
            "front_image" if !data.is_empty() => f.front_upload = Some(data.to_vec()),
            "back_image" if !data.is_empty() => f.back_upload = Some(data.to_vec()),
            "remove_front_image" => f.remove_front_image = true,
            "remove_back_image" => f.remove_back_image = true,
            _ => {}
        }
    }
    if let (Some(name), Some(pending)) = (f.new_deck.as_mut(), f.pending_new_deck.take()) {
        if name.is_empty() {
            *name = pending;
        }
    }
    if f.front_upload.as_ref().is_some_and(|d| d.len() > media::MAX_UPLOAD_BYTES)
        || f.back_upload.as_ref().is_some_and(|d| d.len() > media::MAX_UPLOAD_BYTES)
    {
        return Err(AppError::bad("images must be smaller than 15 MB"));
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
fn card_form(
    action: &str,
    decks: &[crate::service::Deck],
    deck_id: Option<i64>,
    front: &str,
    back: &str,
    front_image: &Option<String>,
    back_image: &Option<String>,
    submit: &str,
    allow_new_deck: bool,
) -> Markup {
    let new_selected = allow_new_deck && (decks.is_empty() || deck_id.is_none());
    html! {
        form.card-form method="post" action=(action) enctype="multipart/form-data" {
            div.deck-line {
                label.deck-pick { span.deck-label { "Deck" }
                    span.select {
                        select name="deck" required data-new-deck-select {
                            @for d in decks { option value=(d.id) selected[!new_selected && Some(d.id) == deck_id] { (deck_plain(&d.name)) } }
                            @if allow_new_deck {
                                option value=(NEW_DECK) selected[new_selected] { "New deck…" }
                            }
                        }
                    }
                }
                @if allow_new_deck {
                    label.new-deck id="new-deck-field" {
                        span.deck-label { "New deck name" }
                        input type="text" name="new_deck" maxlength="200" placeholder="e.g. Spanish verbs";
                    }
                }
            }
            div.sheet {
                (face_field("front", "Front", "Question", front, front_image, true))
                hr.divider;
                (face_field("back", "Back", "Answer", back, back_image, false))
            }
            div.form-foot {
                p.muted.small { "Text supports Markdown: **bold**, *italic*, lists, `code`." }
                button.btn.primary.wide type="submit" { (submit) span.key.submit-key hidden { "Ctrl ⏎" } }
            }
        }
    }
}

#[derive(Deserialize)]
struct NewCardQuery {
    deck: Option<i64>,
}

async fn card_new_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash, Query(q): Query<NewCardQuery>) -> WebResult {
    let access = user.access();
    let decks = app.decks(&access).await?;
    // The deck from the link, else the one last added to, else the first.
    let deck = match q.deck {
        Some(id) => Some(id),
        None => app.last_used_deck(&access).await?.or_else(|| decks.first().map(|d| d.id)),
    };
    Ok(layout(
        "Add card",
        Some(&user.0),
        html! {
            (add_tabs("card"))
            (flash(&msg))
            h1 { "Add card" }
            (card_form("/cards/new", &decks, deck, "", "", &None, &None, "Add card", true))
        },
    )
    .into_response())
}

async fn card_create(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let f = read_card_form(mp).await?;
    // A taken name adds to that deck instead of failing; the message says so.
    let mut msg = "Card added.".to_string();
    let deck = match (&f.new_deck, f.deck) {
        (Some(name), _) => {
            if name.trim().is_empty() {
                return Err(AppError::bad("give the new deck a name").into());
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
        (None, None) => return Err(AppError::bad("pick a deck").into()),
    };
    let front_image = match f.front_upload {
        Some(d) => Some(app.store_image(&access, d).await?.id),
        None => None,
    };
    let back_image = match f.back_upload {
        Some(d) => Some(app.store_image(&access, d).await?.id),
        None => None,
    };
    app.create_card(&access, deck, &f.front, &f.back, front_image, back_image)
        .await?;
    Ok(redirect_msg(format!("/cards/new?deck={deck}"), &msg))
}

async fn card_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
) -> WebResult {
    let access = user.access();
    let card = app.card(&access, id).await?;
    let decks = app.decks(&access).await?;
    let history = app.card_history(&access, id).await?;
    let clock = user.0.clock();
    let params = user.0.params();
    let r = scheduler::retrievability(&card.sched(), &params, &clock);
    Ok(layout(
        "Edit card",
        Some(&user.0),
        html! {
            (flash(&msg))
            h1 { "Edit card" }
            (card_form(&format!("/cards/{id}"), &decks, Some(card.deck_id), &card.front, &card.back, &card.front_image, &card.back_image, "Save", false))
            h2 { "Scheduling" }
            dl.kv {
                dt { "State" } dd { (state_label(card.card_state())) @if card.suspended { " (suspended)" } }
                @if card.card_state() != CardState::New {
                    dt { "Due" } dd { (capitalize(&format::when(&clock, card.due))) }
                }
                @if let Some(s) = card.stability { dt { "Stability" } dd { (format!("{s:.1} days")) } }
                @if let Some(d) = card.difficulty { dt { "Difficulty" } dd { (format!("{d:.1} / 10")) } }
                @if let Some(r) = r { dt { "Recall chance now" } dd { (format::percent(Some(r as f64))) } }
                dt { "Reviews / lapses" } dd { (card.reps) " / " (card.lapses) }
                dt { "Added" } dd { (source_label(&card.created_by)) }
            }
            form.actions method="post" action=(format!("/cards/{id}/action")) {
                @if card.suspended {
                    button.btn type="submit" name="action" value="unsuspend" { "Unsuspend" }
                } @else {
                    button.btn type="submit" name="action" value="suspend" { "Suspend" }
                }
                button.btn type="submit" name="action" value="reset" { "Forget progress" }
                button.btn.danger type="submit" name="action" value="delete" { "Delete" }
            }
            @if !history.is_empty() {
                h2 { "History" }
                table.history {
                    thead { tr { th { "When" } th { "Answer" } th { "Next" } } }
                    tbody {
                        @for h in history.iter().rev() {
                            tr {
                                td { (capitalize(&format::when(&clock, h["timestamp"].as_i64().unwrap_or(0)))) }
                                td { (rating_label(h["rating"].as_str().unwrap_or(""))) @if h["source"] == "agent" { " (by an agent)" } }
                                td { (h["next_interval"].as_str().unwrap_or("")) }
                            }
                        }
                    }
                }
            }
            p { a href=(format!("/decks/{}", card.deck_id)) { "Back to deck" } }
        },
    )
    .into_response())
}

async fn card_update(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>, mp: Multipart) -> WebResult {
    let access = user.access();
    let f = read_card_form(mp).await?;
    let front_image = match f.front_upload {
        Some(d) => Some(Some(app.store_image(&access, d).await?.id)),
        None if f.remove_front_image => Some(None),
        None => None,
    };
    let back_image = match f.back_upload {
        Some(d) => Some(Some(app.store_image(&access, d).await?.id)),
        None if f.remove_back_image => Some(None),
        None => None,
    };
    app.update_card(
        &access,
        id,
        CardInput {
            front: Some(f.front),
            back: Some(f.back),
            front_image,
            back_image,
        },
    )
    .await?;
    if let Some(deck) = f.deck {
        if deck != app.card(&access, id).await?.deck_id {
            app.move_card(&access, id, deck).await?;
        }
    }
    Ok(redirect_msg(format!("/cards/{id}"), "Card saved."))
}

#[derive(Deserialize)]
struct ActionForm {
    action: String,
}

async fn card_action(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<ActionForm>,
) -> WebResult {
    let access = user.access();
    let msg = match f.action.as_str() {
        "suspend" => {
            app.set_suspended(&access, id, true).await?;
            "Card suspended."
        }
        "unsuspend" => {
            app.set_suspended(&access, id, false).await?;
            "Card unsuspended."
        }
        "reset" => {
            app.reset_card(&access, id).await?;
            "Card progress forgotten; it's new again."
        }
        "delete" => {
            let card = app.card(&access, id).await?;
            app.delete_card(&access, id).await?;
            return Ok(redirect_msg(format!("/decks/{}", card.deck_id), "Card deleted."));
        }
        _ => return Err(AppError::bad("unknown action").into()),
    };
    Ok(redirect_msg(format!("/cards/{id}"), msg))
}

// ----- presets ------------------------------------------------------------

async fn presets_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout(
        "Presets",
        Some(&user.0),
        html! {
            (flash(&msg))
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
            form method="post" action="/presets" {
                div.sheet.form {
                    label.line { "Start from"
                        span.select {
                            select name="template" {
                                @for t in presets::TEMPLATES { option value=(t.key) { (t.name) } }
                            }
                        }
                    }
                }
                div.form-foot.end { button.btn type="submit" { "Create preset" } }
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

#[derive(Deserialize)]
struct TemplateForm {
    template: String,
}

async fn preset_create(State(app): State<AppState>, user: WebUser, Form(f): Form<TemplateForm>) -> WebResult {
    let p = app
        .create_preset(&user.access(), Some(&f.template), PresetInput::default())
        .await?;
    Ok(redirect(format!("/presets/{}", p.id)))
}

async fn preset_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
) -> WebResult {
    let access = user.access();
    let p = app.preset(&access, id).await?;
    let decks: Vec<String> = app
        .decks(&access)
        .await?
        .into_iter()
        .filter(|d| d.preset_id == id)
        .map(|d| d.name)
        .collect();
    Ok(layout(
        &p.name,
        Some(&user.0),
        html! {
            (flash(&msg))
            h1 { "Edit preset" }
            form method="post" action=(format!("/presets/{id}")) {
                div.sheet.form {
                    label.title { span.face-label { "Name" } input type="text" name="name" required value=(p.name); }
                    label.title { span.face-label { "Description" }
                        textarea name="description" rows="2" placeholder="When to use it" { (p.description) }
                    }
                    @if !decks.is_empty() { p.hint.muted.small { "Used by " (decks.iter().map(|d| deck_plain(d)).collect::<Vec<_>>().join(", ")) } }
                    hr.divider;
                    label.line { span { "Desired retention " span.muted { "(0.70–0.99)" } }
                        input type="number" name="desired_retention" min="0.7" max="0.99" step="0.01" required value=(format!("{:.2}", p.desired_retention));
                    }
                    label.line { "New cards per day" input type="number" name="new_per_day" min="0" required value=(p.new_per_day); }
                    label.line { "Maximum reviews per day" input type="number" name="reviews_per_day" min="0" required value=(p.reviews_per_day); }
                    label.line { span { "Learning steps " span.muted { "(e.g. 10m or 1m 10m)" } }
                        input type="text" name="learning_steps" value=(p.learning_steps);
                    }
                    label.line { "Relearning steps" input type="text" name="relearning_steps" value=(p.relearning_steps); }
                    label.line { "Maximum interval (days)" input type="number" name="max_interval" min="1" required value=(p.max_interval); }
                }
                div.form-foot.end { button.btn.primary type="submit" { "Save" } }
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
            p.muted.small { a href="/presets" { "All presets" } }
        },
    )
    .into_response())
}

#[derive(Deserialize)]
struct PresetForm {
    name: String,
    description: String,
    desired_retention: f64,
    new_per_day: i64,
    reviews_per_day: i64,
    learning_steps: String,
    relearning_steps: String,
    max_interval: i64,
}

async fn preset_update(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<PresetForm>,
) -> WebResult {
    app.update_preset(
        &user.access(),
        id,
        PresetInput {
            name: Some(f.name),
            description: Some(f.description),
            desired_retention: Some(f.desired_retention),
            new_per_day: Some(f.new_per_day),
            reviews_per_day: Some(f.reviews_per_day),
            learning_steps: Some(f.learning_steps),
            relearning_steps: Some(f.relearning_steps),
            max_interval: Some(f.max_interval),
        },
    )
    .await?;
    Ok(redirect_msg(format!("/presets/{id}"), "Preset saved."))
}

async fn preset_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    app.delete_preset(&user.access(), id).await?;
    Ok(redirect_msg("/presets", "Preset deleted."))
}

// ----- settings -----------------------------------------------------------

async fn settings_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let u = &user.0;
    let reviews = app.optimizable_reviews(&access).await?;
    let custom = !u.params().is_empty();
    Ok(layout(
        "Settings",
        Some(u),
        html! {
            (flash(&msg))
            h1 { "Settings" }
            p.muted { "Logged in as " strong { (u.handle()) } }
            ul.list.links {
                @for (href, name, sub) in [
                    ("/presets", "Presets", "How much to remember, daily limits"),
                    ("/tokens", "Agent access", "Connect Claude and other agents"),
                    ("/import", "Import", "From Anki or a CSV file"),
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
                    label.line { "New day starts at (hour)" input type="number" name="rollover_hour" min="0" max="23" value=(u.rollover_hour); }
                    label.line { "Text size (%)" input type="number" name="font_scale" min="80" max="200" step="10" value=(u.font_scale); }
                    label.check { input type="checkbox" name="four_buttons" value="1" checked[u.four_buttons]; " Show Hard and Easy buttons" }
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
                div.line { "Reviews so far" span.value { (reviews) } }
                @if reviews < 1000 {
                    p.hint.muted.small { "Optimizing works best after about 1,000 reviews. Re-run it every month or so." }
                }
            }
            div.form-foot.end {
                @if custom {
                    form method="post" action="/settings/reset-params" {
                        button.btn type="submit" { "Go back to defaults" }
                    }
                }
                form method="post" action="/settings/optimize" {
                    button.btn type="submit" { "Optimize now" }
                }
            }
            h2 { "Change password" }
            form method="post" action="/settings/password" {
                div.sheet.form {
                    label.line { "Current password" input type="password" name="current" required autocomplete="current-password"; }
                    label.line { span { "New password " span.muted { "(8 or more characters)" } } input type="password" name="new" required minlength="8" autocomplete="new-password"; }
                }
                div.form-foot.end { button.btn type="submit" { "Change password" } }
            }
            form.logout method="post" action="/logout" { button.btn type="submit" { "Log out" } }
        },
    )
    .into_response())
}

#[derive(Deserialize)]
struct SettingsForm {
    timezone: String,
    rollover_hour: i64,
    font_scale: i64,
    four_buttons: Option<String>,
    optimize_include_agent: Option<String>,
    desktop_look: Option<String>,
}

async fn settings_update(State(app): State<AppState>, user: WebUser, Form(f): Form<SettingsForm>) -> WebResult {
    app.update_settings(
        user.0.id,
        f.timezone.trim(),
        f.rollover_hour,
        f.four_buttons.is_some(),
        f.font_scale,
        f.optimize_include_agent.is_some(),
        f.desktop_look.as_deref().unwrap_or("calm"),
    )
    .await?;
    Ok(redirect_msg("/settings", "Settings saved."))
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
    app.change_password(&user.0, &f.current, &f.new).await?;
    let token = app.create_session(user.0.id).await?;
    Ok((
        jar.add(session_cookie(&app, token)).add(flash_cookie("Password changed.")),
        Redirect::to("/settings"),
    )
        .into_response())
}

async fn optimize(State(app): State<AppState>, user: WebUser) -> WebResult {
    let result = app.optimize_parameters(&user.access()).await;
    let msg = match result {
        Ok(v) => format!(
            "FSRS parameters fitted to {}.",
            count(v["reviews_used"].as_i64().unwrap_or(0), "review")
        ),
        Err(AppError::BadRequest(m)) => capitalize(&m),
        Err(e) => return Err(e.into()),
    };
    Ok(redirect_msg("/settings#fsrs", &msg))
}

async fn reset_params(State(app): State<AppState>, user: WebUser) -> WebResult {
    app.reset_parameters(&user.access()).await?;
    Ok(redirect_msg("/settings#fsrs", "FSRS parameters reset to the defaults."))
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

async fn tokens_page(
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
    Ok(layout(
        "Agent access",
        Some(&user.0),
        html! {
            (flash(&msg))
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
                    li.token {
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
                        form method="post" action=(format!("/tokens/{}/revoke", t.id)) {
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
            form method="post" action="/tokens" {
                label { "Name" input type="text" name="name" required placeholder="e.g. Claude"; }
                label.check { input type="checkbox" name="can_write" value="1" checked; " " (PERM_WRITE) }
                label.check { input type="checkbox" name="can_review" value="1" checked; " " (PERM_REVIEW) }
                fieldset {
                    legend { "Decks" }
                    label.check { input type="radio" name="scope" value="all" checked; " " (PERM_ALL_DECKS) }
                    label.check { input type="radio" name="scope" value="some"; " " (PERM_SOME_DECKS) }
                    @for d in &decks {
                        label.check.indent { input type="checkbox" name="decks" value=(d.id); " " (deck_plain(&d.name)) }
                    }
                }
                button.btn.primary type="submit" { "Create token" }
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
    scope: String,
    #[serde(default)]
    decks: Vec<i64>,
}

async fn token_create(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> WebResult {
    let decks = if f.scope == "all" { None } else { Some(f.decks) };
    let (token, raw) = app
        .create_token(user.0.id, &f.name, f.can_write.is_some(), f.can_review.is_some(), decks)
        .await?;
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
    Ok(layout(
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
            p { a.btn href="/tokens" { "Done" } }
        },
    )
    .into_response())
}

async fn token_revoke(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    app.revoke_token(user.0.id, id).await?;
    Ok(redirect_msg("/tokens", "Access removed."))
}

// ----- import -------------------------------------------------------------

async fn import_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let decks = app.decks(&user.access()).await?;
    Ok(layout(
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
                    (icon(ICON_IMPORT, 28))
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
                    div.form-foot.end { button.btn.primary type="submit" { "Import CSV" } }
                }
            }
        },
    )
    .into_response())
}

async fn import_upload(State(app): State<AppState>, user: WebUser, mut mp: Multipart) -> WebResult {
    let access = user.access();
    let mut fields: HashMap<String, Vec<u8>> = HashMap::new();
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| AppError::bad(format!("upload failed: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|e| AppError::bad(format!("upload failed: {e}")))?;
        fields.insert(name, data.to_vec());
    }
    let text = |k: &str| fields.get(k).map(|v| String::from_utf8_lossy(v).to_string()).unwrap_or_default();
    let report = match text("kind").as_str() {
        "apkg" => {
            let data = fields.remove("file").filter(|d| !d.is_empty()).ok_or_else(|| AppError::bad("choose a file"))?;
            app.import_apkg(&access, data).await?
        }
        "csv" => {
            let deck: i64 = text("deck").parse().map_err(|_| AppError::bad("pick a deck"))?;
            let mut body = text("file");
            if body.trim().is_empty() {
                body = text("text");
            }
            app.import_csv(&access, deck, &body).await?
        }
        _ => return Err(AppError::bad("unknown import type").into()),
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
    let deck = match q.deck {
        Some(id) => Some(app.deck(&access, id).await?),
        None => None,
    };
    let max = s.forecast.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    Ok(layout(
        "Stats",
        Some(&user.0),
        html! {
            h1 { "Stats" @if let Some(d) = &deck { ": " (deck_title(&d.name)) } }
            h2 { "Today" }
            dl.stats-inline {
                div { dt { (format::plural(s.cards_today, "Card")) " studied" } dd { (s.cards_today) } }
                div { dt { (format::plural(s.minutes_today.round() as i64, "Minute")) } dd { (s.minutes_today.round() as i64) } }
                div { dt { (format::plural(s.streak_days, "Day")) " in a row" } dd { (s.streak_days) } }
            }
            h2 { "Memory" }
            dl.kv {
                dt { "Reviews, last 30 days" } dd { (s.reviews_30d) }
                dt { "Recalled when due, last 30 days" } dd { (format::percent(s.true_retention_30d)) " (" (count(s.retention_sample_30d, "review")) ")" }
                dt { "Average recall chance now" } dd { (format::percent(s.average_retrievability)) }
            }
            h2 { "Cards" }
            dl.kv {
                dt { "New" } dd { (s.new) }
                dt { "Learning" } dd { (s.learning) }
                dt { "Review" } dd { (s.review) }
                dt { "Suspended" } dd { (s.suspended) }
                dt { "Total" } dd { (s.total) }
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
                            td.num { (n) }
                        }
                    }
                }
            }
        },
    )
    .into_response())
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
    let share = app.share_deck(&user.access(), id, &f.user).await?;
    Ok(redirect_msg(
        format!("/decks/{id}/edit#share"),
        &format!("Shared with {}. They'll see it on their home page.", share.to),
    ))
}

async fn share_action(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<ActionForm>,
) -> WebResult {
    let access = user.access();
    match f.action.as_str() {
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
        _ => Err(AppError::bad("unknown action").into()),
    }
}
