//! The card coach's pages: cards that need a look, and splitting a card.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

/// Whether the back offers a split into cards.
pub(crate) fn splittable(card: &Card) -> bool {
    card.multi.is_some() || !multi::splits(&card.back).is_empty()
}

/// What the coach noticed about a card, each with what can be done. The
/// card is never changed until the user picks something.
pub(crate) fn coach_box(card: &Card, flags: &[coach::Flag]) -> Markup {
    let id = card.id;
    html! {
        @if !flags.is_empty() {
            section.coach aria-label="The coach noticed" {
                @for f in flags {
                    div.coach-flag {
                        p.coach-label { (f.kind.label()) }
                        p { (f.reason) }
                        @if f.kind == coach::Kind::Leech {
                            p.muted.small { "Each lapse sends it back to short gaps, so it costs more time than many easy cards. Rewrite it so it asks one clear thing, add an image or a memory hook, or split it. Suspending takes it out of your reviews." }
                        }
                        form.actions method="post" action=(format!("/cards/{id}/keep-{}", f.kind.as_str())) {
                            @if f.kind != coach::Kind::Duplicate && card.multi.is_none() && coach::list_items(&card.back) >= 2 {
                                button.btn type="submit" formaction=(format!("/cards/{id}/learn-items")) { "Learn item by item" }
                            }
                            @if f.kind != coach::Kind::Duplicate && splittable(card) {
                                a.btn href=(format!("/cards/{id}/split")) { "Split into cards…" }
                            }
                            @if let Some(other) = f.other_card {
                                a.btn href=(format!("/cards/{other}")) { "Open the other card" }
                            }
                            @if f.kind == coach::Kind::Leech && !card.suspended {
                                button.btn type="submit" formaction=(format!("/cards/{id}/suspend")) { "Suspend" }
                            }
                            button.btn type="submit" { "Keep as is" }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct AttentionQuery {
    pub(crate) deck: Option<i64>,
}

/// Every card the coach flagged, in one deck or all of them.
pub(crate) async fn attention_page(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
    Query(q): Query<AttentionQuery>,
) -> WebResult {
    let access = user.access();
    let found = app.attention(&access, q.deck).await?;
    let deck = match q.deck {
        Some(id) => Some(app.deck(&access, id).await?),
        None => None,
    };
    Ok(layout_in(
        Nav::Decks,
        "Needs a look",
        Some(&user.0),
        html! {
            (flash(msg.as_deref()))
            @if let Some(d) = &deck { (parent(&format!("/decks/{}", d.id), &deck_plain(&d.name))) }
            h1 { "Needs a look" }
            @if found.is_empty() {
                p { "Nothing right now. The coach looks for cards you keep forgetting, cards that ask for several things at once, and cards that look alike." }
            } @else {
                p.muted {
                    "The coach noticed " (count(num::usize_i64(found.len()), "card")) ". Open one to see why and what you can do. Nothing changes unless you pick something."
                }
                ul.list.cards {
                    @for a in &found {
                        li {
                            a.row href=(format!("/cards/{}", a.card.id)) {
                                span.grow {
                                    @if a.card.front.is_empty() { "[image]" } @else { (truncate(&a.card.front, 80)) }
                                    br;
                                    span.muted.small { (a.flags.iter().map(|f| f.kind.label()).collect::<Vec<_>>().join(" · ")) }
                                }
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

/// The cards a split starts from: one per item of the back, each with the
/// original front. Without items, the card itself and an empty one.
pub(crate) fn split_drafts(
    card: &Card,
    items: &[multi::CardItem],
) -> Vec<(String, String)> {
    let texts: Vec<String> = if items.is_empty() {
        multi::splits(&card.back)
            .first()
            .and_then(|&how| multi::items(&card.back, how))
            .unwrap_or_default()
    } else {
        items.iter().map(|i| i.text.clone()).collect()
    };
    if texts.len() >= 2 {
        texts.into_iter().map(|t| (card.front.clone(), t)).collect()
    } else {
        vec![
            (card.front.clone(), card.back.clone()),
            (String::new(), String::new()),
        ]
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct SplitQuery {
    /// How many cards to show, when more than the back gives.
    pub(crate) n: Option<usize>,
}

pub(crate) fn split_view(
    user: &User,
    card: &Card,
    drafts: &[(String, String)],
    keep: bool,
    error: Option<&str>,
) -> Markup {
    let id = card.id;
    layout_in(
        Nav::Decks,
        "Split card",
        Some(user),
        html! {
            (parent(&format!("/cards/{id}"), "Back to the card"))
            h1 { "Split into cards" }
            p.muted { "One thing per card is easier to remember. Each new card starts fresh. Change the fronts so each asks for exactly its answer." }
            (form_error(error))
            form.split-form method="post" action=(format!("/cards/{id}/split")) {
                @for (n, (front, back)) in drafts.iter().enumerate() {
                    div.sheet {
                        div.face-edit data-face="front" {
                            label.face-label for=(format!("front_{n}")) { "Card " (n + 1) ": front" }
                            textarea id=(format!("front_{n}")) name=(format!("front_{n}")) rows="2" { (front) }
                        }
                        hr.divider;
                        div.face-edit data-face="back" {
                            label.face-label for=(format!("back_{n}")) { "Back" }
                            textarea id=(format!("back_{n}")) name=(format!("back_{n}")) rows="2" { (back) }
                        }
                    }
                }
                p.small { a href=(format!("/cards/{id}/split?n={}", drafts.len() + 1)) { "One more card" } " · Leave a card empty to skip it." }
                label.check { input type="checkbox" name="keep" value="1" checked[keep]; " Keep the original card, suspended" }
                div.form-foot { button.btn.primary.wide type="submit" { "Make " (drafts.len()) " cards" } }
            }
        },
    )
}

pub(crate) async fn split_page(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Query(q): Query<SplitQuery>,
) -> WebResult {
    let access = user.access();
    let card = app.card(&access, id).await?;
    let items = app.card_items(&card).await?;
    let mut drafts = split_drafts(&card, &items);
    if let Some(n) = q.n.filter(|n| *n > drafts.len()) {
        drafts.resize(n.min(50), (String::new(), String::new()));
    }
    Ok(split_view(&user.0, &card, &drafts, false, None).into_response())
}

pub(crate) async fn split_submit(
    State(app): State<AppState>,
    user: WebUser,
    Path(id): Path<i64>,
    Form(f): Form<HashMap<String, String>>,
) -> WebResult {
    let access = user.access();
    let card = app.card(&access, id).await?;
    let mut drafts = Vec::new();
    for n in 0..50 {
        match (f.get(&format!("front_{n}")), f.get(&format!("back_{n}"))) {
            (None, None) => break,
            (front, back) => drafts.push((
                front.cloned().unwrap_or_default(),
                back.cloned().unwrap_or_default(),
            )),
        }
    }
    let keep = f.contains_key("keep");
    match app.split_card(&access, id, drafts.clone(), keep).await {
        Ok(cards) => Ok(redirect_msg(
            format!("/decks/{}", card.deck_id),
            &format!(
                "Split into {}.",
                count(num::usize_i64(cards.len()), "card")
            ),
        )),
        Err(e) => {
            let msg = fixable(&e).ok_or(e)?;
            Ok(form_again(split_view(
                &user.0,
                &card,
                &drafts,
                keep,
                Some(&msg),
            )))
        }
    }
}
