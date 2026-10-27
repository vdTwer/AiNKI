-- Timed questions and typed answers are set per preset. A time limit of
-- 0 means none.
ALTER TABLE presets ADD COLUMN time_limit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE presets ADD COLUMN typed_answers INTEGER NOT NULL DEFAULT 0;

-- Quiet hours (whole hours in the user's timezone, off when NULL), and
-- until when "Study anyway" lifts them.
ALTER TABLE users ADD COLUMN quiet_from INTEGER;
ALTER TABLE users ADD COLUMN quiet_to INTEGER;
ALTER TABLE users ADD COLUMN quiet_lifted_until INTEGER;
-- Sessions by minutes (0: off), and when the last break hint was waved off.
ALTER TABLE users ADD COLUMN session_minutes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN break_skipped_at INTEGER;
-- Catch-up mode after time away: no new cards, a cap on reviews, the
-- cards most likely still remembered first.
ALTER TABLE users ADD COLUMN catch_up INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN catch_up_reviews INTEGER NOT NULL DEFAULT 100;
-- "Sure or not?" before the answer is shown.
ALTER TABLE users ADD COLUMN confidence_check INTEGER NOT NULL DEFAULT 0;

-- 1 sure, 0 unsure, NULL not asked; and whether time ran out.
ALTER TABLE review_log ADD COLUMN confidence INTEGER;
ALTER TABLE review_log ADD COLUMN timed_out INTEGER NOT NULL DEFAULT 0;
