//! The quick find: fuzzy search over decks, cards and places, as JSON for the dot rail.

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

/// Places the quick find knows besides decks, lessons and cards. The words
/// after the name are what people may type for it.
pub(crate) const FIND_PLACES: &[(&str, &str, &str)] = &[
    ("Today", "/", "home decks start"),
    ("Study", "/study", "review learn cards now"),
    ("Add a card", "/cards/new", "new card write"),
    ("New deck", "/decks/new", "create deck"),
    ("Import", "/import", "anki apkg csv upload"),
    ("Stats", "/stats", "statistics progress charts week retention"),
    ("Learn", "/learn", "lessons how memory works"),
    ("Settings", "/settings", "preferences options account password quiet hours breaks"),
    ("Presets", "/presets", "fsrs retention limits steps"),
    ("Agent access", "/access", "tokens claude mcp api keys"),
    ("Cards to look at", "/attention", "coach leeches attention"),
];

/// Results shown at most, and at most this many of them cards.
pub(crate) const FIND_MAX: usize = 9;
pub(crate) const FIND_MAX_CARDS: usize = 5;

/// One line of the quick find: what it is, where it leads.
#[derive(Serialize)]
pub(crate) struct Found {
    pub(crate) kind: &'static str,
    pub(crate) title: String,
    pub(crate) sub: String,
    pub(crate) href: String,
    #[serde(skip)]
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

pub(crate) async fn find_json(State(app): State<AppState>, user: WebUser, Query(f): Query<FindQuery>) -> WebResult {
    let access = user.access();
    let q = f.q.trim();
    let q = &q[..q.char_indices().nth(64).map_or(q.len(), |(i, _)| i)];
    let decks = app.decks(&access).await?;
    let mut found: Vec<Found> = Vec::new();

    // Nothing typed yet: the places, then the decks.
    if q.is_empty() {
        for (name, href, _) in FIND_PLACES.iter().filter(|p| matches!(p.1, "/" | "/stats" | "/learn" | "/settings")) {
            found.push(Found {
                kind: "place",
                title: name.to_string(),
                sub: String::new(),
                href: href.to_string(),
                score: 0,
            });
        }
        for d in &decks {
            found.push(Found {
                kind: "deck",
                title: deck_plain(&d.name),
                sub: deck_kind(d.status).into(),
                href: format!("/decks/{}", d.id),
                score: 0,
            });
        }
        found.truncate(FIND_MAX);
        return Ok((
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({ "q": q, "results": found }).to_string(),
        )
            .into_response());
    }

    for (name, href, also) in FIND_PLACES {
        if let Some(s) = find_score(q, name, also) {
            found.push(Found {
                kind: "place",
                title: name.to_string(),
                sub: String::new(),
                href: href.to_string(),
                score: s + 4,
            });
        }
    }
    let mut names = HashMap::new();
    for d in &decks {
        let title = deck_plain(&d.name);
        if let Some(s) = find_score(q, &title, &d.description) {
            found.push(Found {
                kind: "deck",
                title: title.clone(),
                sub: deck_kind(d.status).into(),
                href: format!("/decks/{}", d.id),
                score: s + 3,
            });
        }
        names.insert(d.id, title);
    }
    for l in lessons::all() {
        if let Some(s) = find_score(q, &l.title, &l.summary) {
            found.push(Found {
                kind: "lesson",
                title: l.title.clone(),
                sub: "Lesson".into(),
                href: format!("/learn/{}", l.slug),
                score: s + 1,
            });
        }
    }
    let mut cards: Vec<Found> = Vec::new();
    for c in app.card_texts(&access).await? {
        let front = fuzzy::score(q, &c.front);
        // A match on the back alone counts for less.
        let back = fuzzy::score(q, &c.back).map(|s| s - 10);
        let Some(s) = front.max(back) else { continue };
        let deck = names.get(&c.deck_id).cloned().unwrap_or_default();
        cards.push(Found {
            kind: "card",
            title: find_line(&c.front, 80),
            sub: deck,
            href: format!("/cards/{}", c.id),
            score: s,
        });
    }
    // Newest first already, so equal scores keep the newer card on top.
    cards.sort_by_key(|b| std::cmp::Reverse(b.score));
    cards.truncate(FIND_MAX_CARDS);
    found.extend(cards);
    found.sort_by_key(|b| std::cmp::Reverse(b.score));
    // Letters scattered through a long text match too: next to a clear
    // match such stragglers are noise, so they go.
    if let Some(top) = found.first().map(|f| f.score).filter(|&s| s > 0) {
        found.retain(|f| f.score * 2 >= top);
    }
    found.truncate(FIND_MAX);
    Ok(([(header::CONTENT_TYPE, "application/json")], serde_json::json!({ "q": q, "results": found }).to_string())
        .into_response())
}
