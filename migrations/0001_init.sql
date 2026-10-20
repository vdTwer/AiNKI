CREATE TABLE users (
    id                      INTEGER PRIMARY KEY,
    username                TEXT    NOT NULL,
    discriminator           INTEGER NOT NULL,
    password_hash           TEXT    NOT NULL,
    timezone                TEXT    NOT NULL DEFAULT 'UTC',
    rollover_hour           INTEGER NOT NULL DEFAULT 4,
    four_buttons            INTEGER NOT NULL DEFAULT 0,
    font_scale              INTEGER NOT NULL DEFAULT 100,
    fsrs_params             TEXT    NOT NULL DEFAULT '[]',
    fsrs_params_updated_at  INTEGER,
    optimize_include_agent  INTEGER NOT NULL DEFAULT 1,
    created_at              INTEGER NOT NULL
);
CREATE UNIQUE INDEX users_name_tag ON users (username COLLATE NOCASE, discriminator);

CREATE TABLE sessions (
    id          TEXT    PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL
);

CREATE TABLE presets (
    id                  INTEGER PRIMARY KEY,
    user_id             INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                TEXT    NOT NULL,
    description         TEXT    NOT NULL DEFAULT '',
    desired_retention   REAL    NOT NULL,
    new_per_day         INTEGER NOT NULL,
    reviews_per_day     INTEGER NOT NULL,
    learning_steps      TEXT    NOT NULL,
    relearning_steps    TEXT    NOT NULL,
    max_interval        INTEGER NOT NULL,
    created_at          INTEGER NOT NULL
);

CREATE TABLE decks (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    preset_id   INTEGER NOT NULL REFERENCES presets(id),
    exam_date   TEXT,
    created_at  INTEGER NOT NULL
);
CREATE INDEX decks_user ON decks (user_id);

CREATE TABLE media (
    id          TEXT    PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    mime        TEXT    NOT NULL,
    width       INTEGER NOT NULL,
    height      INTEGER NOT NULL,
    bytes       INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE cards (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    deck_id     INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    front       TEXT    NOT NULL DEFAULT '',
    back        TEXT    NOT NULL DEFAULT '',
    front_image TEXT    REFERENCES media(id) ON DELETE SET NULL,
    back_image  TEXT    REFERENCES media(id) ON DELETE SET NULL,
    -- 0 new, 1 learning, 2 review, 3 relearning
    state       INTEGER NOT NULL DEFAULT 0,
    step        INTEGER NOT NULL DEFAULT 0,
    stability   REAL,
    difficulty  REAL,
    due         INTEGER NOT NULL DEFAULT 0,
    last_review INTEGER,
    reps        INTEGER NOT NULL DEFAULT 0,
    lapses      INTEGER NOT NULL DEFAULT 0,
    suspended   INTEGER NOT NULL DEFAULT 0,
    position    INTEGER NOT NULL DEFAULT 0,
    created_by  TEXT    NOT NULL DEFAULT 'web',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX cards_queue ON cards (deck_id, suspended, state, due);
CREATE INDEX cards_user ON cards (user_id);

CREATE TABLE review_log (
    id              INTEGER PRIMARY KEY,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    card_id         INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    deck_id         INTEGER NOT NULL,
    rating          INTEGER NOT NULL,
    reviewed_at     INTEGER NOT NULL,
    state_before    INTEGER NOT NULL,
    elapsed_days    INTEGER NOT NULL,
    scheduled_secs  INTEGER NOT NULL,
    stability       REAL,
    difficulty      REAL,
    duration_ms     INTEGER,
    -- 'web', 'agent' or 'import'
    source          TEXT    NOT NULL,
    prev_card       TEXT
);
CREATE INDEX review_log_user_time ON review_log (user_id, reviewed_at);
CREATE INDEX review_log_card ON review_log (card_id, reviewed_at);

CREATE TABLE api_tokens (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    token_hash   TEXT    NOT NULL UNIQUE,
    prefix       TEXT    NOT NULL,
    can_write    INTEGER NOT NULL,
    can_review   INTEGER NOT NULL,
    all_decks    INTEGER NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at   INTEGER
);

CREATE TABLE api_token_decks (
    token_id INTEGER NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    deck_id  INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    PRIMARY KEY (token_id, deck_id)
);
