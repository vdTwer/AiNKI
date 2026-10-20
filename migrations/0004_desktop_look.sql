-- Look on desktop computers: 'sketch' (hand-drawn) or 'simple'. Phones,
-- e-readers and browsers without JavaScript always get the simple look.
ALTER TABLE users ADD COLUMN desktop_look TEXT NOT NULL DEFAULT 'sketch';
