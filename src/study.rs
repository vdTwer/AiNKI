//! How a study session runs, beyond picking cards: quiet hours, breaks,
//! catch-up mode after time away, and the "sure or not?" check.
//!
//! Timed questions and typed answers are set per preset and handled on
//! the study page; a timeout is recorded as Again (see `note_answer`).

use chrono::{Duration, NaiveTime, TimeZone, Timelike};
use serde::Serialize;

use crate::auth::{Access, User};
use crate::error::{AppError, Result};
use crate::App;
use crate::now;

/// A gap this long between answers ends a session.
pub const SESSION_GAP_SECS: i64 = 10 * 60;
/// Session lengths offered in settings, in minutes; 0 is off.
pub const SESSION_LENGTHS: &[i64] = &[0, 10, 15, 20, 25, 30, 45, 60];
/// Catch-up mode suggests itself above this many overdue cards.
pub const BACKLOG_HINT: i64 = 100;
/// A card counts as overdue once it has been due this long.
const OVERDUE_SECS: i64 = 24 * 3600;

/// Is `hour` inside quiet hours from `from` to `to` (to not included)?
/// The window may cross midnight; equal ends mean no quiet hours.
pub fn in_window(from: i64, to: i64, hour: i64) -> bool {
    if from == to {
        false
    } else if from < to {
        (from..to).contains(&hour)
    } else {
        hour >= from || hour < to
    }
}

/// When the quiet hours going on at `at` end, or `None` when it is not
/// quiet then (or "Study anyway" lifted them).
pub fn quiet_until(user: &User, at: i64) -> Option<i64> {
    let (from, to) = (user.quiet_from?, user.quiet_to?);
    if user.quiet_lifted_until.is_some_and(|t| t > at) {
        return None;
    }
    let tz = user.tz();
    let local = tz.timestamp_opt(at, 0).single()?;
    if !in_window(from, to, local.hour() as i64) {
        return None;
    }
    let end_time = NaiveTime::from_hms_opt(to.clamp(0, 23) as u32, 0, 0)?;
    let mut day = local.date_naive();
    loop {
        // A local time that doesn't exist (DST) is taken an hour later.
        let end = day.and_time(end_time);
        let t = tz
            .from_local_datetime(&end)
            .earliest()
            .or_else(|| tz.from_local_datetime(&(end + Duration::hours(1))).earliest())?
            .timestamp();
        if t > at {
            return Some(t);
        }
        day = day.succ_opt()?;
    }
}

/// How a minute count reads in a break hint.
pub fn minutes_words(m: i64) -> String {
    if m == 1 { "1 minute".into() } else { format!("{m} minutes") }
}

#[derive(Debug, Default, Serialize)]
pub struct Confidence {
    pub sure: i64,
    pub sure_recalled: i64,
    pub unsure: i64,
    pub unsure_recalled: i64,
}

impl Confidence {
    pub fn sure_pct(&self) -> Option<f64> {
        (self.sure > 0).then(|| self.sure_recalled as f64 * 100.0 / self.sure as f64)
    }

    pub fn unsure_pct(&self) -> Option<f64> {
        (self.unsure > 0).then(|| self.unsure_recalled as f64 * 100.0 / self.unsure as f64)
    }
}

impl App {
    /// After an answer on the website: whether the user felt sure, and
    /// whether time ran out. Goes onto the review just logged.
    pub async fn note_answer(&self, access: &Access, card_id: i64, sure: Option<bool>, timed_out: bool) -> Result<()> {
        if sure.is_none() && !timed_out {
            return Ok(());
        }
        sqlx::query(
            "UPDATE review_log SET confidence = ?, timed_out = ?
             WHERE id = (SELECT MAX(id) FROM review_log WHERE user_id = ? AND card_id = ?)",
        )
        .bind(sure)
        .bind(timed_out)
        .bind(access.user_id())
        .bind(card_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// "Study anyway": quiet hours are off until they end this time.
    pub async fn lift_quiet(&self, user: &User) -> Result<()> {
        let until = quiet_until(user, now()).unwrap_or_else(now);
        sqlx::query("UPDATE users SET quiet_lifted_until = ? WHERE id = ?")
            .bind(until)
            .bind(user.id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// When the session going on now began: the first answer on the website
    /// after the last pause of `SESSION_GAP_SECS`, or the last time a break
    /// hint was waved off. `None` when nothing was answered lately.
    pub async fn session_start(&self, access: &Access) -> Result<Option<i64>> {
        let n = now();
        let times: Vec<i64> = sqlx::query_scalar(
            "SELECT reviewed_at FROM review_log
             WHERE user_id = ? AND source = 'web' AND reviewed_at >= ? AND rating > 0
             ORDER BY reviewed_at DESC",
        )
        .bind(access.user_id())
        .bind(n - 6 * 3600)
        .fetch_all(&self.db)
        .await?;
        let Some(&last) = times.first() else { return Ok(None) };
        if n - last >= SESSION_GAP_SECS {
            return Ok(None);
        }
        let mut start = last;
        for &t in &times[1..] {
            if start - t >= SESSION_GAP_SECS {
                break;
            }
            start = t;
        }
        Ok(Some(match access.user.break_skipped_at {
            Some(b) if b > start => b,
            _ => start,
        }))
    }

    /// Minutes studied without a break, when that has reached the user's
    /// session length.
    pub async fn break_due(&self, access: &Access) -> Result<Option<i64>> {
        let len = access.user.session_minutes;
        if len <= 0 {
            return Ok(None);
        }
        let Some(start) = self.session_start(access).await? else { return Ok(None) };
        let minutes = (now() - start) / 60;
        Ok((minutes >= len).then_some(minutes))
    }

    pub async fn skip_break(&self, user_id: i64) -> Result<()> {
        sqlx::query("UPDATE users SET break_skipped_at = ? WHERE id = ?")
            .bind(now())
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Review cards due for more than a day: what piled up while away.
    pub async fn overdue(&self, access: &Access) -> Result<i64> {
        let ids: Vec<i64> = self.decks(access).await?.into_iter().map(|d| d.id).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let sql = format!(
            "SELECT COUNT(*) FROM cards WHERE user_id = ? AND suspended = 0 AND state = 2 AND due <= ?
             AND deck_id IN ({})",
            crate::service::placeholders(ids.len())
        );
        let mut q = sqlx::query_scalar(&sql).bind(access.user_id()).bind(now() - OVERDUE_SECS);
        for id in ids {
            q = q.bind(id);
        }
        Ok(q.fetch_one(&self.db).await?)
    }

    /// How often you remembered when you felt sure, and when you didn't,
    /// over the last year.
    pub async fn confidence(&self, access: &Access, deck_id: Option<i64>) -> Result<Confidence> {
        #[derive(sqlx::FromRow)]
        struct Row {
            confidence: i64,
            n: i64,
            recalled: i64,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT confidence, COUNT(*) AS n, SUM(rating > 1) AS recalled FROM review_log
             WHERE user_id = ? AND confidence IS NOT NULL AND reviewed_at >= ?
               AND (? IS NULL OR deck_id = ?)
             GROUP BY confidence",
        )
        .bind(access.user_id())
        .bind(now() - 365 * 86400)
        .bind(deck_id)
        .bind(deck_id)
        .fetch_all(&self.db)
        .await?;
        let mut c = Confidence::default();
        for r in rows {
            if r.confidence == 1 {
                (c.sure, c.sure_recalled) = (r.n, r.recalled);
            } else {
                (c.unsure, c.unsure_recalled) = (r.n, r.recalled);
            }
        }
        Ok(c)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn update_study_settings(
        &self,
        user_id: i64,
        quiet: Option<(i64, i64)>,
        session_minutes: i64,
        catch_up: bool,
        catch_up_reviews: i64,
        confidence_check: bool,
    ) -> Result<()> {
        let quiet = quiet.filter(|(f, t)| f != t);
        if let Some((f, t)) = quiet {
            if !(0..24).contains(&f) || !(0..24).contains(&t) {
                return Err(AppError::bad("quiet hours must be whole hours from 0 to 23"));
            }
        }
        if !(0..=240).contains(&session_minutes) {
            return Err(AppError::bad("session length must be between 0 (off) and 240 minutes"));
        }
        if !(1..=9999).contains(&catch_up_reviews) {
            return Err(AppError::bad("catch-up reviews must be between 1 and 9999 a day"));
        }
        sqlx::query(
            "UPDATE users SET quiet_from = ?, quiet_to = ?, quiet_lifted_until = NULL, session_minutes = ?,
                catch_up = ?, catch_up_reviews = ?, confidence_check = ? WHERE id = ?",
        )
        .bind(quiet.map(|q| q.0))
        .bind(quiet.map(|q| q.1))
        .bind(session_minutes)
        .bind(catch_up)
        .bind(catch_up_reviews)
        .bind(confidence_check)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn set_catch_up(&self, user_id: i64, on: bool) -> Result<()> {
        sqlx::query("UPDATE users SET catch_up = ? WHERE id = ?")
            .bind(on)
            .bind(user_id)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_windows_cross_midnight() {
        assert!(in_window(22, 7, 23));
        assert!(in_window(22, 7, 0));
        assert!(in_window(22, 7, 6));
        assert!(!in_window(22, 7, 7));
        assert!(!in_window(22, 7, 12));
        assert!(in_window(13, 15, 14));
        assert!(!in_window(13, 15, 15));
        assert!(!in_window(9, 9, 9));
    }
}
