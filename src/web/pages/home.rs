//! The home page, with the coming week and the coach's notes.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) async fn home(State(app): State<AppState>, user: WebUser, Flash(msg): Flash) -> WebResult {
    let access = user.access();
    let (archived, decks): (Vec<_>, Vec<_>) =
        app.deck_summaries(&access).await?.into_iter().partition(|d| d.deck.status == DeckStatus::Archived);
    let shares = app.incoming_shares(&access).await?;
    let stats = app.stats(&access, None).await?;
    let total_due: i64 = decks.iter().map(crate::service::DeckSummary::due_total).sum();
    let flagged = app.attention(&access, None).await?;
    let attention = num::usize_i64(flagged.len());
    let overdue = app.overdue(&access).await?;
    let quiet = study::quiet_until(&user.0, now());
    let week = app.week(&access).await?;
    let outlook = app.outlook(&access, &decks).await?;
    let known = advice::Known { decks: &decks, stats: &stats, attention: &flagged, overdue };
    let notes = app.advice_from(&access, &known).await?;
    let minutes = stats.minutes_for(total_due).max(1);
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
            (flash(msg.as_deref()))
            // On desktops the week sits beside today, as a sidebar.
            div.hero {
            section.today aria-label="Today" {
                p.date { @if user.0.catch_up { span.calm-only { "Catching up · " } } (format::long_date(&user.0.clock())) }
                @if total_due > 0 {
                    h1 { span.count { (total_due) " " (format::plural(total_due, "card")) } " to study today" }
                } @else if !decks.is_empty() {
                    h1.all-done { span.simple-only { "All done for today" } span.calm-only { "All done." } }
                } @else {
                    h1 { span.simple-only { "Welcome to AiNKI" } span.calm-only { "Welcome, " (capitalize(&user.0.username)) "." } }
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
                    (week_note_with(&week, notes.first().map(|n| coach_note(n, "/"))))
                    @if user.0.catch_up && overdue >= 20 {
                        @let days = (overdue + user.0.catch_up_reviews - 1) / user.0.catch_up_reviews.max(1);
                        div.pace {
                            p.kicker { "At this pace" }
                            p.pace-days { span.n { (days) } " " (format::plural(days, "day")) " to catch up" }
                            p.small.muted { (format::number(overdue)) " overdue, " (format::number(user.0.catch_up_reviews)) " a day." }
                        }
                    }
                    @if attention > 0 && notes.first().is_none_or(|n| n.rule != advice::Rule::Leeches) {
                        p.small { a href="/attention" { (count(attention, "card")) " " (format::agree(attention, "needs", "need")) " a look" } }
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
            @if decks.is_empty() && !archived.is_empty() {
                p.muted { "Every deck is archived. Restore one to study it again." }
            } @else if decks.is_empty() {
                p.simple-only { "No decks yet. Create one, or bring your Anki decks along." }
                // The desktop look: someone new gets three quiet ways in.
                div.calm-only.first-steps {
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
                    @for d in &decks {
                        @let due = d.learning_due + d.review_due;
                        li {
                            a.row href=(format!("/decks/{}", d.deck.id)) {
                                span.grow {
                                    span.deck-name { (deck_title(&d.deck.name)) }
                                    @if d.deck.status == DeckStatus::Paused {
                                        span.sub { "Paused" }
                                    } @else if let Some(exam) = &d.deck.exam_date {
                                        span.sub { "Exam " (exam_words(&user.0, *exam)) }
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
            @if !archived.is_empty() {
                p.small.archived-link { a href="/decks/archived" { (count(num::usize_i64(archived.len()), "archived deck")) } }
            }
            @if attention > 0 {
                p.small.attention-link { a href="/attention" { (count(attention, "card")) " " (format::agree(attention, "needs", "need")) " a look" } }
            }
            p.actions.icon-links.home-actions {
                (icon_btn("/cards/new", ICON_ADD, "Add card"))
                (icon_btn("/decks/new", ICON_DECKS, "New deck"))
                (icon_btn("/import", ICON_IMPORT, "Import"))
            }
            // The desktop look: a plus for a new deck, Import in words.
            p.desk-actions.calm-only hidden[decks.is_empty()] {
                a.add-deck href="/decks/new" aria-label="New deck" data-label="New deck" { (icon(ICON_ADD, PAGE_ICON)) }
                a href="/import" { "Import deck" }
            }
            @if !decks.is_empty() {
                (week_home(&user.0, total_due, &outlook, stats.seconds_per_review))
            }
        },
    )
    .into_response())
}

/// Rounds a forecast the way people say it: whole cards below a hundred,
/// two significant figures above.
pub(crate) const fn about(n: f64) -> i64 {
    let n = num::f64_i64(n.max(0.0).round());
    if n < 100 {
        return n;
    }
    let step = 10i64.pow(n.ilog10() - 1);
    (n + step / 2) / step * step
}

pub(crate) const WHY_OUTLOOK: &str = "Cards come back after you study them: a miss the next day, a young card a few days later. The forecast counts them too, if you study every day and add no more cards.";

/// The desktop home's week looks ahead: what tomorrow asks for, what the
/// next seven days probably ask for (with the forecast and its range on
/// hover), then a bar for each day, today first and in bold. Today holds
/// what the page says to study now (new cards and limits included); later
/// days hold the forecast, which counts the cards that come back after you
/// review them (see `outlook`).
pub(crate) fn week_home(user: &User, today_due: i64, outlook: &Outlook, seconds_per_review: Option<f64>) -> Markup {
    let clock = user.clock();
    let days: Vec<(chrono::NaiveDate, f64)> = (outlook.days.iter().enumerate())
        .map(|(i, &forecast)| {
            let n = if i == 0 { num::i64_f64(today_due.max(0)) } else { forecast };
            (clock.today() + chrono::Duration::days(num::usize_i64(i)), n)
        })
        .collect();
    let tomorrow = outlook.days[1];
    // Tomorrow is settled once today is: only cards answered today can
    // still move it, so any fraction left means it is a forecast.
    let tomorrow_settled = (tomorrow - tomorrow.round()).abs() < 0.05;
    let tomorrow_n = num::f64_i64(tomorrow.round());
    let week = num::i64_f64(today_due.max(0)) + outlook.later();
    let (low, high) = outlook.later_range();
    let (low, high) = (low + today_due.max(0), high + today_due.max(0));
    let total = num::f64_i64(week.round());
    let most = days.iter().map(|&(_, n)| num::f64_i64(n.round())).max().unwrap_or(0).max(1);
    let tip = |i: usize, date: chrono::NaiveDate, n: f64| {
        let r = num::f64_i64(n.round());
        let words = if r == 0 { "nothing due".to_string() } else { count(r, "card") };
        let about = if i >= 2 && r > 0 { "about " } else { "" };
        format!("{}: {about}{words}", format::short_day(&clock, date))
    };
    html! {
        div.week-home.calm-only {
            section.week aria-label="Next seven days" {
                h2 { "Next seven days" }
                @if total == 0 {
                    p { "Nothing comes due in the next week." }
                } @else {
                    p {
                        @if tomorrow_n == 0 {
                            "Nothing due tomorrow."
                        } @else {
                            (if tomorrow_settled { "" } else { "About " })
                            (count(tomorrow_n, "card")) " tomorrow."
                        }
                        " Probably "
                        span.forecast
                            data-tip=(format!("Forecast: {}", count(total, "card")))
                            data-tip-label=(format!("Likely {} to {}", format::number(low), format::number(high)))
                            data-why=(WHY_OUTLOOK) {
                            "about " (format::number(about(week)))
                        }
                        " in the next week"
                        @if let Some(sec) = seconds_per_review { ", " (format::minutes(sec * week / 60.0)) " of study" }
                        "."
                    }
                }
            }
            @if total > 0 {
            div.week-bars role="img" aria-label=(format!("Cards due on each of the next seven days. {}", days.iter().enumerate().map(|(i, &(d, n))| tip(i, d, n)).collect::<Vec<_>>().join(", "))) {
                @for (i, &(date, n)) in days.iter().enumerate() {
                    @let r = num::f64_i64(n.round());
                    div.day.today[i == 0] title=(tip(i, date, n)) {
                        span.bar.empty[r == 0] style=(format!("height: {}%", (r * 100 / most).max(3))) data-n=(format::number(r)) {}
                        span.letter { (date.format("%a").to_string().chars().next().unwrap_or(' ')) }
                    }
                }
            }
            }
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

#[cfg(test)]
mod tests {
    use super::about;

    #[test]
    fn forecasts_round_the_way_people_say_them() {
        assert_eq!(about(0.4), 0);
        assert_eq!(about(42.5), 43);
        assert_eq!(about(99.4), 99);
        assert_eq!(about(312.4), 310);
        assert_eq!(about(315.0), 320);
        assert_eq!(about(1_234.0), 1_200);
    }
}
