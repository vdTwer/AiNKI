-- Presets remember the template they came from (the goal picker for new
-- decks uses it) and can have FSRS parameters of their own.
ALTER TABLE presets ADD COLUMN template TEXT;
UPDATE presets SET template = CASE name
    WHEN 'Standard' THEN 'standard'
    WHEN 'Exam prep' THEN 'exam'
    WHEN 'Language vocabulary' THEN 'language'
    WHEN 'Relaxed' THEN 'relaxed'
    WHEN 'Maintenance' THEN 'maintenance'
END;
ALTER TABLE presets ADD COLUMN fsrs_params TEXT NOT NULL DEFAULT '[]';
ALTER TABLE presets ADD COLUMN fsrs_params_updated_at INTEGER;

-- Monthly re-optimization: on unless turned off; when it last ran, how
-- many reviews there were then, and what it did, in a sentence.
ALTER TABLE users ADD COLUMN auto_optimize INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN optimized_at INTEGER;
ALTER TABLE users ADD COLUMN optimized_reviews INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN optimize_note TEXT;
UPDATE users SET optimized_at = fsrs_params_updated_at;
