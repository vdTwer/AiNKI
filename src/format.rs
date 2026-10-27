//! How numbers, intervals and dates are written. One place, so every page
//! (and the MCP answers) says the same thing the same way.

use chrono::Datelike;

use crate::scheduler::Clock;

/// "1 card", "3 cards". English plurals by adding "s"; every word we
/// count with takes that.
pub fn count(n: i64, word: &str) -> String {
    format!("{} {}", number(n), plural(n, word))
}

/// A whole number with thousands separators: "20,000".
pub fn number(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The word alone, singular or plural for `n`.
pub fn plural(n: i64, word: &str) -> String {
    if n == 1 { word.to_string() } else { format!("{word}s") }
}

/// Shown where a number has no value yet.
pub const NONE: &str = "—";

/// A share as a whole percent, "91%"; the dash when there's none.
pub fn percent(v: Option<f64>) -> String {
    v.map(|v| format!("{:.0}%", v * 100.0)).unwrap_or_else(|| NONE.into())
}

/// Whole minutes, rounded: "about 3 minutes"; "under a minute" below one.
pub fn minutes(m: f64) -> String {
    let n = m.round() as i64;
    if n < 1 { "under a minute".into() } else { format!("about {}", count(n, "minute")) }
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

/// A calendar day in words: "today", "tomorrow", "yesterday", or
/// "Friday, 18 December" (with the year when it isn't this year).
pub fn day(clock: &Clock, date: chrono::NaiveDate) -> String {
    match (date - clock.today()).num_days() {
        0 => "today".into(),
        1 => "tomorrow".into(),
        -1 => "yesterday".into(),
        _ if date.year() == clock.today().year() => date.format("%A, %-d %B").to_string(),
        _ => date.format("%A, %-d %B %Y").to_string(),
    }
}

/// A short day label for charts: "Today", "Tue 22".
pub fn short_day(clock: &Clock, date: chrono::NaiveDate) -> String {
    match (date - clock.today()).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        _ => date.format("%a %-d").to_string(),
    }
}

/// When something happens or happened, in words: "today at 14:30",
/// "tomorrow at 9:00", "yesterday at 8:15", "on Friday, 24 October".
/// With a time named, "today" is the calendar day, not the study day: at
/// 23:00, 0:30 is "tomorrow" even if the new study day starts at 4:00.
pub fn when(clock: &Clock, ts: i64) -> String {
    let Some(local) = chrono::DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&clock.tz)) else {
        return String::new();
    };
    let Some(now) = chrono::DateTime::from_timestamp(clock.now, 0).map(|t| t.with_timezone(&clock.tz)) else {
        return String::new();
    };
    let time = local.format("%-H:%M");
    match (local.date_naive() - now.date_naive()).num_days() {
        0 => format!("today at {time}"),
        1 => format!("tomorrow at {time}"),
        -1 => format!("yesterday at {time}"),
        _ => format!("on {}", day(clock, clock.date_of(ts))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_and_intervals() {
        assert_eq!(count(1, "card"), "1 card");
        assert_eq!(count(0, "card"), "0 cards");
        assert_eq!(count(20_000, "card"), "20,000 cards");
        assert_eq!(number(999), "999");
        assert_eq!(number(1_000), "1,000");
        assert_eq!(number(-1_234_567), "-1,234,567");
        assert_eq!(interval(30), "<1m");
        assert_eq!(interval(600), "10m");
        assert_eq!(interval(3 * 86_400), "3d");
        assert_eq!(days(3), "3d");
        assert_eq!(days(29), interval(29 * 86_400));
        assert_eq!(days(90), "3.0mo");
    }

    #[test]
    fn when_uses_the_calendar_day() {
        // 23:00 UTC with the study day starting at 4:00: 0:30 is tomorrow.
        let now = 1_761_606_000; // 2025-10-27 23:00 UTC
        let clock = Clock::new(now, chrono_tz::UTC, 4);
        assert_eq!(when(&clock, now + 5_400), "tomorrow at 0:30");
        assert_eq!(when(&clock, now + 1_800), "today at 23:30");
    }
}
