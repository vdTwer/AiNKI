//! Signing in, signing up and signing out, with their rate limits.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

/// Only same-site paths are allowed as a post-login target.
pub(crate) fn safe_next(next: Option<&str>) -> Option<&str> {
    // Browsers ignore tabs/newlines and treat "\" like "/" in URLs, so
    // "/\t/evil.example" would become "//evil.example".
    next.filter(|n| {
        n.starts_with('/')
            && n.len() < 4096
            && !n
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
            && !n.starts_with("//")
    })
}

pub(crate) fn auth_form(
    signup: bool,
    error: Option<&str>,
    allow_signup: bool,
    next: Option<&str>,
) -> Markup {
    let title = if signup { "Sign up" } else { "Log in" };
    let action = if signup { "/signup" } else { "/login" };
    let next_q =
        next.map_or_else(Default::default, |n| format!("?next={}", urlenc(n)));
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
pub(crate) struct NextQuery {
    pub(crate) next: Option<String>,
}

pub(crate) async fn login_page(
    State(app): State<AppState>,
    user: Option<WebUser>,
    Query(q): Query<NextQuery>,
) -> Response {
    let next = safe_next(q.next.as_deref());
    if user.is_some() {
        return redirect(next.unwrap_or("/"));
    }
    auth_form(false, None, app.signup_open().await, next).into_response()
}

pub(crate) const LOGIN_WINDOW: Duration = Duration::from_mins(15);
/// Failed logins allowed per client IP, and per account, per window.
pub(crate) const LOGIN_FAILURES_PER_IP: u32 = 20;
pub(crate) const LOGIN_FAILURES_PER_ACCOUNT: u32 = 50;

#[derive(Deserialize)]
pub(crate) struct LoginForm {
    pub(crate) username: String,
    pub(crate) password: String,
    #[serde(default)]
    pub(crate) timezone: String,
    pub(crate) next: Option<String>,
}

pub(crate) async fn login(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(f): Form<LoginForm>,
) -> Response {
    let next = safe_next(f.next.as_deref());
    let ip_key = format!("login-ip:{ip}");
    let user_key = format!("login-user:{}", f.username.trim().to_lowercase());
    if app.limiter.exceeded(&ip_key, LOGIN_FAILURES_PER_IP, LOGIN_WINDOW)
        || app.limiter.exceeded(
            &user_key,
            LOGIN_FAILURES_PER_ACCOUNT,
            LOGIN_WINDOW,
        )
    {
        tracing::warn!("login throttled for {ip} / {}", f.username.trim());
        let page = auth_form(
            false,
            Some("Too many failed attempts. Wait 15 minutes and try again."),
            app.signup_open().await,
            next,
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "900")],
            page,
        )
            .into_response();
    }
    match app.authenticate(&f.username, &f.password).await {
        Ok(user) => match app.create_session(user.id).await {
            Ok(token) => (
                jar.add(session_cookie(&app, token)),
                Redirect::to(next.unwrap_or("/")),
            )
                .into_response(),
            Err(e) => WebError(e).into_response(),
        },
        Err(e) => {
            app.limiter.hit(&ip_key, LOGIN_WINDOW);
            app.limiter.hit(&user_key, LOGIN_WINDOW);
            (
                StatusCode::UNAUTHORIZED,
                auth_form(
                    false,
                    Some(&capitalize(&e.to_string())),
                    app.signup_open().await,
                    next,
                ),
            )
                .into_response()
        }
    }
}

pub(crate) async fn signup_page(
    State(app): State<AppState>,
    Query(q): Query<NextQuery>,
) -> Response {
    if !app.signup_open().await {
        return redirect("/login");
    }
    auth_form(true, None, true, safe_next(q.next.as_deref())).into_response()
}

pub(crate) async fn signup(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Form(f): Form<LoginForm>,
) -> Response {
    if !app.signup_open().await {
        return redirect("/login");
    }
    if app.limiter.check_and_hit(
        &format!("signup-ip:{ip}"),
        10,
        Duration::from_hours(1),
    ) {
        return security::too_many_requests(3600);
    }
    let next = safe_next(f.next.as_deref());
    let result = async {
        let user =
            app.create_user(&f.username, &f.password, &f.timezone).await?;
        let token = app.create_session(user.id).await?;
        Ok::<_, AppError>((user, token))
    }
    .await;
    match result {
        Ok((user, token)) => {
            let welcome = format!(
                "Welcome! Your username is {}. Use it to log in.",
                user.handle()
            );
            (
                jar.add(session_cookie(&app, token))
                    .add(flash_cookie(&welcome)),
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

pub(crate) async fn logout(
    State(app): State<AppState>,
    jar: CookieJar,
) -> Response {
    let name = app.config.session_cookie_name();
    if let Some(c) = jar.get(name) {
        let _ = app.delete_session(c.value()).await;
    }
    (jar.remove(Cookie::build(name).path("/")), Redirect::to("/login"))
        .into_response()
}
