//! Inviting people, and accepting an invitation.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

/// "tomorrow", or "on Friday, 18 December".
pub(crate) fn on_day(clock: &scheduler::Clock, ts: i64) -> String {
    match format::day(clock, clock.date_of(ts)) {
        d if d == "today" || d == "tomorrow" => d,
        d => format!("on {d}"),
    }
}

/// The open links, each to copy or take back, and when the next one comes.
pub(crate) fn invite_links(inv: &invites::Invites, base: &str, clock: &scheduler::Clock) -> Markup {
    html! {
        @for i in &inv.open {
            div.invite-link {
                pre.copy id=(format!("invite-{}", i.id)) { (base) "/invite/" (i.token) }
                div.invite-meta.muted.small {
                    span.grow { "Works once, until " (format::day(clock, clock.date_of(i.expires_at))) "." }
                    button.link type="button" data-copy=(format!("invite-{}", i.id)) hidden { "Copy" }
                    form.inline method="post" action=(format!("/invites/{}/cancel", i.id)) {
                        button.link type="submit" { "Take it back" }
                    }
                }
            }
        }
        @match inv.next_at {
            None => form method="post" action="/invites" {
                button.btn.primary type="submit" { (if inv.open.is_empty() { "Make an invite link" } else { "Make another link" }) }
            },
            Some(t) if inv.new_account => p.muted { "You can invite someone " (on_day(clock, t).replace("on ", "from ")) ", a week after you joined." },
            Some(t) => p.muted { "Your next invite is ready " (on_day(clock, t)) "." },
        }
    }
}

pub(crate) async fn invites_page(
    State(app): State<AppState>,
    user: WebUser,
    headers: HeaderMap,
    Flash(msg): Flash,
) -> WebResult {
    let u = &user.0;
    let inv = app.invites(u).await?;
    let clock = u.clock();
    let base = oauth::base_url(&app, &headers);
    Ok(layout_in(
        Nav::Settings,
        "Invite",
        Some(u),
        html! {
            (flash(msg.as_deref()))
            (parent("/settings", "Settings"))
            h1 { "Invite someone" }
            p { "AiNKI grows by invitation. Once a week you can send one person a link. It works once, for 30 days." }
            (invite_links(&inv, &base, &clock))
            @if !inv.joined.is_empty() {
                h2 { "Joined through you" }
                ul.list {
                    @for (who, at) in &inv.joined {
                        li.item { span.grow { (who.handle()) } span.muted.small { (capitalize(&format::day(&clock, clock.date_of(*at)))) } }
                    }
                }
            }
        },
    )
    .into_response())
}

pub(crate) async fn invite_create(State(app): State<AppState>, user: WebUser) -> WebResult {
    let to = if user.0.is_admin { "/admin" } else { "/invites" };
    let made = app.create_invite(&user.0).await.map(|_| redirect(to));
    or_back(to, made)
}

pub(crate) async fn invite_cancel(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    let to = if user.0.is_admin { "/admin" } else { "/invites" };
    let done = app.cancel_invite(&user.0, id).await.map(|()| redirect_msg(to, "The link no longer works."));
    or_back(to, done)
}

/// The page behind an invite link: the first thing a new person sees.
/// Quiet on purpose: who invited them, one sentence on what `AiNKI` is, a
/// name and a password, and one button.
pub(crate) fn invite_form(token: &str, from: &User, error: Option<&str>, name: &str) -> Markup {
    layout(
        "You're invited",
        None,
        html! {
            h1.greeting.invited {
                @if from.is_admin { "You're invited." } @else { (capitalize(&from.username)) " invited you." }
            }
            p.lede {
                "AiNKI is a quiet place to learn things by heart. "
                "Each card comes back just before you would forget it, so a few minutes a day are enough."
            }
            @if let Some(e) = error { p.notice role="alert" { (e) } }
            form.invite method="post" action=(format!("/invite/{token}")) {
                label {
                    "Choose a name"
                    input type="text" name="username" value=(name) required minlength="2" maxlength="32"
                        autocomplete="username" autocapitalize="none" autocorrect="off" spellcheck="false";
                }
                label {
                    "Choose a password"
                    input type="password" name="password" required minlength="8" autocomplete="new-password";
                }
                input type="hidden" name="timezone" id="tz" value="";
                button.btn.primary type="submit" { "Begin" }
            }
            p.small.muted.after { "Already have an account? " a href="/login" { "Log in" } }
        },
    )
}

/// A link that was used, taken back or is too old.
pub(crate) fn invite_gone() -> Response {
    (
        StatusCode::NOT_FOUND,
        layout(
            "Invite",
            None,
            html! {
                h1.greeting.invited { "This link no longer works." }
                p.lede { "It has been used, taken back, or is older than 30 days. Ask whoever sent it for a new one." }
                p.small.muted.after { "Already have an account? " a href="/login" { "Log in" } }
            },
        ),
    )
        .into_response()
}

pub(crate) async fn invite_page(
    State(app): State<AppState>,
    user: Option<WebUser>,
    Path(token): Path<String>,
) -> WebResult {
    let Some((_, from)) = app.open_invite(&token).await? else {
        return Ok(invite_gone());
    };
    // Someone with an account opened a link meant for someone new (most
    // likely the one who made it, to see what it looks like): show the page,
    // but signing up would log them out, so say so.
    let note = user.map(|u| format!("You're logged in as {}. This link is for someone new.", u.0.handle()));
    Ok(invite_form(&token, &from, note.as_deref(), "").into_response())
}

pub(crate) async fn invite_accept(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    jar: CookieJar,
    Path(token): Path<String>,
    Form(f): Form<LoginForm>,
) -> Response {
    if app.limiter.check_and_hit(&format!("signup-ip:{ip}"), 10, Duration::from_hours(1)) {
        return security::too_many_requests(3600);
    }
    let from = match app.open_invite(&token).await {
        Ok(Some((_, from))) => from,
        Ok(None) => return invite_gone(),
        Err(e) => return WebError(e).into_response(),
    };
    let result = async {
        let user = app.create_account(&f.username, &f.password, &f.timezone, Some(&token), false).await?;
        let session = app.create_session(user.id).await?;
        Ok::<_, AppError>((user, session))
    }
    .await;
    match result {
        Ok((user, session)) => {
            let welcome = format!("Your name here is {}. Use it, with your password, to log in.", user.handle());
            (jar.add(session_cookie(&app, session)).add(flash_cookie(&welcome)), Redirect::to("/")).into_response()
        }
        Err(e) => form_again(invite_form(&token, &from, Some(&capitalize(&e.to_string())), f.username.trim())),
    }
}
