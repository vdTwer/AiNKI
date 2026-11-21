//! Decks: creating, editing, merging, pausing, archiving and exporting them.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) fn preset_select(presets: &[crate::service::Preset], selected: Option<i64>) -> Markup {
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
pub(crate) fn goal_words(template: Option<&str>) -> Option<&'static str> {
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
pub(crate) fn goal_picker(presets: &[crate::service::Preset], selected: Option<i64>) -> Markup {
    let order = ["standard", "exam", "language", "relaxed", "maintenance"];
    let mut sorted: Vec<&crate::service::Preset> = presets.iter().collect();
    sorted
        .sort_by_key(|p| p.template.as_deref().and_then(|t| order.iter().position(|o| *o == t)).unwrap_or(order.len()));
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
pub(crate) fn deck_sheet(
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
                input type="date" name="exam_date" value=(deck.and_then(|d| d.exam_date).map_or_else(Default::default, |d| d.to_string()));
            }
            p.hint.muted.small { "With an exam date, no card in this deck is scheduled after the exam." }
        }
    }
}

/// The new-deck page; `draft` and `error` when a sent form comes back.
pub(crate) fn deck_new_view(
    user: &User,
    presets: &[crate::service::Preset],
    draft: Option<&Deck>,
    error: Option<&str>,
) -> Markup {
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
            (add_head("deck", "New deck", html! {}))
            (form_error(error))
            form method="post" action="/decks/new" {
                (deck_sheet(draft, presets, selected))
                div.form-foot.end { button.btn.primary type="submit" { "Create deck" } }
            }
            p.muted.small.merge-link { "Or " a href="/decks/merge" { "merge decks you have" } " into a new one." }
        },
    )
}

pub(crate) async fn deck_new_page(State(app): State<AppState>, user: WebUser) -> WebResult {
    let presets = app.list_presets(&user.access()).await?;
    Ok(deck_new_view(&user.0, &presets, None, None).into_response())
}

#[derive(Deserialize)]
pub(crate) struct DeckCreateForm {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    pub(crate) preset_id: Option<i64>,
    pub(crate) exam_date: Option<String>,
}

impl DeckCreateForm {
    /// What was typed, to show the form again.
    pub(crate) fn draft(&self, id: i64, user_id: i64) -> Deck {
        Deck {
            id,
            user_id,
            name: self.name.clone(),
            description: self.description.clone(),
            preset_id: self.preset_id.unwrap_or(0),
            exam_date: self
                .exam_date
                .as_deref()
                .and_then(|e| chrono::NaiveDate::parse_from_str(e.trim(), "%Y-%m-%d").ok()),
            created_at: 0,
            status: crate::service::DeckStatus::Active,
        }
    }
}

pub(crate) async fn deck_create(
    State(app): State<AppState>,
    user: WebUser,
    Form(f): Form<DeckCreateForm>,
) -> WebResult {
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

/// The merge page: pick decks, name the new one. `draft` and `error` when
/// a sent form comes back.
pub(crate) fn deck_merge_view(
    user: &User,
    decks: &[crate::service::DeckSummary],
    presets: &[crate::service::Preset],
    draft: &DeckMergeForm,
    error: Option<&str>,
) -> Markup {
    let selected = draft
        .preset_id
        .or_else(|| {
            draft.decks.first().and_then(|id| decks.iter().find(|d| d.deck.id == *id)).map(|d| d.deck.preset_id)
        })
        .or_else(|| presets.iter().find(|p| p.template.as_deref() == Some("standard")).map(|p| p.id))
        .or_else(|| presets.first().map(|p| p.id));
    layout_in(
        Nav::Add,
        "Merge decks",
        Some(user),
        html! {
            (add_head("merge", "Merge decks", html! {}))
            (form_error(error))
            @if decks.len() < 2 {
                p.empty.muted { "Merging needs at least two decks. " a href="/decks/new" { "Create a deck." } }
            } @else {
                form method="post" action="/decks/merge" {
                    div.sheet.form {
                        fieldset.merge-decks {
                            legend { "Decks to merge" }
                            @for d in decks {
                                label.check {
                                    input type="checkbox" name="decks" value=(d.deck.id) checked[draft.decks.contains(&d.deck.id)];
                                    span.grow { (deck_plain(&d.deck.name)) " " span.muted.small { (count(d.total, "card")) } }
                                }
                            }
                        }
                        hr.divider;
                        label.title { span.face-label { "Name of the new deck" }
                            input type="text" name="name" required placeholder="e.g. All languages" value=(draft.name);
                        }
                        label.title { span.face-label { "Description " span.muted.optional { "(optional)" } }
                            textarea name="description" rows="2" placeholder="What it's for" { (draft.description) }
                        }
                        hr.divider;
                        (goal_picker(presets, selected))
                        label.check { input type="checkbox" name="keep_progress" value="1" checked[draft.keep_progress.is_some()]; " Keep review progress" }
                        p.hint.muted.small { "On: cards you know stay known. Off: they start as new cards." }
                        label.check { input type="checkbox" name="delete_sources" value="1" checked[draft.delete_sources.is_some()]; " Delete the old decks" }
                        p.hint.muted.small { "On: their cards move into the new deck with their history, and the old decks are gone for good. Off: the new deck gets copies, and the old decks stay as they are." }
                    }
                    div.form-foot.end { button.btn.primary type="submit" { "Merge decks" } }
                }
            }
        },
    )
}

#[derive(Deserialize, Default)]
pub(crate) struct DeckMergeForm {
    #[serde(default)]
    pub(crate) decks: Vec<i64>,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    pub(crate) preset_id: Option<i64>,
    pub(crate) keep_progress: Option<String>,
    pub(crate) delete_sources: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct DeckMergeQuery {
    /// A deck to have picked already (from its deck page).
    pub(crate) with: Option<i64>,
}

pub(crate) async fn deck_merge_page(
    State(app): State<AppState>,
    user: WebUser,
    Query(q): Query<DeckMergeQuery>,
) -> WebResult {
    let access = user.access();
    let decks = app.deck_summaries(&access).await?;
    let presets = app.list_presets(&access).await?;
    let draft = DeckMergeForm { decks: q.with.into_iter().collect(), ..Default::default() };
    Ok(deck_merge_view(&user.0, &decks, &presets, &draft, None).into_response())
}

pub(crate) async fn deck_merge(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckMergeForm>) -> WebResult {
    let access = user.access();
    let input = crate::merge::MergeInput {
        sources: f.decks.clone(),
        name: f.name.clone(),
        description: f.description.clone(),
        preset_id: f.preset_id,
        keep_progress: f.keep_progress.is_some(),
        delete_sources: f.delete_sources.is_some(),
    };
    match app.merge_decks(&access, input).await {
        Ok(m) => {
            let verb = if m.deleted_decks > 0 { "moved" } else { "copied" };
            let mut msg = format!("Decks merged: {} {verb}.", count(m.cards, "card"));
            if m.duplicates > 0 {
                msg.push_str(&format!(" {} left out, already in it once.", count(m.duplicates, "identical card")));
            }
            if m.deleted_decks > 0 {
                msg.push_str(&format!(" {} deleted.", count(m.deleted_decks, "old deck")));
            }
            Ok(redirect_msg(format!("/decks/{}", m.deck.id), &msg))
        }
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            let decks = app.deck_summaries(&access).await?;
            let presets = app.list_presets(&access).await?;
            Ok(form_again(deck_merge_view(&user.0, &decks, &presets, &f, Some(&msg))))
        }
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct DeckQuery {
    pub(crate) q: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) page: Option<i64>,
}

/// Cards listed on one page of a deck.
const DECK_PAGE_SIZE: i64 = 50;

pub(crate) async fn deck_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
    Query(q): Query<DeckQuery>,
) -> WebResult {
    let access = user.access();
    let summary = app.deck_summaries(&access).await?.into_iter().find(|d| d.deck.id == id).ok_or(AppError::NotFound)?;
    let page = q.page.unwrap_or(0).max(0);
    let per_page = DECK_PAGE_SIZE;
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
    let attention = num::usize_i64(app.attention(&access, Some(id)).await?.len());
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
            (flash(msg.as_deref()))
            // On desktops: the deck and its one action on the left, its
            // settings beside it, the rest in a menu.
            (deck_hero(&user.0, &summary, attention))
            (deck_cards(id, &q, &cards, total, page, &base))
        },
    )
    .into_response())
}

fn deck_hero(user: &User, summary: &DeckSummary, attention: i64) -> Markup {
    let (d, id) = (&summary.deck, summary.deck.id);
    html! {
        div.deck-hero {
        div.deck-main {
        div.deck-title {
            h1 { (deck_title(&d.name)) }
        }
        @if !d.description.is_empty() { p { (d.description) } }
        dl.stats-inline {
            div { dt { "New today" } dd { (format::number(summary.new_today)) } }
            div { dt { "Due" } dd { (format::number(summary.learning_due + summary.review_due)) } }
            div { dt { "Cards" } dd { (format::number(summary.total)) } }
        }
        @if let Some(exam) = &d.exam_date { p.muted { "Exam " (exam_words(user, *exam)) } }
        @if !d.status.is_active() {
            // At rest, waking the deck is what this page is for.
            p.rest-note.muted { (rest_words(d.status)) }
            form method="post" action=(format!("/decks/{id}/resume")) {
                button.btn.primary type="submit" { (if d.status == DeckStatus::Archived { "Restore deck" } else { "Resume deck" }) }
            }
        } @else if summary.due_total() > 0 {
            p { a.btn.primary.start href=(format!("/study?deck={id}")) { "Start studying " span.arrow aria-hidden="true" { "→" } } }
        }
        p.actions.deck-menu id="deck-menu" {
            a.btn href=(&format!("/cards/new?deck={id}")) { "Add card" }
            a.btn href=(&format!("/decks/{id}/edit")) { "Edit deck" }
            a.btn href=(&format!("/decks/{id}/edit#share")) { "Share" }
            a.btn href=(&format!("/stats?deck={id}")) { "Stats" }
            a.btn href=(format!("/decks/merge?with={id}")) { "Merge" }
            a.btn href=(format!("/decks/{id}/export.csv")) { "Export CSV" }
        }
        }
        div.deck-side {
        p.muted.small { "Preset: " a href=(format!("/presets/{}", d.preset_id)) { (summary.preset_name) } }
        @if attention > 0 {
            p.small { a href=(format!("/attention?deck={id}")) { (count(attention, "card")) " " (format::agree(attention, "needs", "need")) " a look" } }
        }
        }
        }
    }
}

fn deck_cards(id: i64, q: &DeckQuery, cards: &[Card], total: i64, page: i64, base: &str) -> Markup {
    let per_page = DECK_PAGE_SIZE;
    html! {
        div.cards-head {
            h2 { "Cards" }
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
        ul.list.cards hidden[cards.is_empty()] {
            @for c in cards {
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
        @if page > 0 || (page + 1) * per_page < total {
            nav.actions.quiet-links aria-label="Pages" {
                @if page > 0 { a.btn href=(format!("{base}&page={}", page - 1)) { "‹ Previous" } }
                @if (page + 1) * per_page < total { a.btn href=(format!("{base}&page={}", page + 1)) { "Next ›" } }
            }
        }
        p.deck-export { a href=(&format!("/decks/{id}/export.csv")) { "Export as CSV" } }
    }
}

/// One-line plain preview of Markdown text for lists.
pub(crate) fn truncate(s: &str, n: usize) -> String {
    let flat: String = s.replace('\n', " ").chars().filter(|c| !matches!(c, '*' | '`' | '#' | '_')).collect();
    if flat.chars().count() > n { flat.chars().take(n).collect::<String>() + "…" } else { flat }
}

/// The edit-deck page; `draft` and `error` when a sent form comes back.
pub(crate) async fn deck_edit_view(
    app: &App,
    user: &User,
    id: i64,
    msg: Option<&str>,
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
            (deck_sharing(id, &shares))
            (deck_rest(&deck))
        },
    ))
}

fn deck_sharing(id: i64, shares: &[crate::share::OutgoingShare]) -> Markup {
    html! {
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
                @for sh in shares {
                    li.item {
                        span.grow { (sh.to) }
                        form method="post" action=(format!("/shares/{}/cancel", sh.id)) {
                            button.btn type="submit" { "Cancel" }
                        }
                    }
                }
            }
        }
    }
}

fn deck_rest(deck: &Deck) -> Markup {
    let id = deck.id;
    html! {
        h2 id="rest" { "Pause" }
        p { (rest_words(deck.status)) }
        form.actions.rest-actions method="post" action=(format!("/decks/{id}/pause")) {
            @match deck.status {
                DeckStatus::Active => {
                    button.btn type="submit" { "Pause deck" }
                    " "
                    button.btn type="submit" formaction=(format!("/decks/{id}/archive")) { "Archive deck" }
                }
                DeckStatus::Paused => {
                    button.btn type="submit" formaction=(format!("/decks/{id}/resume")) { "Resume deck" }
                    " "
                    button.btn type="submit" formaction=(format!("/decks/{id}/archive")) { "Archive deck" }
                }
                DeckStatus::Archived => {
                    button.btn type="submit" formaction=(format!("/decks/{id}/resume")) { "Restore deck" }
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
    }
}

pub(crate) async fn deck_edit_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Flash(msg): Flash,
) -> WebResult {
    Ok(deck_edit_view(&app, &user.0, id, msg.as_deref(), None, None).await?.into_response())
}

pub(crate) async fn deck_update(
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
            let page = deck_edit_view(&app, &user.0, id, None, Some(&f.draft(id, user.0.id)), Some(&msg)).await?;
            Ok(form_again(page))
        }
    }
}

pub(crate) async fn deck_delete(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
    or_back(
        format!("/decks/{id}/edit"),
        async {
            let n = app.delete_deck(&user.access(), id).await?;
            Ok(redirect_msg("/", &format!("Deck deleted, with its {}.", count(n, "card"))))
        }
        .await,
    )
}

/// The decks to pick from for a card: archived ones are put away, except
/// `keep`, the one the card is in or was asked for.
pub(crate) fn open_decks(decks: Vec<Deck>, keep: Option<i64>) -> Vec<Deck> {
    decks.into_iter().filter(|d| d.status != DeckStatus::Archived || Some(d.id) == keep).collect()
}

/// What resting means for a deck, said on its pages.
pub(crate) const fn rest_words(status: DeckStatus) -> &'static str {
    match status {
        DeckStatus::Active => {
            "Pausing stops a deck's cards until you resume it; the deck stays on your list. Archiving also puts it away. Progress is kept either way."
        }
        DeckStatus::Paused => "Paused. Its cards wait, with their progress, until you resume the deck.",
        DeckStatus::Archived => {
            "Archived. The deck is put away and its cards wait, with their progress, until you restore it."
        }
    }
}

pub(crate) async fn deck_action(
    State(app): State<AppState>,
    user: WebUser,
    Path((id, action)): Path<(i64, String)>,
) -> WebResult {
    or_back(
        format!("/decks/{id}"),
        async {
            let (status, msg) = match action.as_str() {
                "pause" => (DeckStatus::Paused, "Deck paused."),
                "archive" => (DeckStatus::Archived, "Deck archived. Find it under archived decks below."),
                "resume" => (DeckStatus::Active, "Deck back in study."),
                _ => return Err(AppError::bad("unknown action")),
            };
            app.set_deck_status(&user.access(), id, status).await?;
            // An archived deck leaves the list: home is where it went from.
            let to = if status == DeckStatus::Archived { "/".to_string() } else { format!("/decks/{id}") };
            Ok(redirect_msg(to, msg))
        }
        .await,
    )
}

pub(crate) async fn archived_page(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let decks: Vec<_> =
        app.deck_summaries(&access).await?.into_iter().filter(|d| d.deck.status == DeckStatus::Archived).collect();
    Ok(layout_in(
        Nav::Decks,
        "Archived decks",
        Some(&user.0),
        html! {
            (flash(msg.as_deref()))
            (parent("/", "Decks"))
            h1 { "Archived decks" }
            @if decks.is_empty() {
                p.empty.muted { "Nothing archived. Archive a deck from its settings to put it away." }
            } @else {
                p.muted { "Put away, with their progress. Open one to restore it." }
                ul.list.decks.archived {
                    @for d in &decks {
                        li {
                            a.row href=(format!("/decks/{}", d.deck.id)) {
                                span.grow { span.deck-name { (deck_title(&d.deck.name)) } }
                                span.num.small.muted { (count(d.total, "card")) }
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

pub(crate) async fn deck_export(State(app): State<AppState>, user: WebUser, Path(id): Path<i64>) -> WebResult {
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
