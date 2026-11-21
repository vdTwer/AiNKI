//! How a study session runs, beyond picking cards: quiet hours, breaks,
//! catch-up mode after time away, and the "sure or not?" check.
//!
//! Timed questions and typed answers are set per preset and handled on
//! the study page; a timeout is recorded as Again (see `note_answer`).

use chrono::{Duration, NaiveTime, TimeZone, Timelike};
use serde::Serialize;

use crate::App;
use crate::auth::{Access, User};
use crate::breaks;
use crate::error::{AppError, Result};
use crate::now;
use crate::num;

/// A gap this long between answers ends a session.
pub const SESSION_GAP_SECS: i64 = 10 * 60;
/// Session lengths offered in settings, in minutes; 0 is off.
pub const SESSION_LENGTHS: &[i64] = &[0, 10, 15, 20, 25, 30, 45, 60];
/// The session length that means "when I need one": smart breaks.
pub const SMART_BREAKS: i64 = -1;
/// A break on the fixed timer lasts this long.
const TIMER_BREAK_MINUTES: i64 = 5;
/// How far back smart breaks look to learn the user's recovery.
const RECOVERY_DAYS: i64 = 60;

/// A break is due: after how many minutes of study, how long it should be,
/// and whether smart breaks asked for it (then whether it is a long one and
/// whether the answers were slowing down) or the timer did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakDue {
    pub studied: i64,
    pub length: i64,
    pub smart: bool,
    pub long: bool,
    pub slowing: bool,
}
/// Catch-up mode suggests itself above this many overdue cards.
pub const BACKLOG_HINT: i64 = 100;
/// A card counts as overdue once it has been due this long.
const OVERDUE_SECS: i64 = 24 * 3600;

/// The user's learned break rhythm.
fn rhythm(user: &User) -> breaks::Rhythm {
    breaks::Rhythm::new(user.break_work_min, user.break_mult)
}

/// Is `hour` inside quiet hours from `from` to `to` (to not included)?
/// The window may cross midnight; equal ends mean no quiet hours.
pub fn in_window(from: i64, to: i64, hour: i64) -> bool {
    match from.cmp(&to) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => (from..to).contains(&hour),
        std::cmp::Ordering::Greater => hour >= from || hour < to,
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
    if !in_window(from, to, i64::from(local.hour())) {
        return None;
    }
    let end_time = NaiveTime::from_hms_opt(num::i64_u32(to.clamp(0, 23)), 0, 0)?;
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

/// How long an answer took, as the browser measured it. Anything out of
/// range is dropped rather than stored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub think_ms: Option<i64>,
    pub answer_ms: Option<i64>,
    pub away_ms: Option<i64>,
    pub paused_ms: Option<i64>,
    pub input: Option<&'static str>,
}

/// Longer than this is no answer time but a page left open.
const TIMING_MAX_MS: i64 = 24 * 3600 * 1000;

impl Timing {
    pub fn checked(
        think_ms: Option<i64>,
        answer_ms: Option<i64>,
        away_ms: Option<i64>,
        paused_ms: Option<i64>,
        input: Option<&str>,
    ) -> Self {
        let ms = |v: Option<i64>| v.filter(|v| (0..=TIMING_MAX_MS).contains(v));
        Self {
            think_ms: ms(think_ms),
            answer_ms: ms(answer_ms),
            away_ms: ms(away_ms),
            paused_ms: ms(paused_ms),
            input: ["key", "mouse", "touch", "pen"].into_iter().find(|k| input == Some(*k)),
        }
    }
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
        (self.sure > 0).then(|| num::i64_f64(self.sure_recalled) * 100.0 / num::i64_f64(self.sure))
    }

    pub fn unsure_pct(&self) -> Option<f64> {
        (self.unsure > 0).then(|| num::i64_f64(self.unsure_recalled) * 100.0 / num::i64_f64(self.unsure))
    }
}

impl App {
    /// After an answer on the website: whether the user felt sure, whether
    /// time ran out, and how long it took. Goes onto the review just logged.
    pub async fn note_answer(
        &self,
        access: &Access,
        card_id: i64,
        sure: Option<bool>,
        timed_out: bool,
        timing: &Timing,
    ) -> Result<()> {
        if sure.is_none() && !timed_out && *timing == Timing::default() {
            return Ok(());
        }
        sqlx::query(
            "UPDATE review_log SET confidence = ?, timed_out = ?, think_ms = ?, answer_ms = ?, away_ms = ?, paused_ms = ?, input = ?
             WHERE id = (SELECT MAX(id) FROM review_log WHERE user_id = ? AND card_id = ?)",
        )
        .bind(sure)
        .bind(timed_out)
        .bind(timing.think_ms)
        .bind(timing.answer_ms)
        .bind(timing.away_ms)
        .bind(timing.paused_ms)
        .bind(timing.input)
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
        Ok(self.session_began(access).await?.map(|start| match access.user.break_skipped_at {
            Some(b) if b > start => b,
            _ => start,
        }))
    }

    /// The first answer after the last pause of `SESSION_GAP_SECS`.
    async fn session_began(&self, access: &Access) -> Result<Option<i64>> {
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
        for &t in times.iter().skip(1) {
            if start - t >= SESSION_GAP_SECS {
                break;
            }
            start = t;
        }
        Ok(Some(start))
    }

    /// Whether a break is due before the next card: on the fixed timer once
    /// the session has run the user's length, or with smart breaks once the
    /// answers show tiredness (see `breaks`).
    pub async fn break_due(&self, access: &Access) -> Result<Option<BreakDue>> {
        let len = access.user.session_minutes;
        if len == SMART_BREAKS {
            return self.smart_break_due(access).await;
        }
        if len <= 0 {
            return Ok(None);
        }
        let Some(start) = self.session_start(access).await? else { return Ok(None) };
        let minutes = (now() - start) / 60;
        Ok((minutes >= len).then_some(BreakDue {
            studied: minutes,
            length: TIMER_BREAK_MINUTES,
            smart: false,
            long: false,
            slowing: false,
        }))
    }

    async fn smart_break_due(&self, access: &Access) -> Result<Option<BreakDue>> {
        let steps = self.break_steps(access).await?;
        // Back after a rest: that was the break.
        if steps.last().is_none_or(|s| now() - s.at >= breaks::REST_SECS) {
            return Ok(None);
        }
        let st = breaks::replay(&steps, access.user.break_skipped_at.unwrap_or(0));
        Ok(breaks::suggest(&st, rhythm(&access.user)).map(|s| BreakDue {
            studied: s.worked,
            length: s.minutes,
            smart: true,
            long: s.long,
            slowing: s.slowing,
        }))
    }

    /// Today's answers on the website (the last twelve hours), oldest first,
    /// each with its card's usual front time from the last `RECOVERY_DAYS`.
    /// Multicards and timed-out answers count as work but measure nothing:
    /// their times mean something else.
    async fn break_steps(&self, access: &Access) -> Result<Vec<breaks::Step>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            reviewed_at: i64,
            card_id: i64,
            think_ms: Option<i64>,
            answer_ms: Option<i64>,
            away_ms: Option<i64>,
            plain: bool,
        }
        let n = now();
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT reviewed_at, card_id, think_ms, answer_ms, away_ms, (items_asked IS NULL AND timed_out = 0) AS plain
             FROM review_log
             WHERE user_id = ? AND source = 'web' AND rating > 0 AND reviewed_at >= ?
             ORDER BY reviewed_at, id",
        )
        .bind(access.user_id())
        .bind(n - RECOVERY_DAYS * 86_400)
        .fetch_all(&self.db)
        .await?;
        let usuals =
            breaks::card_usuals(&rows.iter().map(|r| (r.card_id, r.think_ms.filter(|_| r.plain))).collect::<Vec<_>>());
        Ok(rows
            .iter()
            .zip(usuals)
            .filter(|(r, _)| r.reviewed_at >= n - 12 * 3600)
            .map(|(r, usual)| breaks::Step {
                at: r.reviewed_at,
                think_ms: r.think_ms,
                answer_ms: r.answer_ms,
                away_ms: r.away_ms,
                usual: usual.filter(|_| r.plain),
            })
            .collect())
    }

    /// After an answer with smart breaks on: a pause the user took on their
    /// own, before any suggestion, shortens their stretches and lengthens
    /// their breaks.
    pub async fn note_rest(&self, access: &Access) -> Result<()> {
        let user = &access.user;
        if user.session_minutes != SMART_BREAKS {
            return Ok(());
        }
        let steps = self.break_steps(access).await?;
        if breaks::paused_on_own(
            &breaks::replay(&steps, user.break_skipped_at.unwrap_or(0)),
            &steps,
            user.break_shown_at,
        ) {
            self.set_rhythm(user.id, rhythm(user).paused_early()).await?;
        }
        Ok(())
    }

    /// A break page is on screen: remember when this suggestion first came.
    pub async fn break_shown(&self, user_id: i64) -> Result<()> {
        sqlx::query(
            "UPDATE users SET break_shown_at = ?
             WHERE id = ? AND (break_shown_at IS NULL OR break_shown_at < (SELECT MAX(reviewed_at) FROM review_log WHERE user_id = ?))",
        )
        .bind(now())
        .bind(user_id)
        .bind(user_id)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    /// Keep going, or the break is over: the stretch starts over. With
    /// smart breaks, Keep going right away means the user felt fresh: longer
    /// stretches, shorter breaks. After a few minutes it was a break.
    pub async fn skip_break(&self, user: &User) -> Result<()> {
        let n = now();
        let refused =
            user.session_minutes == SMART_BREAKS && user.break_shown_at.is_some_and(|t| n - t < breaks::REST_SECS);
        if refused {
            self.set_rhythm(user.id, rhythm(user).kept_going()).await?;
        }
        sqlx::query("UPDATE users SET break_skipped_at = ? WHERE id = ?")
            .bind(n)
            .bind(user.id)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn set_rhythm(&self, user_id: i64, r: breaks::Rhythm) -> Result<()> {
        sqlx::query("UPDATE users SET break_work_min = ?, break_mult = ? WHERE id = ?")
            .bind(r.work_min)
            .bind(r.mult)
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
        let sql = "SELECT COUNT(*) FROM cards WHERE user_id = ? AND suspended = 0 AND state = 2 AND due <= ?
             AND deck_id IN (SELECT value FROM json_each(?))";
        let mut q = sqlx::query_scalar(sql).bind(access.user_id()).bind(now() - OVERDUE_SECS);
        q = q.bind(crate::service::id_list(&ids));
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
        if let Some((f, t)) = quiet
            && (!(0..24).contains(&f) || !(0..24).contains(&t))
        {
            return Err(AppError::bad("quiet hours must be whole hours from 0 to 23"));
        }
        if !(SMART_BREAKS..=240).contains(&session_minutes) {
            return Err(AppError::bad("session length must be -1 (smart breaks), 0 (off) or up to 240 minutes"));
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
        sqlx::query("UPDATE users SET catch_up = ? WHERE id = ?").bind(on).bind(user_id).execute(&self.db).await?;
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
