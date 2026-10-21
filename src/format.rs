//! How numbers, intervals and dates are written. One place, so every page
//! (and the MCP answers) says the same thing the same way.

use crate::scheduler::Clock;

/// "1 card", "3 cards". English plurals by adding "s"; every word we
/// count with takes that.
pub fn count(n: i64, word: &str) -> String {
    format!("{n} {}", plural(n, word))
}

/// The word alone, singular or plural for `n`.
pub fn plural(n: i64, word: &str) -> String {
    if n == 1 { word.to_string() } else { format!("{word}s") }
}

/// Short label for a wait, e.g. "10m", "3d", "2.1mo".
pub fn interval(secs: i64) -> String {
    let secs = secs.max(0) as f64;
    if secs < 60.0 {
        "<1m".into()
    } else if secs < 3600.0 {
        format!("{:.0}m", secs / 60.0)
    } else if secs < 86_400.0 {
        format!("{:.0}h", secs / 3600.0)
    } else if secs < 30.0 * 86_400.0 {
        // Review cards are due at the start of a day, so round up.
        format!("{:.0}d", ((secs - 60.0) / 86_400.0).ceil())
    } else if secs < 365.0 * 86_400.0 {
        format!("{:.1}mo", secs / (30.44 * 86_400.0))
    } else {
        format!("{:.1}y", secs / (365.25 * 86_400.0))
    }
}

/// The same for a whole number of days.
pub fn days(d: i64) -> String {
    if d <= 0 { "0d".into() } else { interval(d * 86_400) }
}

/// Machine-readable local time, "2026-10-21 14:30". For MCP answers and
/// exports; pages use words.
pub fn iso(ts: i64, clock: &Clock) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.with_timezone(&clock.tz).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// "Tuesday, 20 October".
pub fn long_date(clock: &Clock) -> String {
    clock.today().format("%A, %-d %B").to_string()
}

/// When something happens, in words: "today at 14:30", "tomorrow at 9:00",
/// "on Friday, 24 October".
pub fn when(clock: &Clock, ts: i64) -> String {
    let Some(local) = chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&clock.tz)) else {
        return String::new();
    };
    let days = (clock.date_of(ts) - clock.today()).num_days();
    let time = local.format("%-H:%M");
    match days {
        ..=0 => format!("today at {time}"),
        1 => format!("tomorrow at {time}"),
        _ => format!("on {}", local.format("%A, %-d %B")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_and_intervals() {
        assert_eq!(count(1, "card"), "1 card");
        assert_eq!(count(0, "card"), "0 cards");
        assert_eq!(interval(30), "<1m");
        assert_eq!(interval(600), "10m");
        assert_eq!(interval(3 * 86_400), "3d");
        assert_eq!(days(3), "3d");
        assert_eq!(days(29), interval(29 * 86_400));
        assert_eq!(days(90), "3.0mo");
    }
}
