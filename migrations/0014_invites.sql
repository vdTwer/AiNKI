-- Invitations. Each account can make one invite link a week, from a week
-- after it joined; the link works once, for 30 days. Who invited whom is
-- kept for the admin's invitation graph.
ALTER TABLE users ADD COLUMN invited_by INTEGER REFERENCES users(id) ON DELETE SET NULL;
-- The admin account sees only the admin panel and invites without a limit.
ALTER TABLE users ADD COLUMN is_admin INTEGER NOT NULL DEFAULT 0;

CREATE TABLE invites (
    id          INTEGER PRIMARY KEY,
    -- The secret part of the link. Kept as is, so the inviter can copy
    -- the link again until someone uses it.
    token       TEXT    NOT NULL UNIQUE,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER,
    used_by     INTEGER REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX invites_user ON invites (user_id, created_at);
