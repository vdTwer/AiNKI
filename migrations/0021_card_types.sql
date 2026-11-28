-- Card types people bring themselves: a package (a manifest, one ES module
-- and the files it needs) uploaded to an account. A package is data to the
-- server: it is stored and served, never run. Its code runs only in the
-- browser, inside a sandboxed frame. Every upload is a new row, so a card
-- keeps the version it was written with.
CREATE TABLE card_types (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Unguessable: the sandboxed frame loads its files without cookies, so
    -- the path itself is the key.
    token       TEXT    NOT NULL UNIQUE,
    name        TEXT    NOT NULL,
    version     INTEGER NOT NULL,
    manifest    TEXT    NOT NULL,
    bytes       INTEGER NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX card_types_user ON card_types (user_id);

CREATE TABLE card_type_files (
    type_id     INTEGER NOT NULL REFERENCES card_types(id) ON DELETE CASCADE,
    path        TEXT    NOT NULL,
    mime        TEXT    NOT NULL,
    body        BLOB    NOT NULL,
    PRIMARY KEY (type_id, path)
);

-- A card of such a type: which one, and its fields as a JSON object. The
-- front and back hold a plain-text summary so search, lists and agents work.
ALTER TABLE cards ADD COLUMN type_id INTEGER REFERENCES card_types(id);
ALTER TABLE cards ADD COLUMN fields TEXT;

-- What the learner did before the reveal (a move played, say), as JSON from
-- the card's type. Stored as data, never shown as HTML.
ALTER TABLE review_log ADD COLUMN response TEXT;
