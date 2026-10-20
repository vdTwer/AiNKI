//! The website: server-rendered HTML, no JavaScript required, high contrast,
//! no animations, big tap targets. Designed for phones and e-ink readers.

use std::collections::HashMap;

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
    AppState,
    auth::{Access, User},
    error::AppError,
    import, markdown, mcp, media, now, oauth, presets,
    scheduler::{self, CardState, Rating},
    service::{CardFilter, CardInput, PresetInput, fmt_ts},
};

const COOKIE: &str = "ainki_session";
const CSS: &str = include_str!("style.css");

pub async fn serve(app: AppState) -> anyhow::Result<()> {
    let bind = app.config.bind.clone();
    let router = router(app);
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("AiNKI listening on http://{bind}");
    axum::serve(listener, router)
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
        .layer(middleware::from_fn_with_state(app.clone(), mcp::auth));
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
        .merge(mcp_routes)
        .layer(cors);
    Router::new()
        .route("/oauth/authorize", get(oauth::authorize_page).post(oauth::authorize_submit))
        .route("/offline", get(offline_page))
        .route("/offline.js", get(offline_js))
        .route("/sw.js", get(service_worker))
        .route("/api/offline/queue", get(offline_queue))
        .route("/api/offline/sync", post(offline_sync))
        .route("/decks/{id}/share", post(deck_share))
        .route("/shares/{id}", post(share_action))
        .route("/", get(home))
        .route("/style.css", get(style))
        .route("/healthz", get(|| async { "ok" }))
        .route("/login", get(login_page).post(login))
        .route("/signup", get(signup_page).post(signup))
        .route("/logout", post(logout))
        .route("/study", get(study))
        .route("/study/answer", post(study_answer))
        .route("/study/undo", post(study_undo))
        .route("/decks/new", get(deck_new_page).post(deck_create))
        .route("/decks/{id}", get(deck_page))
        .route("/decks/{id}/edit", get(deck_edit_page).post(deck_update))
        .route("/decks/{id}/delete", post(deck_delete))
        .route("/decks/{id}/export.csv", get(deck_export))
        .route("/cards/new", get(card_new_page).post(card_create))
        .route("/cards/{id}", get(card_page).post(card_update))
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
        .route("/import", get(import_page).post(import_upload))
        .route("/stats", get(stats_page))
        .route("/media/{id}", get(media_file))
        .merge(client_routes)
        .layer(DefaultBodyLimit::max(import::MAX_IMPORT_BYTES + 1024 * 1024))
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
    app.session_user(jar.get(COOKIE)?.value()).await.ok().flatten()
}

impl axum::extract::OptionalFromRequestParts<AppState> for WebUser {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Option<Self>, Self::Rejection> {
        Ok(session_from_parts(parts, app).await.map(WebUser))
    }
}

impl FromRequestParts<AppState> for WebUser {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        match session_from_parts(parts, app).await {
            Some(user) => Ok(WebUser(user)),
            None => Err(Redirect::to("/login").into_response()),
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
                p { a.btn href="javascript:history.back()" { "Go back" } " " a.btn href="/" { "Home" } }
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

pub fn layout(title: &str, user: Option<&User>, body: Markup) -> Markup {
    page(title, user, None, body)
}

/// `page_kind` marks special pages for the offline script.
fn page(title: &str, user: Option<&User>, page_kind: Option<&str>, body: Markup) -> Markup {
    let scale = user.map(|u| u.font_scale).unwrap_or(100) as f64 / 100.0;
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="light dark";
                title { (title) " · AiNKI" }
                link rel="stylesheet" href="/style.css";
                @if (scale - 1.0).abs() > f64::EPSILON {
                    style { (PreEscaped(format!(":root{{--scale:{scale}}}"))) }
                }
            }
            body data-user=[user.map(|u| u.id)] data-page=[page_kind] {
                @if let Some(u) = user {
                    nav.top {
                        a.brand href="/" { "AiNKI" }
                        a href="/study" { "Study" }
                        a href="/cards/new" { "Add" }
                        a href="/stats" { "Stats" }
                        a href="/settings" title=(u.handle()) { "Settings" }
                    }
                } @else if page_kind == Some("offline") {
                    nav.top {
                        a.brand href="/" { "AiNKI" }
                        a href="/offline" { "Study offline" }
                    }
                }
                main { (body) }
                script src="/offline.js" defer {}
            }
        }
    }
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
    Cookie::build((COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(app.config.secure_cookies)
        .max_age(time_days(crate::auth::SESSION_DAYS))
        .build()
}

fn time_days(days: i64) -> time::Duration {
    time::Duration::days(days)
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

#[derive(Deserialize, Default)]
struct MsgQuery {
    msg: Option<String>,
}

// ----- auth pages ---------------------------------------------------------

/// Only same-site paths are allowed as a post-login target.
fn safe_next(next: Option<&str>) -> Option<&str> {
    next.filter(|n| n.starts_with('/') && !n.starts_with("//") && !n.starts_with("/\\"))
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
            form method="post" action=(action) data-clear-offline {
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
                    script { (PreEscaped("try{document.getElementById('tz').value=Intl.DateTimeFormat().resolvedOptions().timeZone}catch(e){}")) }
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
    auth_form(false, None, app.config.allow_signup, next).into_response()
}

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    #[serde(default)]
    timezone: String,
    next: Option<String>,
}

async fn login(State(app): State<AppState>, jar: CookieJar, Form(f): Form<LoginForm>) -> Response {
    let next = safe_next(f.next.as_deref());
    match app.authenticate(&f.username, &f.password).await {
        Ok(user) => match app.create_session(user.id).await {
            Ok(token) => (jar.add(session_cookie(&app, token)), Redirect::to(next.unwrap_or("/"))).into_response(),
            Err(e) => WebError(e).into_response(),
        },
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            auth_form(false, Some(&capitalize(&e.to_string())), app.config.allow_signup, next),
        )
            .into_response(),
    }
}

async fn signup_page(State(app): State<AppState>, Query(q): Query<NextQuery>) -> Response {
    if !app.config.allow_signup {
        return redirect("/login");
    }
    auth_form(true, None, true, safe_next(q.next.as_deref())).into_response()
}

async fn signup(State(app): State<AppState>, jar: CookieJar, Form(f): Form<LoginForm>) -> Response {
    if !app.config.allow_signup {
        return redirect("/login");
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
            let welcome = format!(
                "/?msg={}",
                urlenc(&format!("Welcome! Your username is {}. Use it to log in.", user.handle()))
            );
            (jar.add(session_cookie(&app, token)), Redirect::to(next.unwrap_or(&welcome))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            auth_form(true, Some(&capitalize(&e.to_string())), true, next),
        )
            .into_response(),
    }
}

async fn logout(State(app): State<AppState>, jar: CookieJar) -> Response {
    if let Some(c) = jar.get(COOKIE) {
        let _ = app.delete_session(c.value()).await;
    }
    (jar.remove(Cookie::build(COOKIE).path("/")), Redirect::to("/login")).into_response()
}

// ----- home ---------------------------------------------------------------

async fn home(State(app): State<AppState>, user: WebUser, Query(q): Query<MsgQuery>) -> WebResult {
    let access = user.access();
    let decks = app.deck_summaries(&access).await?;
    let shares = app.incoming_shares(&access).await?;
    let total_due: i64 = decks.iter().map(|d| d.due_total()).sum();
    Ok(page(
        "Decks",
        Some(&user.0),
        Some("home"),
        html! {
            (flash(&q.msg))
            @if total_due > 0 {
                a.btn.primary.wide href="/study" { "Study all · " (total_due) " due" }
            } @else if !decks.is_empty() {
                p.notice { "All done for today." }
            }
            @if !decks.is_empty() {
                p.small.offline-line {
                    a href="/offline" { "Study offline" }
                    " " span.muted id="offline-status" {}
                }
            }
            @if !shares.is_empty() {
                h2 { "Shared with you" }
                ul.list {
                    @for sh in &shares {
                        li.token {
                            div.grow {
                                strong { (sh.deck_name) } br;
                                span.small { (sh.cards) (if sh.cards == 1 { " card" } else { " cards" }) " from " (sh.from) }
                            }
                            form method="post" action=(format!("/shares/{}", sh.id)) {
                                button.btn.primary type="submit" name="action" value="accept" { "Add" }
                                " "
                                button.btn type="submit" name="action" value="decline" { "No thanks" }
                            }
                        }
                    }
                }
                p.muted.small { "Adding gives you your own copy. Your progress is yours; later changes by the sender don't carry over." }
            }
            h1 { "Decks" }
            @if decks.is_empty() {
                p { "No decks yet. Create one, or import from Anki or a CSV file." }
            }
            ul.list {
                @for d in &decks {
                    li {
                        a.row href=(format!("/decks/{}", d.deck.id)) {
                            span.grow { (d.deck.name) }
                            span.counts {
                                @if d.due_total() > 0 {
                                    span title="new" { (d.new_today) " new" }
                                    " · "
                                    span title="due" { (d.learning_due + d.review_due) " due" }
                                } @else {
                                    span.muted { "done" }
                                }
                            }
                        }
                    }
                }
            }
            p.actions {
                a.btn href="/decks/new" { "New deck" }
                a.btn href="/import" { "Import" }
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
    msg: Option<String>,
}

fn study_url(deck: Option<i64>) -> String {
    match deck {
        Some(d) => format!("/study?deck={d}"),
        None => "/study".into(),
    }
}

async fn study(State(app): State<AppState>, user: WebUser, Query(q): Query<StudyQuery>) -> WebResult {
    let access = user.access();
    let deck_param = q.deck.map(|d| format!("&deck={d}")).unwrap_or_default();

    // Answer side of a specific card.
    if let (Some(card_id), Some(1)) = (q.card, q.show) {
        let card = app.card(&access, card_id).await?;
        let deck = app.deck(&access, card.deck_id).await?;
        let previews = app.previews(&access, &card, &deck).await?;
        let four = user.0.four_buttons;
        let shown = q.t.unwrap_or_else(now);
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
        return Ok(layout(
            "Study",
            Some(&user.0),
            html! {
                p.muted.small { (deck.name) }
                section.face { (face(&card.front, &card.front_image)) }
                hr.divider;
                section.face.back id="answer" { (face(&card.back, &card.back_image)) }
                form.answers.cols-2[!four].cols-4[four] method="post" action="/study/answer" {
                    input type="hidden" name="card" value=(card.id);
                    input type="hidden" name="reps" value=(card.reps);
                    input type="hidden" name="t" value=(shown);
                    @if let Some(d) = q.deck { input type="hidden" name="deck" value=(d); }
                    @for (rating, label, key) in &buttons {
                        @let preview = previews.iter().find(|(r, _)| r == rating).map(|(_, p)| p.as_str()).unwrap_or("");
                        button.btn.primary[*rating == Rating::Good] type="submit" name="rating" value=(*rating as i64) data-key=(key) {
                            span.label { (label) }
                            span.ivl { (preview) }
                        }
                    }
                }
                (shortcuts())
            },
        )
        .into_response());
    }

    let Some(sc) = app.next_card(&access, q.deck).await? else {
        let decks = app.deck_summaries(&access).await?;
        let any_cards = decks.iter().any(|d| d.total > 0);
        return Ok(layout(
            "Study",
            Some(&user.0),
            html! {
                (flash(&q.msg))
                h1 { "Done for now" }
                @if any_cards {
                    p { "Nothing else is due. Come back later." }
                } @else {
                    p { "There are no cards yet." }
                    p { a.btn.primary href=(match q.deck { Some(d) => format!("/cards/new?deck={d}"), None => "/cards/new".into() }) { "Add cards" } }
                }
                form method="post" action="/study/undo" {
                    @if let Some(d) = q.deck { input type="hidden" name="deck" value=(d); }
                    button.btn type="submit" { "Undo last answer" }
                }
                p { a href="/" { "Back to decks" } }
            },
        )
        .into_response());
    };
    let card = &sc.card;
    let state_label = match card.card_state() {
        CardState::New => "new",
        CardState::Learning | CardState::Relearning => "learning",
        CardState::Review => "review",
    };
    Ok(layout(
        "Study",
        Some(&user.0),
        html! {
            (flash(&q.msg))
            p.muted.small.status {
                span { (sc.deck.name) }
                span { (state_label) " · " (sc.remaining) " left" }
            }
            section.face { (face(&card.front, &card.front_image)) }
            a.btn.primary.wide.show data-key=" " href=(format!("/study?card={}&show=1&t={}{}#answer", card.id, now(), deck_param)) {
                "Show answer"
            }
            div.study-tools {
                form method="post" action="/study/undo" {
                    @if let Some(d) = q.deck { input type="hidden" name="deck" value=(d); }
                    button.link type="submit" { "Undo" }
                }
                a href=(format!("/cards/{}", card.id)) { "Edit card" }
            }
            (shortcuts())
        },
    )
    .into_response())
}

/// Optional keyboard shortcuts for devices with keyboards. Everything works
/// without it.
fn shortcuts() -> Markup {
    html! {
        script {
            (PreEscaped(r#"document.addEventListener('keydown',function(e){if(e.target.tagName==='INPUT'||e.target.tagName==='TEXTAREA')return;var k=e.key===' '||e.key==='Enter'?' ':e.key;var el=document.querySelector('[data-key="'+k+'"]');if(!el&&k===' ')el=document.querySelector('[data-key="3"]');if(el){e.preventDefault();el.click();}});"#))
        }
    }
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
        None => redirect(format!("{base}{sep}msg={}", urlenc("Nothing to undo."))),
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

async fn deck_new_page(State(app): State<AppState>, user: WebUser) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout(
        "New deck",
        Some(&user.0),
        html! {
            h1 { "New deck" }
            form method="post" action="/decks/new" {
                label { "Name" input type="text" name="name" required autofocus; }
                label { "Description " span.muted { "(optional)" } textarea name="description" rows="2" {} }
                label { "Preset" (preset_select(&presets, presets.first().map(|p| p.id))) }
                p.muted.small { "Presets set how much you want to remember and your daily limits. " a href="/presets" { "About presets" } }
                button.btn.primary type="submit" { "Create deck" }
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
    Ok(redirect(format!("/decks/{}", deck.id)))
}

#[derive(Deserialize, Default)]
struct DeckQuery {
    q: Option<String>,
    state: Option<String>,
    page: Option<i64>,
    msg: Option<String>,
}

async fn deck_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
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
        &d.name,
        Some(&user.0),
        html! {
            (flash(&q.msg))
            h1 { (d.name) }
            @if !d.description.is_empty() { p { (d.description) } }
            dl.stats-inline {
                div { dt { "New today" } dd { (summary.new_today) } }
                div { dt { "Due" } dd { (summary.learning_due + summary.review_due) } }
                div { dt { "Cards" } dd { (summary.total) } }
            }
            @if let Some(exam) = &d.exam_date { p.muted { "Exam date: " (exam) } }
            @if summary.due_total() > 0 {
                a.btn.primary.wide href=(format!("/study?deck={id}")) { "Study" }
            }
            p.actions {
                a.btn href=(format!("/cards/new?deck={id}")) { "Add card" }
                a.btn href=(format!("/decks/{id}/edit")) { "Edit deck" }
                a.btn href=(format!("/decks/{id}/edit#share")) { "Share" }
                a.btn href=(format!("/stats?deck={id}")) { "Stats" }
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
            p.muted.small { (total) " card(s)" }
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
                                @if c.suspended { "suspended" } @else { (c.card_state().as_str()) }
                            }
                        }
                    }
                }
            }
            p.actions {
                @if page > 0 { a.btn href=(format!("{base}&page={}", page - 1)) { "Previous" } }
                @if (page + 1) * per_page < total { a.btn href=(format!("{base}&page={}", page + 1)) { "Next" } }
            }
            p { a href=(format!("/decks/{id}/export.csv")) { "Export as CSV" } }
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
    Query(q): Query<MsgQuery>,
) -> WebResult {
    let access = user.access();
    let deck = app.deck(&access, id).await?;
    let presets = app.list_presets(&access).await?;
    let shares = app.outgoing_shares(&access, id).await?;
    Ok(layout(
        "Edit deck",
        Some(&user.0),
        html! {
            (flash(&q.msg))
            h1 { "Edit deck" }
            form method="post" action=(format!("/decks/{id}/edit")) {
                label { "Name" input type="text" name="name" required value=(deck.name); }
                label { "Description" textarea name="description" rows="2" { (deck.description) } }
                label { "Preset" (preset_select(&presets, Some(deck.preset_id))) }
                label {
                    "Exam date " span.muted { "(optional)" }
                    input type="date" name="exam_date" value=(deck.exam_date.clone().unwrap_or_default());
                }
                p.muted.small { "With an exam date, no card in this deck is scheduled after the exam." }
                button.btn.primary type="submit" { "Save" }
            }
            h2 id="share" { "Share" }
            p { "Send a copy of this deck to someone. They get the cards with fresh progress; your deck stays as it is." }
            form method="post" action=(format!("/decks/{id}/share")) {
                label { "Username" span.muted { " (name#1234)" }
                    input type="text" name="user" required autocapitalize="none" autocorrect="off" placeholder="name#1234";
                }
                button.btn type="submit" { "Share" }
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
            h2 { "Delete" }
            form method="post" action=(format!("/decks/{id}/delete")) {
                label.check { input type="checkbox" name="confirm" value="1" required; " Delete this deck and all its cards" }
                button.btn.danger type="submit" { "Delete deck" }
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
    Ok(redirect(format!("/decks/{id}?msg=Saved.")))
}

async fn deck_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    let n = app.delete_deck(&user.access(), id).await?;
    Ok(redirect(format!("/?msg={}", urlenc(&format!("Deck deleted ({n} cards).")))))
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
            "deck" => f.deck = text().parse().ok(),
            "front" => f.front = text(),
            "back" => f.back = text(),
            "front_image" if !data.is_empty() => f.front_upload = Some(data.to_vec()),
            "back_image" if !data.is_empty() => f.back_upload = Some(data.to_vec()),
            "remove_front_image" => f.remove_front_image = true,
            "remove_back_image" => f.remove_back_image = true,
            _ => {}
        }
    }
    if f.front_upload.as_ref().is_some_and(|d| d.len() > media::MAX_UPLOAD_BYTES)
        || f.back_upload.as_ref().is_some_and(|d| d.len() > media::MAX_UPLOAD_BYTES)
    {
        return Err(AppError::bad("images must be smaller than 15 MB"));
    }
    Ok(f)
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
) -> Markup {
    html! {
        form method="post" action=(action) enctype="multipart/form-data" {
            label { "Deck"
                select name="deck" required {
                    @for d in decks { option value=(d.id) selected[Some(d.id) == deck_id] { (d.name) } }
                }
            }
            label { "Front" textarea name="front" rows="4" autofocus { (front) } }
            @if let Some(img) = front_image {
                img.thumb src=(format!("/media/{img}")) alt="Front image";
                label.check { input type="checkbox" name="remove_front_image" value="1"; " Remove image" }
            }
            label.file { "Front image " span.muted { "(optional)" } input type="file" name="front_image" accept="image/*"; }
            label { "Back" textarea name="back" rows="4" { (back) } }
            @if let Some(img) = back_image {
                img.thumb src=(format!("/media/{img}")) alt="Back image";
                label.check { input type="checkbox" name="remove_back_image" value="1"; " Remove image" }
            }
            label.file { "Back image " span.muted { "(optional)" } input type="file" name="back_image" accept="image/*"; }
            p.muted.small { "Text supports Markdown: **bold**, *italic*, lists, `code`." }
            button.btn.primary.wide type="submit" { (submit) }
        }
    }
}

#[derive(Deserialize)]
struct NewCardQuery {
    deck: Option<i64>,
    msg: Option<String>,
}

async fn card_new_page(State(app): State<AppState>, user: WebUser, Query(q): Query<NewCardQuery>) -> WebResult {
    let decks = app.decks(&user.access()).await?;
    if decks.is_empty() {
        return Ok(redirect("/decks/new"));
    }
    let deck = q.deck.or_else(|| decks.first().map(|d| d.id));
    Ok(layout(
        "Add card",
        Some(&user.0),
        html! {
            (flash(&q.msg))
            h1 { "Add card" }
            (card_form("/cards/new", &decks, deck, "", "", &None, &None, "Add"))
        },
    )
    .into_response())
}

async fn card_create(State(app): State<AppState>, user: WebUser, mp: Multipart) -> WebResult {
    let access = user.access();
    let f = read_card_form(mp).await?;
    let deck = f.deck.ok_or_else(|| AppError::bad("pick a deck"))?;
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
    Ok(redirect(format!("/cards/new?deck={deck}&msg=Added.")))
}

async fn card_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(q): Query<MsgQuery>,
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
            (flash(&q.msg))
            h1 { "Edit card" }
            (card_form(&format!("/cards/{id}"), &decks, Some(card.deck_id), &card.front, &card.back, &card.front_image, &card.back_image, "Save"))
            h2 { "Scheduling" }
            dl.kv {
                dt { "State" } dd { (card.card_state().as_str()) @if card.suspended { " (suspended)" } }
                @if card.card_state() != CardState::New {
                    dt { "Due" } dd { (fmt_ts(card.due, &clock)) }
                }
                @if let Some(s) = card.stability { dt { "Stability" } dd { (format!("{s:.1} days")) } }
                @if let Some(d) = card.difficulty { dt { "Difficulty" } dd { (format!("{d:.1} / 10")) } }
                @if let Some(r) = r { dt { "Recall chance now" } dd { (format!("{:.0}%", r * 100.0)) } }
                dt { "Reviews / lapses" } dd { (card.reps) " / " (card.lapses) }
                dt { "Added by" } dd { (card.created_by) }
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
                                td { (h["at"].as_str().unwrap_or("")) }
                                td { (h["rating"].as_str().unwrap_or("")) @if h["source"] == "agent" { " (agent)" } }
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
    Ok(redirect(format!("/cards/{id}?msg=Saved.")))
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
            "Suspended."
        }
        "unsuspend" => {
            app.set_suspended(&access, id, false).await?;
            "Unsuspended."
        }
        "reset" => {
            app.reset_card(&access, id).await?;
            "Progress reset; the card is new again."
        }
        "delete" => {
            let card = app.card(&access, id).await?;
            app.delete_card(&access, id).await?;
            return Ok(redirect(format!("/decks/{}?msg=Card%20deleted.", card.deck_id)));
        }
        _ => return Err(AppError::bad("unknown action").into()),
    };
    Ok(redirect(format!("/cards/{id}?msg={}", urlenc(msg))))
}

// ----- presets ------------------------------------------------------------

async fn presets_page(State(app): State<AppState>, user: WebUser, Query(q): Query<MsgQuery>) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout(
        "Presets",
        Some(&user.0),
        html! {
            (flash(&q.msg))
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
                label { "Start from"
                    select name="template" {
                        @for t in presets::TEMPLATES { option value=(t.key) { (t.name) } }
                    }
                }
                button.btn type="submit" { "Create" }
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
    Query(q): Query<MsgQuery>,
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
            (flash(&q.msg))
            h1 { (p.name) }
            @if !decks.is_empty() { p.muted { "Used by: " (decks.join(", ")) } }
            form method="post" action=(format!("/presets/{id}")) {
                label { "Name" input type="text" name="name" required value=(p.name); }
                label { "Description" textarea name="description" rows="2" { (p.description) } }
                label { "Desired retention (0.70–0.99)"
                    input type="number" name="desired_retention" min="0.7" max="0.99" step="0.01" required value=(format!("{:.2}", p.desired_retention));
                }
                label { "New cards per day" input type="number" name="new_per_day" min="0" required value=(p.new_per_day); }
                label { "Maximum reviews per day" input type="number" name="reviews_per_day" min="0" required value=(p.reviews_per_day); }
                label { "Learning steps " span.muted { "(e.g. 10m or 1m 10m)" }
                    input type="text" name="learning_steps" value=(p.learning_steps);
                }
                label { "Relearning steps" input type="text" name="relearning_steps" value=(p.relearning_steps); }
                label { "Maximum interval (days)" input type="number" name="max_interval" min="1" required value=(p.max_interval); }
                button.btn.primary type="submit" { "Save" }
            }
            @if decks.is_empty() {
                form method="post" action=(format!("/presets/{id}/delete")) {
                    button.btn.danger type="submit" { "Delete preset" }
                }
            }
            p { a href="/presets" { "All presets" } }
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
    Ok(redirect(format!("/presets/{id}?msg=Saved.")))
}

async fn preset_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    app.delete_preset(&user.access(), id).await?;
    Ok(redirect("/presets?msg=Deleted."))
}

// ----- settings -----------------------------------------------------------

async fn settings_page(State(app): State<AppState>, user: WebUser, Query(q): Query<MsgQuery>) -> WebResult {
    let access = user.access();
    let u = &user.0;
    let reviews = app.optimizable_reviews(&access).await?;
    let custom = !u.params().is_empty();
    Ok(layout(
        "Settings",
        Some(u),
        html! {
            (flash(&q.msg))
            h1 { "Settings" }
            p { "Logged in as " strong { (u.handle()) } }
            p.actions {
                a.btn href="/presets" { "Presets" }
                a.btn href="/tokens" { "Agent access" }
                a.btn href="/import" { "Import" }
            }
            h2 { "Preferences" }
            form method="post" action="/settings" {
                label { "Timezone"
                    input type="text" name="timezone" value=(u.timezone) list="tzs" required autocapitalize="none";
                    datalist id="tzs" { @for tz in chrono_tz::TZ_VARIANTS.iter() { option value=(tz.name()); } }
                }
                label { "New day starts at (hour)" input type="number" name="rollover_hour" min="0" max="23" value=(u.rollover_hour); }
                label { "Text size (%)" input type="number" name="font_scale" min="80" max="200" step="10" value=(u.font_scale); }
                label.check { input type="checkbox" name="four_buttons" value="1" checked[u.four_buttons]; " Show Hard and Easy buttons" }
                label.check { input type="checkbox" name="optimize_include_agent" value="1" checked[u.optimize_include_agent]; " Use reviews done through agents when optimizing" }
                button.btn.primary type="submit" { "Save" }
            }
            h2 id="fsrs" { "FSRS" }
            p {
                @if custom {
                    "Using parameters fitted to your reviews"
                    @if let Some(t) = u.fsrs_params_updated_at { " (" (fmt_ts(t, &u.clock())) ")" }
                    "."
                } @else {
                    "Using the FSRS-6 default parameters."
                }
                " You have " (reviews) " reviews."
            }
            @if reviews < 1000 {
                p.muted.small { "Optimizing works best after about 1,000 reviews. Re-run it every month or so." }
            }
            form.actions method="post" action="/settings/optimize" {
                button.btn type="submit" { "Optimize now" }
            }
            @if custom {
                form method="post" action="/settings/reset-params" {
                    button.btn type="submit" { "Go back to defaults" }
                }
            }
            h2 { "Change password" }
            form method="post" action="/settings/password" {
                label { "Current password" input type="password" name="current" required autocomplete="current-password"; }
                label { "New password" input type="password" name="new" required minlength="8" autocomplete="new-password"; }
                button.btn type="submit" { "Change password" }
            }
            form method="post" action="/logout" { button.btn type="submit" { "Log out" } }
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
}

async fn settings_update(State(app): State<AppState>, user: WebUser, Form(f): Form<SettingsForm>) -> WebResult {
    app.update_settings(
        user.0.id,
        f.timezone.trim(),
        f.rollover_hour,
        f.four_buttons.is_some(),
        f.font_scale,
        f.optimize_include_agent.is_some(),
    )
    .await?;
    Ok(redirect("/settings?msg=Saved."))
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
        jar.add(session_cookie(&app, token)),
        Redirect::to("/settings?msg=Password%20changed."),
    )
        .into_response())
}

async fn optimize(State(app): State<AppState>, user: WebUser) -> WebResult {
    let result = app.optimize_parameters(&user.access()).await;
    let msg = match result {
        Ok(v) => format!(
            "Done. FSRS learned from {} reviews.",
            v["reviews_used"].as_i64().unwrap_or(0)
        ),
        Err(AppError::BadRequest(m)) => capitalize(&m),
        Err(e) => return Err(e.into()),
    };
    Ok(redirect(format!("/settings?msg={}#fsrs", urlenc(&msg))))
}

async fn reset_params(State(app): State<AppState>, user: WebUser) -> WebResult {
    app.reset_parameters(&user.access()).await?;
    Ok(redirect("/settings?msg=Using%20default%20parameters.#fsrs"))
}

// ----- tokens -------------------------------------------------------------

fn mcp_url(app: &AppState, headers: &HeaderMap) -> String {
    format!("{}/mcp", oauth::base_url(app, headers))
}

async fn tokens_page(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Query(q): Query<MsgQuery>,
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
            (flash(&q.msg))
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
                                @if t.all_decks { "all decks" } @else { (deck_names.join(", ")) }
                            }
                            br;
                            span.muted.small {
                                "Last used: "
                                (t.last_used_at.map(|ts| fmt_ts(ts, &clock)).unwrap_or_else(|| "never".into()))
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
                label.check { input type="checkbox" name="can_write" value="1" checked; " Can add, edit and delete" }
                label.check { input type="checkbox" name="can_review" value="1" checked; " Can review cards (quiz me)" }
                fieldset {
                    legend { "Decks" }
                    label.check { input type="radio" name="scope" value="all" checked; " All decks (also lets it create decks and change presets)" }
                    label.check { input type="radio" name="scope" value="some"; " Only these:" }
                    @for d in &decks {
                        label.check.indent { input type="checkbox" name="decks" value=(d.id); " " (d.name) }
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
    Ok(redirect("/tokens?msg=Access%20removed."))
}

// ----- import -------------------------------------------------------------

async fn import_page(State(app): State<AppState>, user: WebUser, Query(q): Query<MsgQuery>) -> WebResult {
    let decks = app.decks(&user.access()).await?;
    Ok(layout(
        "Import",
        Some(&user.0),
        html! {
            (flash(&q.msg))
            h1 { "Import" }
            h2 { "Anki package" }
            p { "Upload an " code { ".apkg" } " or " code { ".colpkg" } " file. Each card keeps the first two fields of its note, the first image of each side and its review history. Cloze notes become one card per gap." }
            form method="post" action="/import" enctype="multipart/form-data" {
                input type="hidden" name="kind" value="apkg";
                label.file { "File" input type="file" name="file" accept=".apkg,.colpkg" required; }
                button.btn.primary type="submit" { "Import Anki package" }
            }
            h2 { "CSV" }
            p { "One card per line: front, back. Commas, semicolons or tabs work. A header line \"front,back\" is skipped." }
            @if decks.is_empty() {
                p { a href="/decks/new" { "Create a deck first." } }
            } @else {
                form method="post" action="/import" enctype="multipart/form-data" {
                    input type="hidden" name="kind" value="csv";
                    label { "Into deck"
                        select name="deck" { @for d in &decks { option value=(d.id) { (d.name) } } }
                    }
                    label.file { "File" input type="file" name="file" accept=".csv,.tsv,.txt"; }
                    label { "…or paste" textarea name="text" rows="5" placeholder="Hund,dog\nKatze,cat" {} }
                    button.btn.primary type="submit" { "Import CSV" }
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
    let mut msg = format!("Imported {} card(s)", report.cards_imported);
    if report.reviews_imported > 0 {
        msg += &format!(", {} review(s)", report.reviews_imported);
    }
    if report.images_imported > 0 {
        msg += &format!(", {} image(s)", report.images_imported);
    }
    if !report.decks_created.is_empty() {
        msg += &format!(". New decks: {}", report.decks_created.join(", "));
    }
    if report.skipped > 0 {
        msg += &format!(". Skipped {} empty", report.skipped);
    }
    msg.push('.');
    Ok(redirect(format!("/?msg={}", urlenc(&msg))))
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
    let pct = |v: Option<f64>| v.map(|v| format!("{:.1}%", v * 100.0)).unwrap_or_else(|| "–".into());
    Ok(layout(
        "Stats",
        Some(&user.0),
        html! {
            h1 { "Stats" @if let Some(d) = &deck { ": " (d.name) } }
            h2 { "Today" }
            dl.stats-inline {
                div { dt { "Reviews" } dd { (s.reviews_today) } }
                div { dt { "Minutes" } dd { (s.minutes_today) } }
                div { dt { "Streak" } dd { (s.streak_days) " d" } }
            }
            h2 { "Memory" }
            dl.kv {
                dt { "Reviews, last 30 days" } dd { (s.reviews_30d) }
                dt { "Recalled when due" } dd { (pct(s.true_retention_30d)) " (" (s.retention_sample_30d) " reviews)" }
                dt { "Average recall chance now" } dd { (pct(s.average_retrievability)) }
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
                            th scope="row" { (day) }
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
    Ok(redirect(format!(
        "/decks/{id}/edit?msg={}#share",
        urlenc(&format!("Shared with {}. They'll see it on their home page.", share.to))
    )))
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
            Ok(redirect(format!("/decks/{}?msg={}", deck.id, urlenc("Added to your decks."))))
        }
        "decline" => {
            app.decline_share(&access, id).await?;
            Ok(redirect("/"))
        }
        "cancel" => {
            app.cancel_share(&access, id).await?;
            Ok(redirect("/"))
        }
        _ => Err(AppError::bad("unknown action").into()),
    }
}

// ----- offline ------------------------------------------------------------

const OFFLINE_JS: &str = include_str!("static/offline.js");
const SW_JS: &str = include_str!("static/sw.js");

async fn offline_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        OFFLINE_JS,
    )
}

async fn service_worker() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        SW_JS,
    )
}

/// The offline study page. It is the same for everyone (the service worker
/// caches it); the script fills it from the downloaded queue.
async fn offline_page() -> impl IntoResponse {
    let body = page(
        "Study offline",
        None,
        Some("offline"),
        html! {
            p.small.muted id="o-conn" {}
            noscript { p.notice { "Offline studying needs JavaScript. " a href="/study" { "Study online" } } }
            p.notice id="o-msg" hidden {}
            div id="o-study" hidden {
                p.muted.small.status { span id="o-deck" {} span id="o-left" {} }
                section.face id="o-front" {}
                a.btn.primary.wide.show id="o-show" href="#" { "Show answer" }
                div id="o-backwrap" hidden {
                    hr.divider;
                    section.face.back id="o-back" {}
                    div.answers.cols-2 id="o-answers" {}
                }
                div.study-tools {
                    button.link type="button" id="o-undo" { "Undo" }
                    a href="/" { "Home" }
                }
            }
        },
    );
    ([(header::CACHE_CONTROL, "no-cache")], body)
}

async fn offline_queue(State(app): State<AppState>, user: Option<WebUser>) -> Response {
    let Some(user) = user else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.offline_queue(&user.access()).await {
        Ok(mut v) => {
            v["font_scale"] = serde_json::json!(user.0.font_scale);
            ([(header::CACHE_CONTROL, "no-store")], axum::Json(v)).into_response()
        }
        Err(e) => WebError(e).into_response(),
    }
}

#[derive(Deserialize)]
struct SyncRequest {
    answers: Vec<crate::offline::OfflineAnswer>,
}

async fn offline_sync(
    State(app): State<AppState>,
    user: Option<WebUser>,
    axum::Json(req): axum::Json<SyncRequest>,
) -> Response {
    let Some(user) = user else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    match app.offline_sync(&user.access(), req.answers).await {
        Ok(results) => axum::Json(serde_json::json!({ "results": results })).into_response(),
        Err(e) => WebError(e).into_response(),
    }
}
