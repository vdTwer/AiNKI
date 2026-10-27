//! The card coach: simple rules that point at cards worth a second look.
//! Most forgetting comes from cards that ask too much at once, not from the
//! schedule. The coach never changes a card by itself; it says what it
//! noticed and the user decides.
//!
//! The rules follow common practice (Anki, SuperMemo, RemNote) and the
//! FSRS community's findings; none of them is a tested threshold, so they
//! stay simple and explainable.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    App,
    auth::Access,
    error::{AppError, Result},
    multi::{self, Split},
    now,
    scheduler::CardState,
    service::{Card, placeholders},
};

/// A card is a leech after this many lapses...
pub const LEECH_LAPSES: i64 = 5;
/// ...while its memory is still short (stability in days). A card forgotten
/// five times over ten years is fine; one forgotten five times in a month
/// is not. Stability stands in for the interval, which a card in
/// relearning no longer has.
const LEECH_STABILITY_DAYS: f64 = 60.0;
/// "Keep as is" holds until this many more lapses, like Anki warning again
/// at every half threshold.
const AGAIN_AFTER_LAPSES: i64 = 3;
/// A back that lists several things, forgotten this often, asks for too
/// much at once.
const SPLIT_LAPSES: i64 = 3;
/// Answers that take this long on average (over the last few timed ones)
/// with lapses as well point the same way. Answer times are counted up to
/// 60 seconds, so 45 is already a long think.
const SLOW_SECS: f64 = 45.0;
const SLOW_LAPSES: i64 = 2;
/// While writing: from this many list items on, suggest learning them item
/// by item.
const MULTI_HINT_ITEMS: usize = 3;
/// Fronts this alike (shared three-letter pieces) are near-duplicates.
const SIMILAR: f64 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Forgotten again and again.
    Leech,
    /// Asks for several things at once, or takes very long.
    Split,
    /// Almost the same front as another card; the two get mixed up.
    Duplicate,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Leech => "leech",
            Kind::Split => "split",
            Kind::Duplicate => "duplicate",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "leech" => Some(Kind::Leech),
            "split" => Some(Kind::Split),
            "duplicate" => Some(Kind::Duplicate),
            _ => None,
        }
    }

    /// A short label for lists.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Leech => "Keeps slipping",
            Kind::Split => "Asks a lot at once",
            Kind::Duplicate => "Looks like another card",
        }
    }
}

/// One thing the coach noticed about a card.
#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub kind: Kind,
    /// What was noticed, in a sentence.
    pub reason: String,
    /// For a leech on a multicard: the item (from 0) that keeps slipping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item: Option<usize>,
    /// For a duplicate: the other card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_card: Option<i64>,
}

/// A card and what the coach noticed about it.
#[derive(Debug, Serialize)]
pub struct Attention {
    pub card: Card,
    pub flags: Vec<Flag>,
}

/// Lowercase words only: no Markdown, punctuation or extra spaces. Two
/// fronts that read the same compare equal.
pub fn normalize(text: &str) -> String {
    let mut out = String::new();
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

fn trigrams(s: &str) -> HashSet<[char; 3]> {
    let padded: Vec<char> = format!("  {s} ").chars().collect();
    padded.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

/// How alike two normalized fronts are, from 0 to 1: the share of their
/// three-letter pieces they have in common.
pub fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let (x, y) = (trigrams(a), trigrams(b));
    let shared = x.intersection(&y).count();
    let all = x.union(&y).count();
    if all == 0 { 0.0 } else { shared as f64 / all as f64 }
}

/// Very short fronts ("Hund") only count when they are the same; three
/// letters more or less would make anything look alike.
fn near_duplicate(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    a.chars().count().min(b.chars().count()) >= 12 && similarity(a, b) >= SIMILAR
}

/// How many list items the back has (0 when it has no list of two or more).
pub fn list_items(back: &str) -> usize {
    multi::items(back, Split::List).map(|i| i.len()).unwrap_or(0)
}

fn is_leech(lapses: i64, stability: Option<f64>, state: CardState) -> bool {
    lapses >= LEECH_LAPSES
        && matches!(state, CardState::Review | CardState::Relearning)
        && stability.is_none_or(|s| s < LEECH_STABILITY_DAYS)
}

/// Whether a flag the user kept as is should show again.
fn back_again(kind: Kind, dismissed_at: Option<i64>, lapses: i64) -> bool {
    match dismissed_at {
        None => true,
        Some(_) if kind == Kind::Duplicate => false,
        Some(at) => lapses >= at + AGAIN_AFTER_LAPSES,
    }
}

/// The leech flag of a plain card, or of a multicard's weakest item.
fn leech_flag(card: &Card, items: &[multi::CardItem]) -> Option<Flag> {
    if card.multi.is_some() {
        let (n, item) = items
            .iter()
            .enumerate()
            .filter(|(_, i)| is_leech(i.lapses, i.stability, i.card_state()))
            .max_by_key(|(_, i)| i.lapses)?;
        return Some(Flag {
            kind: Kind::Leech,
            reason: format!(
                "Item {} (\"{}\") was forgotten {} times and still doesn't stay.",
                n + 1,
                short(multi::title(&item.text), 40),
                item.lapses
            ),
            item: Some(n),
            other_card: None,
        });
    }
    is_leech(card.lapses, card.stability, card.card_state()).then(|| Flag {
        kind: Kind::Leech,
        reason: format!("Forgotten {} times, and it still doesn't stay.", card.lapses),
        item: None,
        other_card: None,
    })
}

fn split_flag(card: &Card, avg_secs: Option<f64>) -> Option<Flag> {
    let lapses = card.lapses;
    let items = if card.multi.is_none() { list_items(&card.back) } else { 0 };
    let reason = if items >= 2 && lapses >= SPLIT_LAPSES {
        format!("The back lists {items} things and the card was forgotten {lapses} times. Each thing on its own is easier to remember.")
    } else if lapses >= SLOW_LAPSES && avg_secs.is_some_and(|s| s >= SLOW_SECS) {
        format!(
            "Answers take about {:.0} seconds and the card was forgotten {lapses} times. A card that asks for less is quicker and sticks better.",
            avg_secs.unwrap_or_default()
        )
    } else {
        return None;
    };
    Some(Flag { kind: Kind::Split, reason, item: None, other_card: None })
}

/// Hints while writing a card, before it has any history.
pub fn write_hints(back: &str, multi: Option<Split>) -> Vec<String> {
    let mut hints = Vec::new();
    let n = list_items(back);
    if multi.is_none() && n >= MULTI_HINT_ITEMS {
        hints.push(format!(
            "The back lists {n} things. Learn them item by item, and AiNKI tracks each one; or make a card for each."
        ));
    }
    hints
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>().trim_end())
    }
}

/// A card that looks like the one being written.
#[derive(Debug, Serialize)]
pub struct Similar {
    pub card_id: i64,
    pub deck_id: i64,
    pub front: String,
}

impl App {
    async fn dismissals(&self, card_ids: &[i64]) -> Result<HashMap<(i64, Kind), i64>> {
        let mut out = HashMap::new();
        for chunk in card_ids.chunks(500) {
            let sql = format!(
                "SELECT card_id, kind, lapses FROM coach_dismissals WHERE card_id IN ({})",
                placeholders(chunk.len())
            );
            let mut q = sqlx::query_as::<_, (i64, String, i64)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            for (id, kind, lapses) in q.fetch_all(&self.db).await? {
                if let Some(k) = Kind::parse(&kind) {
                    out.insert((id, k), lapses);
                }
            }
        }
        Ok(out)
    }

    /// Average thinking time over each card's last five timed answers, for
    /// cards with at least three.
    async fn answer_seconds(&self, card_ids: &[i64]) -> Result<HashMap<i64, f64>> {
        let mut out = HashMap::new();
        for chunk in card_ids.chunks(500) {
            let sql = format!(
                "SELECT card_id, AVG(duration_ms) FROM (
                    SELECT card_id, duration_ms,
                        ROW_NUMBER() OVER (PARTITION BY card_id ORDER BY reviewed_at DESC, id DESC) AS n
                    FROM review_log
                    WHERE card_id IN ({}) AND rating > 0 AND duration_ms > 0 AND source != 'import')
                 WHERE n <= 5 GROUP BY card_id HAVING COUNT(*) >= 3",
                placeholders(chunk.len())
            );
            let mut q = sqlx::query_as::<_, (i64, f64)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            for (id, ms) in q.fetch_all(&self.db).await? {
                out.insert(id, ms / 1000.0);
            }
        }
        Ok(out)
    }

    /// Every visible card's id, deck and normalized front.
    async fn fronts(&self, access: &Access) -> Result<Vec<(i64, i64, String)>> {
        let rows: Vec<(i64, i64, String)> =
            sqlx::query_as("SELECT id, deck_id, front FROM cards WHERE user_id = ?")
                .bind(access.user_id())
                .fetch_all(&self.db)
                .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, deck, _)| access.can_see_deck(*deck))
            .map(|(id, deck, front)| (id, deck, normalize(&front)))
            .collect())
    }

    /// Cards whose front reads (almost) like `front`, other than `except`.
    pub async fn similar_cards(&self, access: &Access, front: &str, except: Option<i64>) -> Result<Vec<Similar>> {
        let want = normalize(front);
        if want.is_empty() {
            return Ok(vec![]);
        }
        let mut hits: Vec<(f64, i64, i64)> = self
            .fronts(access)
            .await?
            .into_iter()
            .filter(|(id, _, f)| Some(*id) != except && near_duplicate(&want, f))
            .map(|(id, deck, f)| (similarity(&want, &f), id, deck))
            .collect();
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(3);
        let mut out = Vec::new();
        for (_, id, deck_id) in hits {
            let front: String = sqlx::query_scalar("SELECT front FROM cards WHERE id = ?")
                .bind(id)
                .fetch_one(&self.db)
                .await?;
            out.push(Similar { card_id: id, deck_id, front });
        }
        Ok(out)
    }

    /// What the coach notices about one card.
    pub async fn card_flags(&self, access: &Access, card: &Card) -> Result<Vec<Flag>> {
        let items = self.card_items(card).await?;
        let secs = self.answer_seconds(&[card.id]).await?;
        let dismissed = self.dismissals(&[card.id]).await?;
        let mut flags: Vec<Flag> = leech_flag(card, &items)
            .into_iter()
            .chain(split_flag(card, secs.get(&card.id).copied()))
            .collect();
        let me = normalize(&card.front);
        if let Some((other, _, _)) = self
            .fronts(access)
            .await?
            .into_iter()
            .find(|(id, _, f)| *id != card.id && near_duplicate(&me, f))
        {
            flags.push(duplicate_flag(other));
        }
        flags.retain(|f| back_again(f.kind, dismissed.get(&(card.id, f.kind)).copied(), lapses_for(card, &items, f)));
        Ok(flags)
    }

    /// Only the leech flag, cheap enough for every studied card.
    pub async fn leech_note(&self, card: &Card) -> Result<Option<Flag>> {
        let items = self.card_items(card).await?;
        let Some(flag) = leech_flag(card, &items) else { return Ok(None) };
        let dismissed = self.dismissals(&[card.id]).await?;
        let lapses = lapses_for(card, &items, &flag);
        Ok(back_again(Kind::Leech, dismissed.get(&(card.id, Kind::Leech)).copied(), lapses).then_some(flag))
    }

    /// Every flagged card, in one deck or all visible decks: leeches first,
    /// then the rest, newest problems first.
    pub async fn attention(&self, access: &Access, deck_id: Option<i64>) -> Result<Vec<Attention>> {
        if let Some(id) = deck_id {
            self.deck(access, id).await?;
        }
        let in_scope = |deck: i64| deck_id.is_none_or(|d| d == deck) && access.can_see_deck(deck);

        // Leech and split candidates: anything forgotten at least twice.
        let candidates: Vec<Card> = sqlx::query_as(
            "SELECT * FROM cards WHERE user_id = ? AND suspended = 0
             AND (lapses >= ? OR (multi IS NOT NULL AND EXISTS
                 (SELECT 1 FROM card_items i WHERE i.card_id = cards.id AND i.lapses >= ?)))",
        )
        .bind(access.user_id())
        .bind(SLOW_LAPSES.min(SPLIT_LAPSES))
        .bind(LEECH_LAPSES)
        .fetch_all(&self.db)
        .await?;
        let candidates: Vec<Card> = candidates.into_iter().filter(|c| in_scope(c.deck_id)).collect();
        let ids: Vec<i64> = candidates.iter().map(|c| c.id).collect();
        let secs = self.answer_seconds(&ids).await?;

        // Each flag with the lapse count it is judged by.
        let mut found: HashMap<i64, (Option<Card>, Vec<(Flag, i64)>)> = HashMap::new();
        for card in candidates {
            let items = self.card_items(&card).await?;
            let flags: Vec<(Flag, i64)> = leech_flag(&card, &items)
                .into_iter()
                .chain(split_flag(&card, secs.get(&card.id).copied()))
                .map(|f| {
                    let lapses = lapses_for(&card, &items, &f);
                    (f, lapses)
                })
                .collect();
            if !flags.is_empty() {
                found.insert(card.id, (Some(card), flags));
            }
        }

        // Near-duplicate fronts: the same words.
        let fronts: Vec<(i64, i64, String)> = self.fronts(access).await?;
        let mut by_text: HashMap<&str, Vec<i64>> = HashMap::new();
        for (id, _, f) in &fronts {
            if !f.is_empty() {
                by_text.entry(f.as_str()).or_default().push(*id);
            }
        }
        let deck_of: HashMap<i64, i64> = fronts.iter().map(|(id, deck, _)| (*id, *deck)).collect();
        for ids in by_text.values().filter(|ids| ids.len() > 1) {
            for &id in ids.iter().filter(|id| in_scope(deck_of[id])) {
                let other = ids.iter().copied().find(|o| *o != id).unwrap_or(id);
                found.entry(id).or_insert((None, vec![])).1.push((duplicate_flag(other), 0));
            }
        }

        let all_ids: Vec<i64> = found.keys().copied().collect();
        let dismissed = self.dismissals(&all_ids).await?;
        let mut out = Vec::new();
        for (id, (card, flags)) in found {
            let flags: Vec<Flag> = flags
                .into_iter()
                .filter(|(f, lapses)| back_again(f.kind, dismissed.get(&(id, f.kind)).copied(), *lapses))
                .map(|(f, _)| f)
                .collect();
            if flags.is_empty() {
                continue;
            }
            let card = match card {
                Some(c) => c,
                None => match self.card(access, id).await {
                    Ok(c) if !c.suspended => c,
                    _ => continue,
                },
            };
            out.push(Attention { card, flags });
        }
        out.sort_by_key(|a| {
            (
                !a.flags.iter().any(|f| f.kind == Kind::Leech),
                std::cmp::Reverse(a.card.lapses),
                std::cmp::Reverse(a.card.id),
            )
        });
        Ok(out)
    }

    /// "Keep as is": hide the flag until the card slips a few more times
    /// (a duplicate stays hidden).
    pub async fn dismiss_flag(&self, access: &Access, card_id: i64, kind: Kind) -> Result<()> {
        access.require_write()?;
        let card = self.card(access, card_id).await?;
        let items = self.card_items(&card).await?;
        let lapses = if card.multi.is_some() {
            items.iter().map(|i| i.lapses).max().unwrap_or(0)
        } else {
            card.lapses
        };
        sqlx::query(
            "INSERT INTO coach_dismissals (card_id, kind, lapses, created_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (card_id, kind) DO UPDATE SET lapses = excluded.lapses, created_at = excluded.created_at",
        )
        .bind(card.id)
        .bind(kind.as_str())
        .bind(lapses)
        .bind(now())
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Replace a card by one card per item of its back: the same front, each
    /// item as a back, fresh progress. The original is deleted, or kept and
    /// suspended. Returns the new cards.
    pub async fn split_card(
        &self,
        access: &Access,
        card_id: i64,
        cards: Vec<(String, String)>,
        keep_original: bool,
    ) -> Result<Vec<Card>> {
        access.require_write()?;
        let card = self.card(access, card_id).await?;
        let cards: Vec<(String, String)> = cards
            .into_iter()
            .filter(|(f, b)| !f.trim().is_empty() || !b.trim().is_empty())
            .collect();
        if cards.len() < 2 {
            return Err(AppError::bad("a split needs at least two cards"));
        }
        if cards.iter().any(|(f, _)| f.trim().is_empty()) {
            return Err(AppError::bad("every new card needs a front"));
        }
        let mut out = Vec::new();
        for (front, back) in &cards {
            out.push(self.create_card(access, card.deck_id, front, back, None, None, None).await?);
        }
        if keep_original {
            self.set_suspended(access, card.id, true).await?;
        } else {
            self.delete_card(access, card.id).await?;
        }
        Ok(out)
    }
}

fn duplicate_flag(other: i64) -> Flag {
    Flag {
        kind: Kind::Duplicate,
        reason: "Another card has (almost) the same front. Cards this alike get mixed up; learn one well first, or make the fronts clearly different.".into(),
        item: None,
        other_card: Some(other),
    }
}

/// The lapse count a flag is judged by: the flagged item's, or the card's.
fn lapses_for(card: &Card, items: &[multi::CardItem], flag: &Flag) -> i64 {
    match flag.item.and_then(|n| items.get(n)) {
        Some(i) => i.lapses,
        None => card.lapses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fronts() {
        assert_eq!(normalize("**What** is  the capital of France?"), "what is the capital of france");
        assert_eq!(normalize("Hund"), normalize("hund!"));
    }

    #[test]
    fn near_duplicates() {
        let a = normalize("What is the capital of France?");
        let b = normalize("What's the capital of France?");
        assert!(near_duplicate(&a, &b));
        assert!(!near_duplicate(&a, &normalize("What is the capital of Spain?")));
        // Short fronts only when equal.
        assert!(near_duplicate("hund", "hund"));
        assert!(!near_duplicate("hund", "hunde"));
    }

    #[test]
    fn leeches_need_lapses_and_a_short_memory() {
        assert!(is_leech(5, Some(10.0), CardState::Review));
        assert!(is_leech(6, None, CardState::Relearning));
        assert!(!is_leech(4, Some(10.0), CardState::Review));
        assert!(!is_leech(8, Some(400.0), CardState::Review));
        assert!(!is_leech(8, Some(1.0), CardState::New));
    }

    #[test]
    fn kept_flags_come_back_after_more_lapses() {
        assert!(back_again(Kind::Leech, None, 5));
        assert!(!back_again(Kind::Leech, Some(5), 7));
        assert!(back_again(Kind::Leech, Some(5), 8));
        assert!(!back_again(Kind::Duplicate, Some(0), 99));
    }

    #[test]
    fn hints_for_long_lists() {
        assert!(write_hints("- a\n- b", None).is_empty());
        assert_eq!(write_hints("- a\n- b\n- c", None).len(), 1);
        assert!(write_hints("- a\n- b\n- c", Some(Split::List)).is_empty());
    }
}
