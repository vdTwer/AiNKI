-- Smart breaks learn a rhythm per user: the work stretch in minutes on
-- cards, and how much longer or shorter than plain the breaks should be.
-- Keep going lengthens the stretch; pausing on one's own shortens it.
ALTER TABLE users ADD COLUMN break_work_min REAL NOT NULL DEFAULT 25;
ALTER TABLE users ADD COLUMN break_mult REAL NOT NULL DEFAULT 1;
-- When a break was last suggested, to tell own pauses from suggested ones
-- and a Keep going from a break taken.
ALTER TABLE users ADD COLUMN break_shown_at INTEGER;
