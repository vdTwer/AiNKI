//! Presets: the study settings that decks share.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) async fn presets_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(layout_in(
        Nav::Settings,
        "Presets",
        Some(&user.0),
        html! {
            (flash(msg.as_deref()))
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
pub(crate) fn preset_sheet(p: &crate::service::Preset, used_by: &[String]) -> Markup {
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
pub(crate) const TIME_LIMITS: &[i64] = &[0, 5, 8, 10, 15, 20, 30, 45, 60];

pub(crate) fn time_limit_words(secs: i64) -> String {
    if secs == 0 { "No limit".into() } else { format!("{secs} seconds") }
}

#[derive(Deserialize)]
pub(crate) struct TemplateQuery {
    pub(crate) template: Option<String>,
}

/// A preset as a template would make it, not saved yet.
pub(crate) fn preset_draft(t: &presets::PresetTemplate, user_id: i64) -> crate::service::Preset {
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
        fsrs_params: sqlx::types::Json(Vec::new()),
        fsrs_params_updated_at: None,
        time_limit: 0,
        typed_answers: false,
    }
}

/// A new preset, filled in from a template. Nothing is saved until the
/// form is sent, like a new deck.
pub(crate) fn preset_new_view(
    user: &User,
    template: &str,
    draft: &crate::service::Preset,
    error: Option<&str>,
) -> Markup {
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

pub(crate) async fn preset_new_page(user: WebUser, Query(q): Query<TemplateQuery>) -> WebResult {
    let t = presets::template(q.template.as_deref().unwrap_or("standard")).ok_or(AppError::NotFound)?;
    Ok(preset_new_view(&user.0, t.key, &preset_draft(t, user.0.id), None).into_response())
}

pub(crate) async fn preset_create(State(app): State<AppState>, user: WebUser, Form(f): Form<PresetForm>) -> WebResult {
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
pub(crate) async fn preset_view(
    app: &App,
    user: &User,
    id: i64,
    msg: Option<&str>,
    draft: Option<&crate::service::Preset>,
    error: Option<&str>,
    days: usize,
) -> Result<Markup, AppError> {
    let access = Access::web(user.clone());
    let p = app.preset(&access, id).await?;
    let workload = app.workload(&access, id, days).await?;
    let decks: Vec<String> =
        app.decks(&access).await?.into_iter().filter(|d| d.preset_id == id).map(|d| d.name).collect();
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
            (preset_cost(user, &p, &workload, days))
            (preset_decks(id, &decks))
        },
    ))
}

fn preset_cost(user: &User, p: &crate::service::Preset, workload: &[crate::tuning::Workload], days: usize) -> Markup {
    let id = p.id;
    html! {
        h2 id="cost" { "What it costs" }
        p.muted.small {
            "A simulation of the next "
            @for (n, (d, label)) in [(30, "30 days"), (90, "90 days"), (365, "year")].into_iter().enumerate() {
                @if n > 0 { " · " }
                @if d == days { (label) } @else { a href=(format!("/presets/{id}?days={d}#cost")) { (label) } }
            }
            ", with your memory model and "
            @if p.new_per_day == 0 { "no new cards" } @else { (count(p.new_per_day, "new card")) " a day" }
            ". Higher retention means more reviews but more remembered."
        }
        div.fit {
            table.workload {
                thead { tr { th { "Remember" } th.num { "Minutes a day" } th.num { "Reviews a day" } th.num { "Known at the end" } } }
                tbody {
                    @for w in workload {
                        tr.current[w.current] {
                            th scope="row" { (format!("{:.0}%", w.retention * 100.0)) @if w.current { span.muted.small { " this preset" } } }
                            td.num { (format!("{:.0}", w.minutes_per_day.max(0.0))) }
                            td.num { (format::number(num::f64_i64(w.reviews_per_day.round()))) }
                            td.num { (format::number(num::f64_i64(w.remembered.round()))) }
                        }
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
    }
}

fn preset_decks(id: i64, decks: &[String]) -> Markup {
    html! {
        @if decks.is_empty() {
            details.danger {
                summary { "Delete this preset…" }
                form method="post" action=(format!("/presets/{id}/delete")) {
                    p { "No deck uses it, so nothing else changes." }
                    button.btn.danger type="submit" { "Delete preset" }
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct PresetQuery {
    pub(crate) days: Option<usize>,
}

pub(crate) async fn preset_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
    Query(q): Query<PresetQuery>,
) -> WebResult {
    let days = q.days.filter(|d| [30, 90, 365].contains(d)).unwrap_or(30);
    Ok(preset_view(&app, &user.0, id, msg.as_deref(), None, None, days).await?.into_response())
}

#[derive(Deserialize)]
pub(crate) struct PresetForm {
    #[serde(default)]
    pub(crate) template: Option<String>,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) desired_retention: f64,
    pub(crate) new_per_day: i64,
    pub(crate) reviews_per_day: i64,
    pub(crate) learning_steps: String,
    pub(crate) relearning_steps: String,
    pub(crate) max_interval: i64,
    #[serde(default)]
    pub(crate) time_limit: i64,
    pub(crate) typed_answers: Option<String>,
}

impl PresetForm {
    pub(crate) fn into_input(self) -> PresetInput {
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
    pub(crate) fn draft(&self, id: i64, user_id: i64) -> crate::service::Preset {
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
            fsrs_params: sqlx::types::Json(Vec::new()),
            fsrs_params_updated_at: None,
            time_limit: self.time_limit,
            typed_answers: self.typed_answers.is_some(),
        }
    }
}

pub(crate) async fn preset_update(
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
            Ok(form_again(preset_view(&app, &user.0, id, None, Some(&draft), Some(&msg), 30).await?))
        }
    }
}

pub(crate) async fn preset_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(
        format!("/presets/{id}"),
        async {
            app.delete_preset(&user.access(), id).await?;
            Ok(redirect_msg("/presets", "Preset deleted."))
        }
        .await,
    )
}
