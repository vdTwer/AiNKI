//! The quick find: fuzzy search over decks, cards and places.

#[allow(
    clippy::wildcard_imports,
    reason = "a page sees the shared web plumbing and its sibling pages, as when they were one file"
)]
use crate::web::*;

#[derive(Deserialize)]
pub(crate) struct FindQuery {
    #[serde(default)]
    pub(crate) q: String,
}

/// Places the quick find knows besides decks and cards. The words
/// after the name are what people may type for it.
pub(crate) const FIND_PLACES: &[(&str, &str, &str)] = &[
    ("Today", "/", "home decks start"),
    ("Study", "/study", "review learn cards now"),
    ("Add a card", "/cards/new", "new card write"),
    ("New deck", "/decks/new", "create deck"),
    ("Import", "/import", "anki apkg csv upload"),
    ("Stats", "/stats", "statistics progress charts week retention"),
    ("Settings", "/settings", "preferences options account password quiet hours breaks"),
    ("Presets", "/presets", "fsrs retention limits steps"),
    ("Agent access", "/access", "tokens claude mcp api keys"),
    ("Cards to look at", "/attention", "coach leeches attention"),
];

/// Results shown at most, and at most this many of them cards.
pub(crate) const FIND_MAX: usize = 9;
pub(crate) const FIND_MAX_CARDS: usize = 5;

/// One line of the quick find: what it is, where it leads.
pub(crate) struct Found {
    pub(crate) kind: &'static str,
    pub(crate) title: String,
    pub(crate) sub: String,
    pub(crate) href: String,
    pub(crate) score: i32,
}

/// The best of a name and its other words: the name counts more. The other
/// words (a summary, a deck's description) are matched one word at a time,
/// so letters scattered across a long text don't count.
pub(crate) fn find_score(q: &str, name: &str, also: &str) -> Option<i32> {
    let by_name = fuzzy::score(q, name).map(|s| s + 6);
    let by_also = if also.is_empty() {
        None
    } else {
        let words: Vec<&str> = name.split_whitespace().chain(also.split_whitespace()).collect();
        q.split_whitespace().map(|qw| words.iter().filter_map(|w| fuzzy::score(qw, w)).max()).sum::<Option<i32>>()
    };
    by_name.max(by_also)
}

/// Fits a card's words to one line of the list.
pub(crate) fn find_line(text: &str, max: usize) -> String {
    let plain = markdown::plain(text);
    if plain.chars().count() <= max {
        return plain;
    }
    let cut: String = plain.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a).filter(|a| a.len() > max / 2).unwrap_or(&cut);
    format!("{cut}…")
}

/// A deck's label in quick find.
pub(crate) const fn deck_kind(status: DeckStatus) -> &'static str {
    match status {
        DeckStatus::Active => "Deck",
        DeckStatus::Paused => "Paused deck",
        DeckStatus::Archived => "Archived deck",
    }
}

/// The quick find as a page: a search field and what it finds. Works
/// without JavaScript; with it, `<ainki-find>` (parts.js) opens the same
/// form in a dialog from anywhere and swaps in `#find-results` as you type.
pub(crate) async fn find_page(State(app): State<AppState>, user: WebUser, Query(f): Query<FindQuery>) -> WebResult {
    let access = user.access();
    let q = f.q.trim();
    let q = &q[..q.char_indices().nth(64).map_or(q.len(), |(i, _)| i)];
    let decks = app.decks(&access).await?;
    let found = if q.is_empty() { browse(&decks) } else { find(q, &decks, &app.card_texts(&access).await?) };
    let body = html! {
        h1 { "Find" }
        (find_form(q))
        (find_results(q, &found))
    };
    Ok(layout("Find", Some(&user.0), body).into_response())
}

/// The search field, on the page and in the dialog alike.
pub(crate) fn find_form(q: &str) -> Markup {
    html! {
        form.search.find-form method="get" action="/find" role="search" {
            input type="search" name="q" value=(q) placeholder="Decks, cards, places" aria-label="Find"
                autocomplete="off" spellcheck="false";
            button type="submit" { "Find" }
        }
    }
}

fn find_results(q: &str, found: &[Found]) -> Markup {
    html! {
        div #find-results {
            @if found.is_empty() && !q.is_empty() {
                p.muted { "Nothing found." }
            }
            ul.list.find-list {
                @for f in found {
                    li {
                        a.row href=(f.href) {
                            span.grow { (f.title) @if !f.sub.is_empty() { span.sub { (f.sub) } } }
                            @if f.kind != "place" { span.small.muted { (f.kind) } }
                        }
                    }
                }
            }
        }
    }
}

impl Found {
    fn place(name: &str, href: &str, score: i32) -> Self {
        Self { kind: "place", title: name.to_string(), sub: String::new(), href: href.to_string(), score }
    }

    fn deck(d: &Deck, title: String, score: i32) -> Self {
        Self { kind: "deck", title, sub: deck_kind(d.status).into(), href: format!("/decks/{}", d.id), score }
    }
}

/// Nothing typed yet: the main places, then the decks.
fn browse(decks: &[Deck]) -> Vec<Found> {
    let places = FIND_PLACES.iter().filter(|p| matches!(p.1, "/" | "/stats" | "/settings"));
    let mut found: Vec<Found> = places.map(|(name, href, _)| Found::place(name, href, 0)).collect();
    found.extend(decks.iter().map(|d| Found::deck(d, deck_plain(&d.name), 0)));
    found.truncate(FIND_MAX);
    found
}

/// The best matches for `q` among places, decks and cards. Places
/// rank above decks on a tie; cards come in by their own
/// score, at most `FIND_MAX_CARDS` of them.
fn find(q: &str, decks: &[Deck], cards: &[crate::service::CardText]) -> Vec<Found> {
    let mut found: Vec<Found> = FIND_PLACES
        .iter()
        .filter_map(|(name, href, also)| Some(Found::place(name, href, find_score(q, name, also)? + 4)))
        .collect();
    let names: HashMap<i64, String> = decks.iter().map(|d| (d.id, deck_plain(&d.name))).collect();
    for d in decks {
        let title = names.get(&d.id).cloned().unwrap_or_default();
        if let Some(s) = find_score(q, &title, &d.description) {
            found.push(Found::deck(d, title, s + 3));
        }
    }
    let mut matched: Vec<Found> = cards
        .iter()
        .filter_map(|c| {
            // A match on the back alone counts for less.
            let s = fuzzy::score(q, &c.front).max(fuzzy::score(q, &c.back).map(|s| s - 10))?;
            Some(Found {
                kind: "card",
                title: find_line(&c.front, 80),
                sub: names.get(&c.deck_id).cloned().unwrap_or_default(),
                href: format!("/cards/{}", c.id),
                score: s,
            })
        })
        .collect();
    // Newest first already, so equal scores keep the newer card on top.
    matched.sort_by_key(|b| std::cmp::Reverse(b.score));
    matched.truncate(FIND_MAX_CARDS);
    found.extend(matched);
    found.sort_by_key(|b| std::cmp::Reverse(b.score));
    // Letters scattered through a long text match too: next to a clear
    // match such stragglers are noise, so they go.
    if let Some(top) = found.first().map(|f| f.score).filter(|&s| s > 0) {
        found.retain(|f| f.score * 2 >= top);
    }
    found.truncate(FIND_MAX);
    found
}

#[cfg(test)]
mod tests {
    use super::{browse, find};
    use crate::service::CardText;

    #[test]
    fn a_place_comes_first_and_cards_follow_by_score() {
        let cards = [CardText { id: 7, deck_id: 1, front: "Statistics of memory".into(), back: String::new() }];
        let found = find("stats", &[], &cards);
        assert_eq!(found.first().map(|f| f.href.as_str()), Some("/stats"));
        assert!(found.iter().any(|f| f.href == "/cards/7"));
        assert!(find("zzzzqqq", &[], &cards).is_empty());
    }

    #[test]
    fn nothing_typed_lists_the_main_places() {
        let hrefs: Vec<String> = browse(&[]).into_iter().map(|f| f.href).collect();
        assert_eq!(hrefs, ["/", "/stats", "/settings"]);
    }
}
