//! Fuzzy matching for the quick find.
//!
//! A query is split into words; every word must match the text, and the
//! scores add up. A word matches when its letters appear in the text in
//! order ("spv" in "Spanish verbs"), scored like fzf: letters at the start
//! of a word and runs of letters in a row count most, gaps cost a little.
//! A word of four letters or more that does not match that way may still
//! match a word of the text with a typo or two ("neuroantomy" finds
//! "Neuroanatomy"), for a lower score. Case and common accents are ignored.

/// Letters looked at per text. Longer texts are cut; the start is what counts.
const MAX_TEXT: usize = 400;
const MAX_WORD: usize = 48;

const MATCH: i32 = 16;
const START: i32 = 12; // first letter of the text
const BOUNDARY: i32 = 10; // first letter of a word
const CAMEL: i32 = 6; // an upper-case letter after a lower-case one
const RUN: i32 = 8; // each letter right after the previous match
const GAP_OPEN: i32 = 3;
const GAP: i32 = 1;
const GAP_MAX: i32 = 12;

/// The score of `text` for `query`, higher is better, or None when some
/// word of the query does not match.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let words: Vec<Vec<char>> = query.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
    if words.is_empty() {
        return None;
    }
    let raw: Vec<char> = text.chars().take(MAX_TEXT).collect();
    let folded: Vec<char> = raw.iter().map(|&c| fold_char(c)).collect();
    let mut total = 0;
    for w in &words {
        let w = &w[..w.len().min(MAX_WORD)];
        total += in_order(w, &raw, &folded).or_else(|| with_typos(w, &folded))?;
    }
    Some(total)
}

/// Lower case without accents, one char for one char.
fn fold(s: &str) -> Vec<char> {
    s.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ğ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
        'ł' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ř' => 'r',
        'ś' | 'š' | 'ş' | 'ß' => 's',
        'ť' | 'ţ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => c,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// What a match at position i is worth beyond MATCH.
fn position_bonus(raw: &[char], i: usize) -> i32 {
    if i == 0 {
        return START;
    }
    let (prev, here) = (raw[i - 1], raw[i]);
    if !is_word_char(prev) && is_word_char(here) {
        BOUNDARY
    } else if prev.is_lowercase() && here.is_uppercase() {
        CAMEL
    } else {
        0
    }
}

/// Best in-order match of `q` in the text, by dynamic programming over
/// (query letter, text position). None when the letters are not all there.
fn in_order(q: &[char], raw: &[char], t: &[char]) -> Option<i32> {
    let (m, n) = (q.len(), t.len());
    if m > n {
        return None;
    }
    // Cheap check first: most texts don't hold the letters at all.
    let mut k = 0;
    for &c in t {
        if k < m && c == q[k] {
            k += 1;
        }
    }
    if k < m {
        return None;
    }
    const NONE: i32 = i32::MIN / 2;
    // best[j]: best score with the current query letter matched at j.
    let mut prev = vec![NONE; n];
    for j in 0..n {
        if t[j] == q[0] {
            // Letters skipped before the first match cost a little too.
            let lead = if j == 0 { 0 } else { (GAP_OPEN + (j as i32 - 1) * GAP).min(GAP_MAX) };
            prev[j] = MATCH + position_bonus(raw, j) - lead;
        }
    }
    for qi in 1..m {
        let mut cur = vec![NONE; n];
        // Best earlier matches at least one letter back, with what the gap
        // costs: growing by GAP per letter, but never more than GAP_MAX.
        let (mut decay, mut capped) = (NONE, NONE);
        for j in 1..n {
            if j >= 2 && prev[j - 2] > NONE {
                decay = decay.max(prev[j - 2] - GAP_OPEN);
                capped = capped.max(prev[j - 2] - GAP_MAX);
            }
            if t[j] == q[qi] {
                let mut best = decay.max(capped);
                if best > NONE {
                    best += MATCH + position_bonus(raw, j);
                }
                if prev[j - 1] > NONE {
                    best = best.max(prev[j - 1] + MATCH + RUN + position_bonus(raw, j));
                }
                cur[j] = best;
            }
            if decay > NONE {
                decay -= GAP;
            }
        }
        prev = cur;
    }
    prev.into_iter().filter(|&s| s > NONE).max()
}

/// A word of the query against each word of the text, allowing a few
/// edits (a letter missing, extra, wrong, or two swapped). The query may
/// also be a prefix of the word, with the same allowance.
fn with_typos(q: &[char], t: &[char]) -> Option<i32> {
    let allowed = match q.len() {
        0..=3 => return None,
        4..=7 => 1,
        _ => 2,
    };
    let mut best: Option<i32> = None;
    let mut i = 0;
    while i < t.len() {
        if !is_word_char(t[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < t.len() && is_word_char(t[i]) {
            i += 1;
        }
        let word = &t[start..i.min(start + MAX_WORD)];
        if let Some(edits) = prefix_distance(q, word, allowed) {
            let s = q.len() as i32 * MATCH / 2 - edits as i32 * 12 + if start == 0 { START } else { BOUNDARY };
            best = Some(best.map_or(s, |b| b.max(s)));
        }
    }
    best
}

/// The fewest edits that turn `q` into some prefix of `w` (optimal string
/// alignment: insert, delete, replace, swap two neighbours), if at most
/// `allowed`.
fn prefix_distance(q: &[char], w: &[char], allowed: usize) -> Option<usize> {
    let (m, n) = (q.len(), w.len());
    if n + allowed < m {
        return None;
    }
    let width = n + 1;
    let mut d = vec![0usize; (m + 1) * width];
    for j in 0..=n {
        d[j] = j;
    }
    for i in 1..=m {
        d[i * width] = i;
        for j in 1..=n {
            let cost = usize::from(q[i - 1] != w[j - 1]);
            let mut v = (d[(i - 1) * width + j] + 1)
                .min(d[i * width + j - 1] + 1)
                .min(d[(i - 1) * width + j - 1] + cost);
            if i > 1 && j > 1 && q[i - 1] == w[j - 2] && q[i - 2] == w[j - 1] {
                v = v.min(d[(i - 2) * width + j - 2] + 1);
            }
            d[i * width + j] = v;
        }
    }
    // Any prefix of the word will do, but not one much shorter than the query.
    let from = m.saturating_sub(allowed);
    (from..=n).map(|j| d[m * width + j]).min().filter(|&e| e <= allowed)
}

#[cfg(test)]
mod tests {
    use super::score;

    fn rank<'a>(q: &str, texts: &[&'a str]) -> Vec<&'a str> {
        let mut scored: Vec<(i32, &str)> = texts.iter().filter_map(|t| score(q, t).map(|s| (s, *t))).collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, t)| t).collect()
    }

    #[test]
    fn letters_in_order_match() {
        assert!(score("spv", "Spanish verbs").is_some());
        assert!(score("nrtmy", "Neuroanatomy").is_some());
        assert!(score("xyz", "Spanish verbs").is_none());
        assert!(score("vs", "Spanish").is_none());
    }

    #[test]
    fn word_starts_and_runs_win() {
        let r = rank("sv", &["Escape velocity", "Spanish verbs"]);
        assert_eq!(r[0], "Spanish verbs");
        let r = rank("set", &["Spacing effect", "Settings"]);
        assert_eq!(r[0], "Settings");
        let r = rank("rust", &["Trust your memory", "Rust lifetimes"]);
        assert_eq!(r[0], "Rust lifetimes");
    }

    #[test]
    fn every_word_of_the_query_must_match() {
        assert!(score("span verb", "Spanish verbs").is_some());
        assert!(score("span noun", "Spanish verbs").is_none());
    }

    #[test]
    fn typos_are_forgiven_in_longer_words() {
        assert!(score("neuroantomy", "Neuroanatomy").is_some());
        assert!(score("setings", "Settings").is_some());
        assert!(score("lifeitmes", "Rust lifetimes").is_some());
        assert!(score("spnaish", "Spanish verbs").is_some());
        // Short words need their letters in order.
        assert!(score("vbr", "Spanish verbs").is_none());
        assert!(score("zzzzzz", "Spanish verbs").is_none());
    }

    #[test]
    fn an_exact_match_beats_a_typo() {
        let r = rank("lesson", &["Lessen the load", "Lesson one"]);
        assert_eq!(r[0], "Lesson one");
    }

    #[test]
    fn case_and_accents_do_not_matter() {
        assert!(score("espanol", "Español").is_some());
        assert!(score("UBER", "über alles").is_some());
        assert!(score("überall", "Uberall").is_some());
    }

    #[test]
    fn empty_queries_match_nothing() {
        assert!(score("", "anything").is_none());
        assert!(score("   ", "anything").is_none());
    }
}
