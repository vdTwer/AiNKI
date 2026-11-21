//! The Learn pages: the built-in lessons, each addable as a deck.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

pub(crate) fn evidence_tag(e: lessons::Evidence) -> Markup {
    html! { span.evidence.(e.class()) { (e.label()) } }
}

pub(crate) async fn learn_page(user: WebUser) -> WebResult {
    let all = lessons::all();
    Ok(layout_in(
        Nav::Learn,
        "Learn",
        Some(&user.0),
        html! {
            h1 { "Learn" }
            p.lead { "Short lessons on how AiNKI schedules your cards, what research says about memory, and how to write cards that stick. Each says how strong the evidence is, and each can be added as a deck." }
            @for (key, name, sub) in lessons::GROUPS {
                h2 { (name) }
                p.muted.small { (sub) }
                ul.list.links.lessons {
                    @for l in all.iter().filter(|l| l.group == *key) {
                        li {
                            a.row href=(format!("/learn/{}", l.slug)) {
                                span.grow {
                                    span { (l.title) }
                                    span.sub.muted.small { (l.summary) }
                                }
                                (evidence_tag(l.evidence))
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

pub(crate) async fn lesson_page(
    State(app): State<AppState>,
    user: WebUser,
    Flash(msg): Flash,
    Path(slug): Path<String>,
) -> WebResult {
    let lesson = lessons::find(&slug).ok_or(AppError::NotFound)?;
    let access = user.access();
    let deck = app.deck_by_name(&access, &lessons::deck_name(lesson)).await?;
    let all = lessons::all();
    let i = all.iter().position(|l| l.slug == lesson.slug).unwrap_or(0);
    let next = all.get(i + 1);
    Ok(layout_in(
        Nav::Learn,
        &lesson.title,
        Some(&user.0),
        html! {
            (flash(msg.as_deref()))
            (parent("/learn", "Learn"))
            article.lesson {
                h1 { (lesson.title) }
                p.lesson-meta { (evidence_tag(lesson.evidence)) }
                div.md.prose { (PreEscaped(markdown::render(&lesson.body))) }
            }
            div.lesson-deck {
                @if let Some(d) = deck {
                    p.muted.small { "This lesson is in your decks. " a href=(format!("/decks/{}", d.id)) { "Open it" } }
                } @else {
                    form method="post" action=(format!("/learn/{}/deck", lesson.slug)) {
                        p.muted.small { "Learn this lesson with " (count(num::usize_i64(lesson.cards.len()), "card")) "." }
                        button.btn type="submit" { "Add as a deck" }
                    }
                }
            }
            @if let Some(n) = next {
                p.next-lesson { "Next: " a href=(format!("/learn/{}", n.slug)) { (n.title) } }
            }
        },
    )
    .into_response())
}

pub(crate) async fn lesson_deck(State(app): State<AppState>, user: WebUser, Path(slug): Path<String>) -> WebResult {
    let lesson = lessons::find(&slug).ok_or(AppError::NotFound)?;
    let access = user.access();
    let name = lessons::deck_name(lesson);
    if let Some(d) = app.deck_by_name(&access, &name).await? {
        return Ok(redirect_msg(format!("/decks/{}", d.id), "You already have this deck."));
    }
    or_back(
        format!("/learn/{slug}"),
        async {
            let deck = app.create_deck(&access, &name, &lesson.summary, None).await?;
            for (front, back) in &lesson.cards {
                app.create_card(&access, deck.id, NewCard::new(front, back)).await?;
            }
            Ok(redirect_msg(format!("/decks/{}", deck.id), "Lesson added as a deck."))
        }
        .await,
    )
}
