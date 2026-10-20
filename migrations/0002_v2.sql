-- Deck sharing: the recipient gets a copy when they accept.
CREATE TABLE deck_shares (
    id          INTEGER PRIMARY KEY,
    deck_id     INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    from_user   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_user     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    UNIQUE (deck_id, to_user)
);
CREATE INDEX deck_shares_to ON deck_shares (to_user);

-- Offline answers carry a client-generated id so a retried sync is a no-op.
ALTER TABLE review_log ADD COLUMN client_id TEXT;
CREATE UNIQUE INDEX review_log_client_id ON review_log (client_id) WHERE client_id IS NOT NULL;

-- OAuth 2.1 for MCP clients. An approved connection is an api_tokens row
-- (so it shows up and can be revoked next to manual tokens); the short-lived
-- access tokens and rotating refresh tokens point at it.
ALTER TABLE api_tokens ADD COLUMN oauth_client_id TEXT;

CREATE TABLE oauth_clients (
    id                  TEXT    PRIMARY KEY,
    secret_hash         TEXT,
    name                TEXT    NOT NULL,
    redirect_uris       TEXT    NOT NULL,
    -- set for clients identified by a metadata document URL
    metadata_url        TEXT,
    created_at          INTEGER NOT NULL
);

CREATE TABLE oauth_codes (
    code_hash       TEXT    PRIMARY KEY,
    client_id       TEXT    NOT NULL,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    redirect_uri    TEXT    NOT NULL,
    code_challenge  TEXT    NOT NULL,
    can_write       INTEGER NOT NULL,
    can_review      INTEGER NOT NULL,
    -- JSON array of deck ids, or NULL for all decks
    decks           TEXT,
    expires_at      INTEGER NOT NULL
);

CREATE TABLE oauth_tokens (
    token_hash      TEXT    PRIMARY KEY,
    -- 'access' or 'refresh'
    kind            TEXT    NOT NULL,
    api_token_id    INTEGER NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    expires_at      INTEGER NOT NULL
);
CREATE INDEX oauth_tokens_grant ON oauth_tokens (api_token_id);
