//! The Stats page and its charts.

use crate::service::Stats;
#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

#[derive(Deserialize)]
pub(crate) struct StatsQuery {
    pub(crate) deck: Option<i64>,
}

/// Everything the Stats page shows, for all decks or one.
pub(crate) struct StatsPage {
    pub(crate) deck: Option<Deck>,
    pub(crate) s: Stats,
    pub(crate) cal: crate::tuning::Calibration,
    pub(crate) sure: study::Confidence,
    /// The last seven days, on the stats of all decks only.
    pub(crate) week: Option<insight::Week>,
    pub(crate) retention: Vec<insight::DeckRetention>,
    pub(crate) history: crate::service::History,
    /// What the coach would say, most important first; it speaks about
    /// everything, so only on the stats of all decks.
    pub(crate) notes: Vec<Advice>,
}

impl StatsPage {
    async fn load(app: &App, access: &Access, deck: Option<i64>) -> Result<Self, AppError> {
        let the_deck = async {
            match deck {
                Some(id) => app.deck(access, id).await.map(Some),
                None => Ok(None),
            }
        };
        let (the_deck, s, cal, sure, retention, history) = tokio::try_join!(
            the_deck,
            app.stats(access, deck),
            app.calibration(access, deck),
            app.confidence(access, deck),
            app.deck_retention(access),
            app.history(access, deck),
        )?;
        let retention =
            retention.into_iter().filter(|d| d.reviews > 0 && deck.is_none_or(|id| id == d.deck_id)).collect();
        // The week and the coach speak about everything: all decks only.
        let (week, notes) = if deck.is_none() {
            let (decks, flagged, overdue, reads) = tokio::try_join!(
                app.deck_summaries(access),
                app.attention(access, None),
                app.overdue(access),
                app.coach_reads(access),
            )?;
            let attention = num::usize_i64(flagged.len());
            let week = app.week_with(access, attention).await?;
            let known = advice::Known { decks: &decks, stats: &s, attention: &flagged, overdue };
            (Some(week), advice::advice_with(access, &known, reads))
        } else {
            (None, vec![])
        };
        Ok(Self { deck: the_deck, s, cal, sure, week, retention, history, notes })
    }

    fn study_url(&self) -> String {
        study_url(self.deck.as_ref().map(|d| d.id))
    }

    /// Share of due learned cards remembered this week.
    fn this_week(&self) -> Option<f64> {
        let (total, passed) = self.history.weeks.last().copied().unwrap_or((0, 0));
        (total > 0).then(|| num::i64_f64(passed) / num::i64_f64(total))
    }

    /// The decks' average aim.
    fn aim(&self) -> Option<f64> {
        let aims: Vec<f64> = self.retention.iter().map(|r| r.target).collect();
        (!aims.is_empty()).then(|| aims.iter().sum::<f64>() / num::usize_f64(aims.len()))
    }

    /// The deck furthest below its aim, if any is.
    fn worst(&self) -> Option<&insight::DeckRetention> {
        let gap = |r: &insight::DeckRetention| r.actual.unwrap_or(1.0) - r.target;
        self.retention.iter().filter(|r| r.short()).min_by(|a, b| gap(a).total_cmp(&gap(b)))
    }

    /// Days with a review among the last seven.
    fn last7(&self) -> usize {
        self.history.days.iter().rev().take(7).filter(|(_, n)| *n > 0).count()
    }
}

pub(crate) async fn stats_page(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
    Query(q): Query<StatsQuery>,
) -> WebResult {
    let d = StatsPage::load(&app, &user.access(), q.deck).await?;
    let u = &user.0;
    let body = html! {
        (flash(msg.as_deref()))
        @if let Some(deck) = &d.deck { (parent(&format!("/decks/{}", deck.id), &deck_plain(&deck.name))) }
        h1 { "Stats" }
        div.desk-stats {
            (desk_big3(&d)) (desk_coach(&d)) (desk_charts(u, &d)) (desk_decks(&d)) (desk_model(&d))
        }
        // The exact numbers, folded under the charts.
        details.numbers {
            summary { "All the numbers" }
            (numbers_today(&d)) (numbers_retention(&d)) (numbers_cards(&d)) (numbers_model(&d))
            (numbers_sure(&d)) (numbers_forecast(u, &d))
        }
    };
    Ok(layout_in(Nav::Stats, "Stats", Some(u), body).into_response())
}

fn numbers_today(d: &StatsPage) -> Markup {
    let (s, week) = (&d.s, &d.week);
    html! {
    @if let Some(w) = &week { (week_note(w)) }
    h2 { "Today" }
    dl.stats-inline {
        div { dt { (format::plural(s.cards_today, "Card")) " studied" } dd { (format::number(s.cards_today)) } }
        div { dt { (format::plural(num::f64_i64(s.minutes_today.round()), "Minute")) } dd { (num::f64_i64(s.minutes_today.round())) } }
        div { dt { (format::plural(s.streak_days, "Day")) " in a row" } dd { (s.streak_days) } }
    }
    h2 { "Memory" }
    dl.kv {
        dt { "Reviews, last 30 days" } dd { (format::number(s.reviews_30d)) }
        dt { "Recalled when due, last 30 days" } dd { (format::percent(s.true_retention_30d)) " (" (count(s.retention_sample_30d, "review")) ")" }
        dt { "Average recall chance now" } dd { (format::percent(s.average_retrievability)) }
    }
    }
}

fn numbers_retention(d: &StatsPage) -> Markup {
    let retention = &d.retention;
    html! {
    @if !retention.is_empty() {
        div.fit {
            table.retention {
                thead { tr { th { "Deck" } th.num { "Aim" } th.num { "Remembered" } th.num { "Reviews" } } }
                tbody {
                    @for d in retention {
                        tr.short[d.short()] {
                            th scope="row" { a href=(format!("/decks/{}", d.deck_id)) { (deck_plain(&d.deck_name)) } }
                            td.num { (format::percent(Some(d.target))) }
                            td.num { (format::percent(d.actual)) }
                            td.num { (format::number(d.reviews)) }
                        }
                    }
                }
            }
        }
        p.muted.small { "Reviews of learned cards in the last 30 days. Fewer than " (insight::MIN_SAMPLE) " say little." }
    }
    }
}

fn numbers_cards(d: &StatsPage) -> Markup {
    let s = &d.s;
    html! {
    h2 { "Cards" }
    dl.kv {
        dt { "New" } dd { (format::number(s.new)) }
        dt { "Learning" } dd { (format::number(s.learning)) }
        dt { "Review" } dd { (format::number(s.review)) }
        dt { "Suspended" } dd { (format::number(s.suspended)) }
        dt { "Total" } dd { (format::number(s.total)) }
    }
    }
}

fn numbers_model(d: &StatsPage) -> Markup {
    let cal = &d.cal;
    html! {
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
        div.fit {
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
        }
    }
    }
}

fn numbers_sure(d: &StatsPage) -> Markup {
    let sure = &d.sure;
    html! {
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
    }
}

fn numbers_forecast(user: &User, d: &StatsPage) -> Markup {
    let s = &d.s;
    let max = s.forecast.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    html! {
    h2 { "Due in the next two weeks" }
    table.forecast {
        tbody {
            @for (day, n) in &s.forecast {
                tr {
                    th scope="row" { (forecast_day(user, day)) }
                    td.bar-cell {
                        @if *n > 0 { span.bar style=(format!("width:{}%", n * 100 / max)) {} }
                    }
                    td.num { (format::number(*n)) }
                }
            }
        }
    }
    }
}

// What the three big numbers count, on hover.
pub(crate) const WHY_DUE: &str = "Learned cards whose review falls today or earlier. New cards come on top. The time below uses your average time per review.";
pub(crate) const WHY_WEEK: &str = "Of the learned cards that came due in the last 7 days, the share you remembered.";
pub(crate) const WHY_STREAK: &str =
    "Days in a row with at least one review. Today counts once you study, and doesn't break the streak before then.";

/// Stats open with three numbers to act on; the charts follow.
fn desk_big3(d: &StatsPage) -> Markup {
    let (s, study, this_week, aim, worst, last7) = (&d.s, d.study_url(), d.this_week(), d.aim(), d.worst(), d.last7());
    let minutes = s.seconds_per_review.map(|sec| sec * num::i64_f64(s.due_now) / 60.0);
    html! {
    section.big3 {
        div {
            p.kicker title=(WHY_DUE) { "Due today" }
            p.n.due title=(WHY_DUE) { (format::number(s.due_now)) }
            p.small.muted {
                @if let Some(m) = minutes.filter(|_| s.due_now > 0) { (capitalize(&format::minutes(m))) "." } @else if s.due_now == 0 { "Nothing waiting." }
            }
            @if s.due_now > 0 { a.small href=(study) { "Start studying" } }
        }
        div {
            p.kicker title=(WHY_WEEK) { "This week" }
            p.n title=(WHY_WEEK) { (format::percent(this_week)) }
            p.small.muted {
                @if this_week.is_none() { "No reviews of learned cards this week. " }
                @if let Some(a) = aim { "You aim for " (format::percent(Some(a))) "." }
                @if let Some(w) = worst { " " (deck_plain(&w.deck_name)) " pulls it down." }
            }
            @if let Some(w) = worst { a.small href=(format!("/decks/{}", w.deck_id)) { "Look at that deck" } }
        }
        div {
            p.kicker title=(WHY_STREAK) { "Days in a row" }
            p.n.it title=(WHY_STREAK) { (s.streak_days) }
            p.small.muted { "You studied on " (last7) " of the last 7 days." }
        }
    }
    }
}

fn desk_coach(d: &StatsPage) -> Markup {
    html! {
    @if let Some((first, more)) = d.notes.split_first() {
        section.coach-notes aria-label="The coach" {
            h2 { "The coach" }
            (coach_note(first, "/stats"))
            @if !more.is_empty() {
                details.more-notes {
                    summary { (count(num::usize_i64(more.len()), "more note")) }
                    @for n in more { (coach_note(n, "/stats")) }
                }
            }
        }
    }
    }
}

fn desk_charts(user: &User, d: &StatsPage) -> Markup {
    let (s, last7) = (&d.s, d.last7());
    let forecast: Vec<(String, i64)> = s.forecast.iter().map(|(day, n)| (forecast_day(user, day), *n)).collect();
    let aim = d.aim();
    html! {
    section.chart-sec {
        div.chart-main {
            h2 title="Each point is one week: of the learned cards that came due, the share you remembered. The dashed line is your aim. Hover a week for its numbers." { "Are you remembering what you aim for?" }
            p.small.muted { "Share of due cards you remembered, week by week." }
            (charts::retention(&d.history.weeks, aim))
        }
        aside.chart-side {
            p.kicker title="Each square is a day and each column a week, oldest on the left; today is the last square. The stronger the color, the more you reviewed." { "Study days" }
            (charts::days(&d.history.days))
            p.small.muted { "Stronger color means more reviews. " (last7) " of the last 7 days." }
        }
    }
    section.chart-sec {
        div.chart-main {
            h2 title="How many learned cards come due on each day, as they are scheduled now. Every review you do schedules its card again, so later days fill up as you study." { "Next two weeks" }
            p.small.muted { "Cards due each day." }
            (charts::bars(&forecast, "Cards due on each of the next fourteen days"))
        }
        aside.chart-side {
            p.kicker title="New cards were never studied. Learning cards are in their first short steps. Review cards are learned, and FSRS spaces them out. Suspended cards are paused and never shown." { "Cards" }
            p.serif-note { (count(s.total, "card")) ". " (format::number(s.review)) " in review, " (format::number(s.new)) " still new." }
            p.small.muted { (format::number(s.learning)) " learning, " (format::number(s.suspended)) " suspended." }
        }
    }
    }
}

fn desk_decks(d: &StatsPage) -> Markup {
    let worst = d.worst();
    let rows: Vec<_> = d.retention.iter().filter(|r| r.actual.is_some()).collect();
    html! {
    @if !rows.is_empty() {
        section.chart-sec {
            h2 title="What each deck aims for, and what you remembered of its learned cards in the last 30 days." { "By deck" }
            table.by-deck {
                thead { tr { th { "Deck" } th.num { "Aim" } th.num { "Remembered" } } }
                tbody {
                    @for r in &rows {
                        tr.short[r.short()] {
                            td { a href=(format!("/decks/{}", r.deck_id)) { (deck_plain(&r.deck_name)) } }
                            td.num { (format::percent(Some(r.target))) }
                            td.num title=(format!("{} reviews", r.reviews)) { (format::percent(r.actual)) }
                        }
                    }
                }
            }
            @if let Some(w) = worst {
                p.small { (deck_plain(&w.deck_name)) " is " (num::f64_i64(((w.target - w.actual.unwrap_or(w.target)) * 100.0).round())) " points below its aim. " a href="/attention" { "Open Needs a look" } }
            }
        }
    }
    }
}

fn desk_model(d: &StatsPage) -> Markup {
    let bins: Vec<(f64, f64, i64)> = d.cal.bins.iter().map(|b| (b.predicted, b.actual, b.reviews)).collect();
    html! {
    @if d.cal.reviews >= 50 {
        section.chart-sec {
            div.chart-main.calib {
                (charts::calibration(&bins))
                div {
                    h2 title="Before every review, FSRS predicts how likely you are to remember. Each dot groups a year of reviews by that prediction and shows how often you did remember. On the dashed line, FSRS knows you well." { "How well FSRS knows you" }
                    p.serif-note {
                        "FSRS expected you to remember " (format::percent(Some(d.cal.predicted))) " of your reviews last year. You remembered " (format::percent(Some(d.cal.actual))) "."
                        @if (d.cal.predicted - d.cal.actual).abs() <= 0.03 { " That's a close fit." }
                    }
                }
            }
            @if let (Some(a), Some(b)) = (d.sure.sure_pct(), d.sure.unsure_pct()) {
                aside.chart-side {
                    p.kicker title="Before seeing the answer, you said whether you knew it. These bars show how often you then remembered, over the last year." { "When you felt sure" }
                    div.sure-bars {
                        div title=(format!("You remembered {} of the {} answers where you felt sure.", d.sure.sure_recalled, d.sure.sure)) {
                            span { "Sure" } span.track { span.fill style=(format!("width:{a:.0}%")) {} } span.num { (format!("{a:.0}%")) }
                        }
                        div title=(format!("You remembered {} of the {} answers where you weren't sure.", d.sure.unsure_recalled, d.sure.unsure)) {
                            span { "Not sure" } span.track { span.fill.grey style=(format!("width:{b:.0}%")) {} } span.num { (format!("{b:.0}%")) }
                        }
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
