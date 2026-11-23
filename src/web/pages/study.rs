//! Studying: the card, the answer buttons, breaks, quiet hours and undo.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

#[derive(Deserialize)]
pub(crate) struct StudyQuery {
    pub(crate) deck: Option<i64>,
    pub(crate) card: Option<i64>,
    pub(crate) show: Option<u8>,
    pub(crate) t: Option<i64>,
    /// From the reveal form: felt sure (1) or not (0), and what was typed.
    pub(crate) sure: Option<u8>,
    pub(crate) typed: Option<String>,
}

/// How a card is on the page: since when, whether turned over, and what
/// came with turning it (the sure check, a typed answer).
pub(crate) struct Shown {
    pub(crate) at: i64,
    pub(crate) revealed: bool,
    pub(crate) sure: Option<bool>,
    pub(crate) typed: Option<String>,
}

impl Shown {
    pub(crate) const fn front(at: i64) -> Self {
        Self { at, revealed: false, sure: None, typed: None }
    }
}

/// What a card's study page adds around the card itself.
pub(crate) struct StudyExtras {
    /// The plain "Show answer" link, inside a multicard's form; empty when
    /// the reveal is a form of its own (typed answers, the sure check).
    pub(crate) reveal_inside: Markup,
    pub(crate) sure_field: Markup,
    pub(crate) echo: Markup,
    pub(crate) timed_out: bool,
}

pub(crate) fn study_url(deck: Option<i64>) -> String {
    match deck {
        Some(d) => format!("/study?deck={d}"),
        None => "/study".into(),
    }
}

/// The thin line and "14 of 32" under the card. `done` is in half cards
/// (see `App::session_progress`).
pub(crate) fn study_foot(
    deck_name: &str,
    deck_id: i64,
    done: i64,
    total: i64,
    q_deck: Option<i64>,
    card_id: Option<i64>,
    leech: bool,
) -> Markup {
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
                    button.link type="submit" { span { "Undo last answer" } }
                }
                @if let Some(id) = card_id { a href=(format!("/cards/{id}{}", CardFrom::study(q_deck).query())) { "Edit card" } }
                // Desktop only: a break of your own. The card is covered and
                // its clock stops (app.js).
                @if card_id.is_some() {
                    button.link.pause-study type="button" { span { "Pause" } }
                }
            }
            @if card_id.is_some() {
                div.pause-cover hidden data-stay {
                    p.kicker { "Paused" }
                    button.btn.primary type="button" data-resume { "Resume" }
                }
            }
            @if let (true, Some(id)) = (leech, card_id) {
                p.leech-note.small { "This card keeps slipping. " a href=(format!("/cards/{id}{}", CardFrom::study(q_deck).query())) { "Have a look after this answer" } }
            }
        }
    }
}

/// What people see for a card's state. Relearning counts as learning,
/// as in the filter and the stats.
pub(crate) const fn state_label(state: CardState) -> &'static str {
    match state {
        CardState::New => "New",
        CardState::Learning | CardState::Relearning => "Learning",
        CardState::Review => "Review",
    }
}

/// A card's state, or "Suspended", for lists.
pub(crate) const fn card_status(card: &Card) -> &'static str {
    if card.suspended { "Suspended" } else { state_label(card.card_state()) }
}

/// Stored rating names as button words.
pub(crate) fn rating_label(stored: &str) -> &str {
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
pub(crate) fn source_label(stored: &str) -> &str {
    match stored {
        "agent" => "By an agent",
        "import" => "Imported",
        "shared" => "From a shared deck",
        "web" | "manual" => "On the website",
        other => other,
    }
}

pub(crate) fn state_tag(state: CardState) -> Markup {
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
pub(crate) async fn study_card(
    app: &App,
    access: &Access,
    user: &User,
    msg: Option<&str>,
    card: &Card,
    deck: &Deck,
    q_deck: Option<i64>,
    shown: &Shown,
) -> Result<Markup, AppError> {
    let revealed = shown.revealed;
    let previews = app.previews(access, card, deck).await?;
    let preset = app.preset(access, deck.preset_id).await?;
    // Time limits, typing and the sure check are for cards seen before;
    // a new card can't be known yet.
    let seen = card.card_state() != CardState::New;
    let limit = if seen { preset.time_limit } else { 0 };
    // A brought card type asks in its own way; typing an answer is not it.
    let brought = app.typed_card(card).await?;
    let typed = seen && preset.typed_answers && brought.is_none();
    let ask_sure = seen && user.confidence_check;
    // Without JavaScript the time counts until the answer is shown.
    let timed_out = revealed && limit > 0 && now() - shown.at > limit + 1;
    let (done, total) = app.session_progress(access, q_deck).await?;
    let leech = app.leech_note(card).await?.is_some();
    let deck_param = q_deck.map_or_else(Default::default, |d| format!("&deck={d}"));
    let show_href = format!("/study?card={}&show=1&t={}{}#answer", card.id, shown.at, deck_param);
    let reveal_form = typed || ask_sure;
    let reveal = if reveal_form {
        reveal_controls(card.id, shown.at, q_deck, revealed, typed, ask_sure)
    } else {
        reveal_button(&show_href, revealed)
    };
    let sure_value = match shown.sure {
        Some(true) => "1",
        Some(false) => "0",
        None => "",
    };
    let sure_field = html! { input.sure-field type="hidden" name="sure" value=(sure_value); };
    let echo = typed_echo(typed, shown.typed.as_deref());
    let timer = timer_box(limit, revealed);
    let timeout = timeout_form(card, shown, q_deck, timed_out, &sure_field);
    let foot = study_foot(&deck.name, deck.id, done, total, q_deck, Some(card.id), leech);
    // A multicard asks its items in its own body, with the reveal and the
    // answers inside; a plain card shows its faces and the buttons below.
    let body = if let Some(how) = card.multi {
        let extras = StudyExtras {
            reveal_inside: if reveal_form {
                html! {}
            } else {
                reveal.clone()
            },
            sure_field,
            echo,
            timed_out,
        };
        let multi = multi_study(card, how, &app.card_items(card).await?, q_deck, shown, &extras);
        html! { (multi) (timer) @if reveal_form { (reveal) } (timeout) (foot) }
    } else if let Some(b) = &brought {
        // What the learner did in the frame goes with the rating.
        let fields = html! { (sure_field) input.response-field type="hidden" name="response"; };
        let answers =
            answer_form(card, shown, q_deck, user.four_buttons, revealed && !timed_out, &previews, &fields);
        html! { (typed_faces(card, b, revealed)) (timer) (reveal) (timeout) (answers) (foot) }
    } else {
        let answers =
            answer_form(card, shown, q_deck, user.four_buttons, revealed && !timed_out, &previews, &sure_field);
        html! { (card_faces(card, revealed, &echo)) (timer) (reveal) (timeout) (answers) (foot) }
    };
    let body = html! { (flash(msg)) section.study.revealed[revealed].timed-out[timed_out] { (body) } };
    Ok(page(Some(Nav::Study), "Study", Some(user), Some("study"), body))
}

/// A plain card's front, and its back once turned over.
fn card_faces(card: &Card, revealed: bool, echo: &Markup) -> Markup {
    html! {
        div.card-area {
            (state_tag(card.card_state()))
            section.face.front { (face(&card.front, card.front_image.as_deref())) }
            hr.divider hidden[!revealed];
            section.face.back id="answer" hidden[!revealed] {
                (echo)
                (face(&card.back, card.back_image.as_deref()))
            }
        }
    }
}

/// Shown when a timed card runs out: the answer counts as forgotten.
fn timeout_form(card: &Card, shown: &Shown, q_deck: Option<i64>, timed_out: bool, sure_field: &Markup) -> Markup {
    html! {
    form.timeout-form method="post" action="/study/answer" hidden[!timed_out] data-stay {
        input type="hidden" name="card" value=(card.id);
        input type="hidden" name="reps" value=(card.reps);
        input type="hidden" name="t" value=(shown.at);
        input type="hidden" name="timeout" value="1";
        @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
        (sure_field)
        p.timeout-note { "Time's up. This one counts as forgotten." }
        div.answers.cols-1 {
            button.btn.again type="submit" data-key=" " { span.label { "Continue" " " span.key { "space" } } }
        }
    }
    }
}

/// The rating buttons, each with the interval it would give.
fn answer_form(
    card: &Card,
    shown: &Shown,
    q_deck: Option<i64>,
    four: bool,
    visible: bool,
    previews: &[(Rating, String)],
    sure_field: &Markup,
) -> Markup {
    let buttons: &[(Rating, &str, &str)] = if four {
        &[
            (Rating::Again, "Again", "1"),
            (Rating::Hard, "Hard", "2"),
            (Rating::Good, "Good", "3"),
            (Rating::Easy, "Easy", "4"),
        ]
    } else {
        &[(Rating::Again, "Again", "1"), (Rating::Good, "Good", "3")]
    };
    html! {
        form.answers.cols-2[!four].cols-4[four] method="post" action="/study/answer" hidden[!visible] {
            input type="hidden" name="card" value=(card.id);
            input type="hidden" name="reps" value=(card.reps);
            input type="hidden" name="t" value=(shown.at);
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            (sure_field)
            @for (rating, label, key) in buttons {
                @let preview = previews.iter().find(|(r, _)| r == rating).map_or("", |(_, p)| p.as_str());
                button.btn.primary[*rating == Rating::Good].again[*rating == Rating::Again] type="submit" name="rating" value=(*rating as i64) data-key=(key) {
                    span.label { (label) " " span.key { (key) } }
                    span.ivl { (preview) }
                }
            }
        }
    }
}

/// The countdown for a timed card. Without JavaScript it only names the
/// limit, and the time counts until the answer is shown.
pub(crate) fn timer_box(limit: i64, revealed: bool) -> Markup {
    if limit <= 0 {
        return html! {};
    }
    html! {
        div.timer data-limit=(limit) hidden[revealed] data-stay {
            div.timer-line aria-hidden="true" { div.timer-fill {} }
            div.timer-meta {
                span.timer-secs role="timer" { (limit) " s" }
                button.link.extend type="button" data-key="+" hidden { "+10 s" }
            }
        }
    }
}

/// "Show answer" as a form: with a box for the typed answer, or as the
/// sure check's two buttons. Without JavaScript it loads the answer side.
pub(crate) fn reveal_controls(
    card_id: i64,
    at: i64,
    q_deck: Option<i64>,
    revealed: bool,
    typed: bool,
    ask_sure: bool,
) -> Markup {
    html! {
        form.reveal.reveal-form method="get" action="/study#answer" hidden[revealed] {
            input type="hidden" name="card" value=(card_id);
            input type="hidden" name="show" value="1";
            input type="hidden" name="t" value=(at);
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            @if typed {
                label.typed-answer {
                    span.face-label { "Your answer" }
                    input type="text" name="typed" autocomplete="off" autocapitalize="off" spellcheck="false" maxlength="500" placeholder="Type it, then Enter";
                }
            }
            @if ask_sure {
                p.sure-ask { "Do you know it?" }
                div.sure-buttons {
                    button.btn.primary type="submit" name="sure" value="1" data-key="s" { span.label { "Sure" " " span.key { "S" } } }
                    button.btn type="submit" name="sure" value="0" data-key="u" { span.label { "Not sure" " " span.key { "U" } } }
                }
            } @else {
                button.btn.primary.show type="submit" data-key=" " { "Show answer" span.key { "space" } }
            }
        }
    }
}

/// What was typed, shown above the answer to compare. Filled in by app.js
/// when it reveals the card.
pub(crate) fn typed_echo(typed: bool, text: Option<&str>) -> Markup {
    if !typed {
        return html! {};
    }
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    html! {
        div.typed-echo hidden[text.is_none()] data-stay {
            span.face-label { "You wrote" }
            p.typed-text { (text.unwrap_or("")) }
        }
    }
}

pub(crate) fn reveal_button(href: &str, revealed: bool) -> Markup {
    html! {
        p.reveal hidden[revealed] {
            a.btn.primary.show data-key=" " href=(href) { "Show answer" span.key { "space" } }
        }
    }
}

/// One item's Markdown, without the paragraph around it when it is a
/// single line, so it sits inside a row.
pub(crate) fn item_md(text: &str) -> Markup {
    let html = markdown::render(text);
    let t = html.trim();
    let inner = t.strip_prefix("<p>").and_then(|x| x.strip_suffix("</p>")).filter(|x| !x.contains("<p>"));
    PreEscaped(inner.unwrap_or(t).to_string())
}

/// A multicard's back as it is studied: everything on it in order, every
/// item a row. Asked items are rows to tap when you remembered them (when
/// `tap`; the editor's preview only shows them), the others show as known.
pub(crate) fn multi_back(pieces: &[Piece], asked: &[bool], tap: bool) -> Markup {
    // Items next to each other share one list: a block is text, or items
    // with their numbers.
    type Block<'a> = (Option<&'a str>, Vec<(usize, &'a str)>);
    let mut blocks: Vec<Block> = Vec::new();
    let mut n = 0;
    for p in pieces {
        match p {
            Piece::Text(t) => blocks.push((Some(t), vec![])),
            Piece::Item(t) => {
                match blocks.last_mut() {
                    Some((None, items)) => items.push((n, t)),
                    _ => blocks.push((None, vec![(n, t)])),
                }
                n += 1;
            }
        }
    }
    html! {
        @for (text, items) in &blocks {
            @if let Some(t) = text {
                div.md { (PreEscaped(markdown::render(t))) }
            } @else {
                ol.list-items {
                    @for (n, text) in items {
                        @let n = *n;
                        @if asked.get(n).copied().unwrap_or(true) && tap {
                            li {
                                input type="hidden" name="asked" value=(n);
                                label.row.item data-key=[(n < 9).then(|| (n + 1).to_string())] {
                                    input type="checkbox" name="recalled" value=(n);
                                    span.num { (n + 1) }
                                    span.text { (item_md(text)) }
                                    span.mark { "Remembered" }
                                }
                            }
                        } @else {
                            li.known[tap].shown[!tap] {
                                span.num { (n + 1) }
                                span.text { (item_md(text)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A back with marked segments, rendered as it is written with every
/// segment drawn by `segment` (its number and words). The marks become
/// placeholders before the Markdown is rendered, so segments sit where they
/// were written, inside sentences and list items alike.
fn marked_with(back: &str, segment: impl Fn(usize, &str) -> Markup) -> Markup {
    let src = markdown::tidy(back);
    let marks = multi::marks(&src);
    let hole = |n: usize| format!("\u{E000}{n}\u{E001}");
    let mut text = String::with_capacity(src.len());
    let mut at = 0;
    for (n, (range, _)) in marks.iter().enumerate() {
        text.push_str(src.get(at..range.start).unwrap_or(""));
        text.push_str(&hole(n));
        at = range.end;
    }
    text.push_str(src.get(at..).unwrap_or(""));
    let mut html = markdown::render(&text);
    for (n, (_, words)) in marks.iter().enumerate() {
        html = html.replace(&hole(n), &segment(n, words).into_string());
    }
    html! { div.md.marked { (PreEscaped(html)) } }
}

/// A marked back as it is studied: every asked segment a word to tap when
/// you remembered it (when `tap`; the editor's preview only shows them),
/// the others plain, as known.
pub(crate) fn marked_back(back: &str, asked: &[bool], tap: bool) -> Markup {
    marked_with(back, |n, words| {
        if !tap {
            html! { span.seg.shown { (item_md(words)) } }
        } else if asked.get(n).copied().unwrap_or(true) {
            html! {
                label.seg data-key=[(n < 9).then(|| (n + 1).to_string())] {
                    input type="hidden" name="asked" value=(n);
                    input type="checkbox" name="recalled" value=(n);
                    (item_md(words))
                }
            }
        } else {
            html! { span.seg.known { (item_md(words)) } }
        }
    })
}

/// While relearning, the front shows the back's skeleton: what you already
/// know in place, and a gap for each segment you are asked again. Every
/// gap is as wide as the others, so none hints at its answer's length.
pub(crate) fn marked_skeleton(back: &str, asked: &[bool]) -> Markup {
    marked_with(back, |n, words| {
        if asked.get(n).copied().unwrap_or(true) {
            html! { span.seg.gap aria-label="a part to recall" {} }
        } else {
            html! { span.seg.known { (item_md(words)) } }
        }
    })
}

/// The back still marks exactly the stored items.
fn marked_fits(card: &Card, how: Split, items: &[multi::CardItem]) -> bool {
    how == Split::Marked && multi::items(&card.back, how).is_some_and(|i| i.len() == items.len())
}

/// A multicard's back for its stored items: marked segments in place when
/// the back still marks exactly those, else rows (see [`multi_back`]).
pub(crate) fn multi_answer(card: &Card, how: Split, items: &[multi::CardItem], asked: &[bool], tap: bool) -> Markup {
    if marked_fits(card, how, items) {
        return marked_back(&card.back, asked, tap);
    }
    multi_back(&pieces_for(card, how, items), asked, tap)
}

/// The back's pieces for its stored items. Should the back no longer split
/// into those (it always does after a save), the items alone are shown.
pub(crate) fn pieces_for(card: &Card, how: Split, items: &[multi::CardItem]) -> Vec<Piece> {
    match multi::split(&card.back, how) {
        Some(p) if p.iter().filter(|p| matches!(p, Piece::Item(_))).count() == items.len() => p,
        _ => items.iter().map(|i| Piece::Item(i.text.clone())).collect(),
    }
}

/// A multicard: the front (with the items you already know while
/// relearning), then the whole back with every item as a row to tap when
/// you remembered it, and one Done. It is all one form, so it also works
/// without JavaScript.
pub(crate) fn multi_study(
    card: &Card,
    how: Split,
    items: &[multi::CardItem],
    q_deck: Option<i64>,
    shown: &Shown,
    extras: &StudyExtras,
) -> Markup {
    let revealed = shown.revealed;
    let asked = multi::asked(card, items, now());
    let n_asked = asked.iter().filter(|a| **a).count();
    let known: Vec<&multi::CardItem> = items.iter().zip(&asked).filter(|(_, a)| !**a).map(|(i, _)| i).collect();
    html! {
        form.list-answer method="post" action="/study/answer" {
            input type="hidden" name="card" value=(card.id);
            input type="hidden" name="reps" value=(card.reps);
            input type="hidden" name="t" value=(shown.at);
            @if let Some(d) = q_deck { input type="hidden" name="deck" value=(d); }
            (extras.sure_field)
            div.card-area {
                (state_tag(card.card_state()))
                section.face.front { (face(&card.front, card.front_image.as_deref())) }
                @if !known.is_empty() && marked_fits(card, how, items) {
                    @if !revealed {
                        div.skeleton { (marked_skeleton(&card.back, &asked)) }
                    }
                } @else if !known.is_empty() {
                    div.list-known {
                        p.known-label { "You know" }
                        ul { @for i in &known { li { (item_md(multi::title(&i.text))) } } }
                        p.more { "and " (n_asked) " more?" }
                    }
                }
                hr.divider hidden[!revealed];
                section.face.back.list id="answer" hidden[!revealed] {
                    (extras.echo)
                    @if let Some(img) = &card.back_image { img src=(format!("/media/{img}")) alt=""; }
                    p.list-how { "Tap the ones you remembered." }
                    (multi_answer(card, how, items, &asked, true))
                }
            }
            (extras.reveal_inside)
            div.answers.cols-1 hidden[!revealed || extras.timed_out] {
                button.btn.primary type="submit" data-key=" " {
                    span.label { "Done" " " span.key { "space" } }
                    span.ivl data-remembered=(n_asked) { "0 of " (n_asked) " remembered" }
                }
            }
        }
    }
}

pub(crate) async fn study(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
    Query(q): Query<StudyQuery>,
) -> WebResult {
    let access = user.access();

    // Answer side of a specific card.
    if let (Some(card_id), Some(1)) = (q.card, q.show) {
        let card = app.card(&access, card_id).await?;
        let deck = app.deck(&access, card.deck_id).await?;
        let shown =
            Shown { at: q.t.unwrap_or_else(now), revealed: true, sure: q.sure.map(|s| s == 1), typed: q.typed.clone() };
        return Ok(study_card(&app, &access, &user.0, msg.as_deref(), &card, &deck, q.deck, &shown)
            .await?
            .into_response());
    }

    if let Some(until) = study::quiet_until(&user.0, now()) {
        return Ok(rest_page(&user.0, msg.as_deref(), q.deck, until).into_response());
    }
    if let Some(due) = app.break_due(&access).await? {
        app.break_shown(user.0.id).await?;
        return Ok(break_page(&user.0, msg.as_deref(), q.deck, due).into_response());
    }

    let Some(sc) = app.next_card(&access, q.deck).await? else {
        let end = session_end(&app, &access, q.deck).await?;
        let body = html! { (flash(msg.as_deref())) (end) };
        return Ok(page(Some(Nav::Study), "Study", Some(&user.0), Some("study-done"), body).into_response());
    };
    Ok(study_card(&app, &access, &user.0, msg.as_deref(), &sc.card, &sc.deck, q.deck, &Shown::front(now()))
        .await?
        .into_response())
}

/// When nothing is left: what today held and when cards come next, or how
/// to get started when there are no cards at all.
async fn session_end(app: &App, access: &Access, deck: Option<i64>) -> Result<Markup, AppError> {
    let any_cards = app.deck_summaries(access).await?.iter().any(|d| d.total > 0);
    let stats = app.stats(access, deck).await?;
    let next = app.next_review_at(access, deck).await?;
    let clock = access.user.clock();
    Ok(html! {
    section.session-end {
        @if any_cards {
            div.end-mark aria-hidden="true" { div.line {} (done_check(26, "check")) }
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
                    "Next cards are due " (format::when(&clock, t)) "."
                } @else {
                    "Nothing else is scheduled yet."
                }
            }
        } @else {
            h1 { "No cards yet" }
            p.summary { "Add a few cards, or import a deck from Anki, and they'll show up here." }
            p { a.btn.primary href=(match deck { Some(d) => format!("/cards/new?deck={d}"), None => "/cards/new".into() }) { "Add cards" } }
        }
        div.end-links {
            a href="/" { "Back to decks" }
            form method="post" action="/study/undo" {
                @if let Some(d) = deck { input type="hidden" name="deck" value=(d); }
                button.link type="submit" { span { "Undo last answer" } }
            }
        }
    }
    })
}

/// "7:00": the time of day of a moment, in the user's zone.
pub(crate) fn clock_time(clock: &crate::scheduler::Clock, ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map_or_else(Default::default, |t| t.with_timezone(&clock.tz).format("%-H:%M").to_string())
}

/// Quiet hours: a rest screen instead of cards.
pub(crate) fn rest_page(user: &User, msg: Option<&str>, deck: Option<i64>, until: i64) -> Markup {
    page(
        Some(Nav::Study),
        "Quiet hours",
        Some(user),
        Some("study-done"),
        html! {
            (flash(msg))
            section.session-end.rest {
                h1 { "Rest until " (clock_time(&user.clock(), until)) }
                p.summary { "You asked for rest until " (format::when(&user.clock(), until)) ". Sleep helps the day's learning settle." }
                p.summary { "Agents won't quiz you now either." }
                div.end-links {
                    a href="/" { "Back to decks" }
                    form method="post" action="/study/anyway" {
                        @if let Some(d) = deck { input type="hidden" name="deck" value=(d); }
                        button.link type="submit" { "Study anyway" }
                    }
                }
            }
        },
    )
}

/// The session has reached the length set in settings, or with smart
/// breaks the answers show tiredness.
pub(crate) fn break_page(user: &User, msg: Option<&str>, deck: Option<i64>, due: study::BreakDue) -> Markup {
    // "a 6-minute break"
    let length = format!("{}-minute", due.length);
    page(
        Some(Nav::Study),
        "Time for a break",
        Some(user),
        Some("study-done"),
        html! {
            (flash(msg))
            section.session-end.rest.break {
                h1 { "Time for a break" }
                @if due.smart && due.slowing {
                    p.summary { "You have studied for " (format::count(due.studied, "minute")) ", and your answers are slowing down. A " (length) " break should bring you back." }
                } @else if due.long {
                    p.summary { "You have studied for " (format::count(due.studied, "minute")) " since your last break. Time for a longer one: " (format::count(due.length, "minute")) ". Get up and move about." }
                } @else if due.smart {
                    p.summary { "You have studied for " (format::count(due.studied, "minute")) " since your last break. A " (length) " break keeps you fresh for the next stretch." }
                } @else {
                    p.summary { "You have studied for " (format::count(due.studied, "minute")) ". Stand up, look out of the window, then carry on if you like." }
                }
                div.end-links {
                    a href="/" { "Stop here" }
                    form id="keep-going" method="post" action="/study/keep-going" {
                        @if let Some(d) = deck { input type="hidden" name="deck" value=(d); }
                        button.link type="submit" { "Keep going" }
                    }
                }
                // With JavaScript the break runs here: parts.js counts it
                // down and sends the form above when it is over.
                ainki-break seconds=(due.length * 60) {
                    p.break-clock role="timer" aria-live="off" { (due.length) ":00" }
                    div.break-line aria-hidden="true" { div.break-fill {} }
                    p.summary { "The next card comes back by itself when the time is up." }
                    p { button.btn type="submit" form="keep-going" { "End break now" } }
                }
            }
        },
    )
}

pub(crate) async fn study_anyway(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    app.lift_quiet(&user.0).await?;
    Ok(redirect(study_url(f.deck)))
}

pub(crate) async fn study_keep_going(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    app.skip_break(&user.0).await?;
    Ok(redirect(study_url(f.deck)))
}

#[derive(Deserialize)]
pub(crate) struct AnswerForm {
    pub(crate) card: i64,
    /// Plain cards: the button pressed.
    pub(crate) rating: Option<i64>,
    /// List cards: positions of the items asked, and of those tapped as
    /// remembered; the rest of the asked ones were missed.
    #[serde(default)]
    pub(crate) asked: Vec<usize>,
    #[serde(default)]
    pub(crate) recalled: Vec<usize>,
    pub(crate) reps: i64,
    pub(crate) t: Option<i64>,
    pub(crate) deck: Option<i64>,
    /// The sure check: "1" sure, "0" not, empty when not asked.
    pub(crate) sure: Option<String>,
    /// Time ran out on a timed card: it counts as Again.
    pub(crate) timeout: Option<u8>,
    /// Measured in the browser, see `study::Timing`.
    pub(crate) think_ms: Option<i64>,
    pub(crate) answer_ms: Option<i64>,
    pub(crate) away_ms: Option<i64>,
    pub(crate) paused_ms: Option<i64>,
    pub(crate) input: Option<String>,
    /// A brought card type's record of what the learner did, as JSON.
    pub(crate) response: Option<String>,
}

pub(crate) async fn study_answer(State(app): State<AppState>, user: WebUser, Form(f): Form<AnswerForm>) -> WebResult {
    // Like Anki, cap the counted thinking time at 60 seconds.
    let duration = f.t.map(|t| ((now() - t).clamp(0, 60)) * 1000);
    let access = user.access();
    let timed_out = f.timeout == Some(1);
    let sure = match f.sure.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    let answered = match f.rating {
        // A timeout misses a multicard's asked items all at once too.
        _ if timed_out => app.answer_card(&access, f.card, Rating::Again, duration, Some(f.reps)).await,
        Some(r) => {
            let rating = Rating::from_i64(r).ok_or_else(|| AppError::bad("invalid rating"))?;
            app.answer_card(&user.access(), f.card, rating, duration, Some(f.reps)).await
        }
        None => {
            let missed = f.asked.into_iter().filter(|n| !f.recalled.contains(n)).collect();
            app.answer_list(&user.access(), f.card, missed, duration, Some(f.reps)).await
        }
    };
    if answered.is_ok() {
        let timing = study::Timing::checked(f.think_ms, f.answer_ms, f.away_ms, f.paused_ms, f.input.as_deref());
        app.note_answer(&access, f.card, sure, timed_out, &timing).await?;
        if let Some(r) = f.response.as_deref().filter(|r| !r.is_empty()) {
            // The rating stands either way; a broken response is the type's
            // bug, not the learner's.
            if let Err(e) = app.note_response(&access, f.card, r).await {
                tracing::warn!("card {}: response not kept: {e:?}", f.card);
            }
        }
        app.note_rest(&access).await?;
    }
    match answered {
        Ok(_) | Err(AppError::Conflict(_)) => Ok(redirect(study_url(f.deck))),
        Err(e) => Err(e.into()),
    }
}

#[derive(Deserialize)]
pub(crate) struct DeckForm {
    pub(crate) deck: Option<i64>,
}

pub(crate) async fn study_undo(State(app): State<AppState>, user: WebUser, Form(f): Form<DeckForm>) -> WebResult {
    let restored = app.undo_last(&user.access()).await?;
    let base = study_url(f.deck);
    let sep = if base.contains('?') { '&' } else { '?' };
    Ok(match restored {
        Some(card) => redirect(format!("{base}{sep}card={}&show=1&t={}", card.id, now())),
        None => redirect_msg(base, "Nothing to undo."),
    })
}

/// After time away: offer catch-up mode, or say what it does while on.
pub(crate) fn catch_up_note(user: &User, overdue: i64) -> Markup {
    html! {
        @if user.catch_up {
            form.catch-up method="post" action="/settings/catch-up" {
                p.small.muted {
                    @if overdue < 20 {
                        "You have caught up. "
                        button.link type="submit" { "Turn catch-up mode off" }
                    } @else {
                        "Catch-up mode: no new cards, at most " (user.catch_up_reviews) " reviews a day, the ones you most likely still know first. "
                        button.link type="submit" { "Turn off catch-up mode" }
                    }
                }
            }
        } @else if overdue >= study::BACKLOG_HINT {
            form.catch-up method="post" action="/settings/catch-up" {
                input type="hidden" name="on" value="1";
                p.small.muted {
                    (format::number(overdue)) " cards piled up while you were away. "
                    button.link type="submit" { "Catch up over a few days" }
                }
            }
        }
    }
}
