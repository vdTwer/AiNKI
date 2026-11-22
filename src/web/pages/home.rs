//! The home page, with the decks and the coach's notes.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;
use crate::{
    service::{DeckSummary, Stats},
    share::IncomingShare,
};

/// Everything the home page shows, fetched once.
pub(crate) struct Home {
    pub(crate) decks: Vec<DeckSummary>,
    pub(crate) archived: usize,
    pub(crate) shares: Vec<IncomingShare>,
    pub(crate) stats: Stats,
    pub(crate) total_due: i64,
    /// Cards the coach flagged.
    pub(crate) attention: i64,
    pub(crate) overdue: i64,
    pub(crate) quiet: Option<i64>,
    pub(crate) week: insight::Week,
    pub(crate) notes: Vec<Advice>,
}

impl Home {
    async fn load(app: &App, access: &Access) -> Result<Self, AppError> {
        // Everything that doesn't depend on anything else, at once.
        let (summaries, stats, flagged, overdue, shares, reads) = tokio::try_join!(
            app.deck_summaries(access),
            app.stats(access, None),
            app.attention(access, None),
            app.overdue(access),
            app.incoming_shares(access),
            app.coach_reads(access),
        )?;
        let (archived, decks): (Vec<_>, Vec<_>) =
            summaries.into_iter().partition(|d| d.deck.status == DeckStatus::Archived);
        let attention = num::usize_i64(flagged.len());
        let week = app.week_with(access, attention).await?;
        let known = advice::Known { decks: &decks, stats: &stats, attention: &flagged, overdue };
        let notes = advice::advice_with(access, &known, reads);
        Ok(Self {
            total_due: decks.iter().map(DeckSummary::due_total).sum(),
            archived: archived.len(),
            shares,
            attention,
            overdue,
            quiet: study::quiet_until(&access.user, now()),
            week,
            notes,
            decks,
            stats,
        })
    }

    /// The short line under the headline: time, recall, streak.
    pub(crate) fn facts(&self) -> Vec<String> {
        let s = &self.stats;
        let mut facts = Vec::new();
        if self.total_due > 0 {
            facts.push(format!("About {}", count(s.minutes_for(self.total_due).max(1), "minute")));
        }
        if let Some(r) = s.true_retention_30d {
            facts.push(format!("{} recalled when due, last 30 days", format::percent(Some(r))));
        }
        if s.streak_days > 1 {
            facts.push(format!("{} in a row", count(s.streak_days, "day")));
        }
        facts
    }
}

pub(crate) async fn home(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let h = Home::load(&app, &user.access()).await?;
    let u = &user.0;
    let body = html! {
        (flash(msg.as_deref()))
        // On desktops the week sits beside today, as a sidebar.
        div.hero { (home_today(u, &h)) (home_side(u, &h)) }
        (home_shares(&h.shares))
        (home_decks(u, &h))
        (home_footer(&h))
    };
    Ok(page(Some(Nav::Decks), "Decks", Some(u), Some("home"), body).into_response())
}

fn home_today(user: &User, h: &Home) -> Markup {
    let (decks, total_due, facts, quiet, overdue) = (&h.decks, h.total_due, h.facts(), h.quiet, h.overdue);
    html! {
    section.today aria-label="Today" {
        p.date { @if user.catch_up { "Catching up · " } (format::long_date(&user.clock())) }
        @if total_due > 0 {
            h1 { span.count { (total_due) " " (format::plural(total_due, "card")) } " to study today" }
        } @else if !decks.is_empty() {
            h1.all-done { "All done for today" }
        } @else {
            h1 { "Welcome, " (capitalize(&user.username)) }
        }
        @if !facts.is_empty() { p.facts { (facts.join(" · ")) } }
        @if let Some(t) = quiet {
            p.facts { "Quiet hours until " (format::when(&user.clock(), t)) "." }
        } @else if total_due > 0 {
            p { a.btn.primary.start href="/study" { "Start studying " span.arrow aria-hidden="true" { "→" } } }
        }
        (catch_up_note(user, overdue))
    }
    }
}

fn home_side(user: &User, h: &Home) -> Markup {
    let (decks, week, notes, overdue, attention) = (&h.decks, &h.week, &h.notes, h.overdue, h.attention);
    html! {
    @if !decks.is_empty() {
        aside.today-side {
            (week_note_with(week, notes.first().map(|n| coach_note(n, "/"))))
            @if user.catch_up && overdue >= 20 {
                @let days = (overdue + user.catch_up_reviews - 1) / user.catch_up_reviews.max(1);
                div.pace {
                    p.kicker { "At this pace" }
                    p.pace-days { span.n { (days) } " " (format::plural(days, "day")) " to catch up" }
                    p.small.muted { (format::number(overdue)) " overdue, " (format::number(user.catch_up_reviews)) " a day." }
                }
            }
            @if attention > 0 && notes.first().is_none_or(|n| n.rule != advice::Rule::Leeches) {
                p.small { a href="/attention" { (count(attention, "card")) " " (format::agree(attention, "needs", "need")) " a look" } }
            }
        }
    }
    }
}

fn home_shares(shares: &[IncomingShare]) -> Markup {
    html! {
    @if !shares.is_empty() {
        h2 { "Shared with you" }
        ul.list {
            @for sh in shares {
                li.item {
                    div.grow {
                        strong { (deck_title(&sh.deck_name)) } br;
                        span.small { (sh.cards) " " (format::plural(sh.cards, "card")) " from " (sh.from) }
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
    }
}

fn home_decks(user: &User, h: &Home) -> Markup {
    let (decks, archived) = (&h.decks, h.archived);
    html! {
    @if decks.is_empty() && archived > 0 {
        p.muted { "Every deck is archived. Restore one to study it again." }
    } @else if decks.is_empty() {
        // Someone new gets three quiet ways in.
        div.first-steps {
            p.lede { "Three ways to begin." }
            ul.list.links {
                @for (href, name, sub) in [
                    ("/learn", "Read a short lesson", "How memory works, in a few minutes. Each lesson can become your first deck."),
                    ("/decks/new", "Make a deck", "Write your own cards, one fact on each."),
                    ("/import", "Bring your Anki decks", "From an .apkg or a CSV file."),
                ] {
                    li { a.row href=(href) { span.grow { span { (name) } span.sub.muted.small { (sub) } } span.chev aria-hidden="true" { "›" } } }
                }
            }
        }
    } @else {
        div.table-head aria-hidden="true" {
            span.grow { "Decks" }
            span.num { "Due" }
            span.num { "New" }
            span.chev {}
        }
        ul.list.decks {
            @for d in decks {
                @let due = d.learning_due + d.review_due;
                li {
                    a.row href=(format!("/decks/{}", d.deck.id)) {
                        span.grow {
                            span.deck-name { (deck_title(&d.deck.name)) }
                            @if d.deck.status == DeckStatus::Paused {
                                span.sub { "Paused" }
                            } @else if let Some(exam) = &d.deck.exam_date {
                                span.sub { "Exam " (exam_words(user, *exam)) }
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
    }
}

fn home_footer(h: &Home) -> Markup {
    let (archived, attention) = (h.archived, h.attention);
    html! {
    @if archived > 0 {
        p.small.archived-link { a href="/decks/archived" { (count(num::usize_i64(archived), "archived deck")) } }
    }
    @if attention > 0 {
        p.small.attention-link { a href="/attention" { (count(attention, "card")) " " (format::agree(attention, "needs", "need")) " a look" } }
    }
    p.actions.home-actions {
        a.btn href=("/cards/new") { "Add card" }
        a.btn href=("/decks/new") { "New deck" }
        a.btn href=("/import") { "Import" }
    }
    }
}

/// The last seven days in a sentence or two, and one thing worth doing.
pub(crate) fn week_note(w: &insight::Week) -> Markup {
    let tip = match w.tip {
        insight::Tip::Attention => Some(
            html! { p.tip { span { (count(w.attention, "card")) " keep slipping or ask too much at once. Fixing them saves more time than reviewing them. " a href="/attention" { "Have a look" } } } },
        ),
        insight::Tip::BelowTarget => Some(
            html! { p.tip { span { "You forgot more than you aim for. Fewer new cards for a while, or optimizing FSRS, usually brings it back. " a href="/learn/desired-retention" { "Why" } } } },
        ),
        insight::Tip::AboveTarget => Some(
            html! { p.tip { span { "You remember more than you aim for. Optimizing lets FSRS space your cards further apart. " a href="/settings#fsrs" { "Optimize" } } } },
        ),
        insight::Tip::Spread => Some(
            html! { p.tip { span { "A little every day works better than a lot on a few days. " a href="/learn/spacing" { "Why" } } } },
        ),
        insight::Tip::Nothing => None,
    };
    week_note_with(w, tip)
}

/// One note from the coach, as the week's tip: what it saw, then its link
/// (or, for catch-up mode, a button that turns it on or off).
pub(crate) fn coach_note(a: &Advice, back: &str) -> Markup {
    html! {
        // A div, not a p: a form inside a p would close it early.
        div.tip data-rule=(a.rule.as_str()) {
            span {
                (a.text)
                @if let Some(on) = a.catch_up_toggle {
                    " "
                    form.inline method="post" action="/settings/catch-up" {
                        @if on { input type="hidden" name="on" value="1"; }
                        button.link type="submit" { (a.link.as_ref().map_or(if on { "Catch up" } else { "Turn it off" }, |l| l.label.as_str())) }
                    }
                } @else if let Some(l) = &a.link {
                    " " a href=(l.href) { (l.label) }
                }
                " "
                form.inline.not-now method="post" action="/coach/not-now" {
                    input type="hidden" name="rule" value=(a.rule.as_str());
                    input type="hidden" name="back" value=(back);
                    button.link type="submit" title="Hide this note for three days" { "Not now" }
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct NotNowForm {
    pub(crate) rule: String,
    pub(crate) back: Option<String>,
}

/// "Not now" on a coach note: quiet for three days, back where it was.
pub(crate) async fn coach_not_now(State(app): State<AppState>, user: WebUser, Form(f): Form<NotNowForm>) -> WebResult {
    let rule = advice::Rule::parse(&f.rule).ok_or_else(|| AppError::bad("unknown note"))?;
    app.snooze_advice(&user.access(), rule).await?;
    let back = if f.back.as_deref() == Some("/stats") { "/stats" } else { "/" };
    Ok(redirect_msg(back, "The coach will keep that to itself for three days."))
}

pub(crate) fn week_note_with(w: &insight::Week, tip: Option<Markup>) -> Markup {
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
            @if let Some(t) = tip { (t) }
        }
    }
}

