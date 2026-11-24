//! The settings page and each of its forms.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) async fn settings_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let u = &user.0;
    let reviews = app.optimizable_reviews(&access).await?;
    let custom = !u.params().is_empty();
    Ok(layout_in(
        Nav::Settings,
        "Settings",
        Some(u),
        html! {
            (flash(msg.as_deref()))
            h1 { "Settings" }
            p.muted { "Logged in as " strong { (u.handle()) } }
            ul.list.links {
                @for (href, name, sub) in [
                    ("/presets", "Presets", "How much to remember, daily limits"),
                    ("/access", "Agent access", "Connect Claude and other agents"),
                    ("/import", "Import", "From Anki or a CSV file"),
                    ("/invites", "Invite someone", "One person a week"),
                ] {
                    li { a.row href=(href) { span.grow { span { (name) } span.sub.muted.small { (sub) } } span.chev aria-hidden="true" { "›" } } }
                }
            }
            (preferences_form(u))
            (studying_form(u))
            (fsrs_section(u, reviews, custom))
            (account_section())
        },
    )
    .into_response())
}

fn preferences_form(u: &User) -> Markup {
    html! {
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
                p.hint.muted.small { "Hard means you remembered, with effort. If you forgot, it's Again; FSRS gets that wrong otherwise." }
                label.check { input type="checkbox" name="optimize_include_agent" value="1" checked[u.optimize_include_agent]; " Use reviews done through agents when optimizing" }
            }
            div.form-foot.end { button.btn.primary type="submit" { "Save" } }
        }
    }
}

fn studying_form(u: &User) -> Markup {
    html! {
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
                label.line { "Suggest a break"
                    span.select { select name="session_minutes" {
                        @for m in study::SESSION_LENGTHS {
                            option value=(m) selected[*m == u.session_minutes] { (if *m == 0 { "Never".to_string() } else { format!("After {}", format::count(*m, "minute")) }) }
                        }
                        // Smart breaks: when the answers show tiredness (breaks.rs).
                        option value=(study::SMART_BREAKS) selected[u.session_minutes == study::SMART_BREAKS] { "When I need one" }
                        @if !study::SESSION_LENGTHS.contains(&u.session_minutes) && u.session_minutes != study::SMART_BREAKS {
                            option value=(u.session_minutes) selected { "After " (format::count(u.session_minutes, "minute")) }
                        }
                    } }
                }
                label.check { input type="checkbox" name="confidence_check" value="1" checked[u.confidence_check]; " Ask whether I'm sure before the answer" }
                p.hint.muted.small { "It doesn't change the schedule. Stats show how often you were right when you felt sure." }
                label.check { input type="checkbox" name="catch_up" value="1" checked[u.catch_up]; " Catch-up mode" }
                label.line { "Reviews a day while catching up" input type="number" name="catch_up_reviews" min="1" max="9999" value=(u.catch_up_reviews); }
                p.hint.muted.small { "For after time away: no new cards, and the cards you most likely still know come first, so less is lost." }
            }
            div.form-foot.end { button.btn.primary type="submit" { "Save" } }
        }
    }
}

fn fsrs_section(u: &User, reviews: i64, custom: bool) -> Markup {
    html! {
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
                "Optimizing fits FSRS to your own memory. It helps once you have a few hundred reviews. New parameters are only kept when they predict your reviews better."
            }
        }
        (params_code(u, custom))
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
    }
}

/// What each run of FSRS-6 weights does, as (first index, count, note).
const PARAM_GROUPS: [(usize, usize, &str); 8] = [
    (0, 4, "initial stability"),
    (4, 2, "initial difficulty"),
    (6, 2, "difficulty change"),
    (8, 3, "stability after recall"),
    (11, 4, "stability after a lapse"),
    (15, 2, "Hard penalty, Easy bonus"),
    (17, 3, "same-day reviews"),
    (20, 1, "forgetting curve decay"),
];

/// The 21 weights as code: one line per group, values aligned, a comment
/// on what they do. Copy gives the plain list.
fn params_code(u: &User, custom: bool) -> Markup {
    let raw = if custom { u.params() } else { fsrs::DEFAULT_PARAMETERS.to_vec() };
    // Older fits have fewer weights; FSRS fills them in the same way.
    let w = fsrs::check_and_fill_parameters(&raw).unwrap_or(raw);
    let plain = format!("[{}]", w.iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>().join(", "));
    let source = if custom { "fitted to your reviews" } else { "defaults" };
    html! {
        details.params {
            summary { "Show the parameters" }
            pre.code {
                span.c { "// FSRS-6 · " (source) } "\n"
                span.k { "w" } " = [\n"
                @for (first, n, note) in PARAM_GROUPS {
                    @let vals = w.iter().skip(first).take(n).map(|x| format!("{x:>7.4},")).collect::<Vec<_>>().join(" ");
                    @let span_ = if n == 1 { format!("{first}") } else { format!("{first}–{}", first + n - 1) };
                    "  " (format!("{vals:<36}")) span.c { "// " (format!("{span_:<6}")) (note) } "\n"
                }
                "]"
            }
            code id="fsrs-weights" hidden { (plain) }
            button.link type="button" data-copy="fsrs-weights" hidden { "Copy" }
        }
    }
}

fn account_section() -> Markup {
    html! {
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
    }
}

#[derive(Deserialize)]
pub(crate) struct SettingsForm {
    pub(crate) timezone: String,
    /// Numbers as typed: a cleared field keeps the current value instead of
    /// failing the whole form.
    #[serde(default)]
    pub(crate) rollover_hour: String,
    #[serde(default)]
    pub(crate) font_scale: String,
    pub(crate) four_buttons: Option<String>,
    pub(crate) optimize_include_agent: Option<String>,
}

pub(crate) async fn settings_update(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<SettingsForm>,
) -> WebResult {
    or_back(
        "/settings",
        async {
            let settings = Settings {
                timezone: f.timezone.trim(),
                rollover_hour: f.rollover_hour.trim().parse().unwrap_or(user.0.rollover_hour),
                four_buttons: f.four_buttons.is_some(),
                font_scale: f.font_scale.trim().parse().unwrap_or(user.0.font_scale),
                optimize_include_agent: f.optimize_include_agent.is_some(),
            };
            app.update_settings(user.0.id, &settings).await?;
            Ok(redirect_msg("/settings", "Settings saved."))
        }
        .await,
    )
}

#[derive(Deserialize)]
pub(crate) struct PasswordForm {
    pub(crate) current: String,
    pub(crate) new: String,
}

pub(crate) async fn password_update(
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
    Ok((jar.add(session_cookie(&app, token)).add(flash_cookie("Password changed.")), Redirect::to("/settings"))
        .into_response())
}

#[derive(Deserialize)]
pub(crate) struct StudySettingsForm {
    #[serde(default)]
    pub(crate) quiet_from: String,
    #[serde(default)]
    pub(crate) quiet_to: String,
    #[serde(default)]
    pub(crate) session_minutes: String,
    pub(crate) confidence_check: Option<String>,
    pub(crate) catch_up: Option<String>,
    #[serde(default)]
    pub(crate) catch_up_reviews: String,
}

pub(crate) async fn study_settings_update(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<StudySettingsForm>,
) -> WebResult {
    let u = &user.0;
    let quiet = match (f.quiet_from.trim().parse::<i64>(), f.quiet_to.trim().parse::<i64>()) {
        (Ok(a), Ok(b)) => Some((a, b)),
        _ => None,
    };
    or_back(
        "/settings#studying",
        async {
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
        .await,
    )
}

#[derive(Deserialize)]
pub(crate) struct CatchUpForm {
    pub(crate) on: Option<String>,
}

pub(crate) async fn catch_up_update(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<CatchUpForm>,
) -> WebResult {
    app.set_catch_up(user.0.id, f.on.is_some()).await?;
    Ok(redirect_msg(
        "/",
        if f.on.is_some() {
            "Catch-up mode is on. No new cards until you have caught up."
        } else {
            "Catch-up mode is off."
        },
    ))
}

#[derive(Deserialize)]
pub(crate) struct AutoOptimizeForm {
    pub(crate) on: Option<String>,
}

pub(crate) async fn auto_optimize(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<AutoOptimizeForm>,
) -> WebResult {
    app.set_auto_optimize(user.0.id, f.on.is_some()).await?;
    Ok(redirect_msg(
        "/settings#fsrs",
        if f.on.is_some() { "FSRS will be optimized every month." } else { "Automatic optimization is off." },
    ))
}

pub(crate) async fn optimize(State(app): State<AppState>, user: WebUser) -> WebResult {
    let result = app.optimize_parameters(&user.access()).await;
    let msg = match result {
        Ok(v) => v.get("note").and_then(serde_json::Value::as_str).unwrap_or("Done.").to_string(),
        Err(AppError::BadRequest(m)) => capitalize(&m),
        Err(e) => return Err(e.into()),
    };
    Ok(redirect_msg("/settings#fsrs", &msg))
}

pub(crate) async fn reset_params(State(app): State<AppState>, user: WebUser) -> WebResult {
    or_back(
        "/settings#fsrs",
        async {
            app.reset_parameters(&user.access()).await?;
            Ok(redirect_msg("/settings#fsrs", "FSRS parameters reset to the defaults."))
        }
        .await,
    )
}
