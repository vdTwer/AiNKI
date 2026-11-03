-- "Not now" on a note from the coach: that rule stays quiet until `until`.
CREATE TABLE coach_snoozes (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rule    TEXT    NOT NULL,
    until   INTEGER NOT NULL,
    PRIMARY KEY (user_id, rule)
);
