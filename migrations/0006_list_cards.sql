-- List cards: the back is a numbered list and every item has its own memory.
-- The card's own scheduling fields hold the weakest item, so queues and
-- counts keep working unchanged.
ALTER TABLE cards ADD COLUMN is_list INTEGER NOT NULL DEFAULT 0;

CREATE TABLE card_items (
    id          INTEGER PRIMARY KEY,
    card_id     INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    text        TEXT    NOT NULL,
    state       INTEGER NOT NULL DEFAULT 0,
    step        INTEGER NOT NULL DEFAULT 0,
    stability   REAL,
    difficulty  REAL,
    due         INTEGER NOT NULL DEFAULT 0,
    last_review INTEGER,
    reps        INTEGER NOT NULL DEFAULT 0,
    lapses      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX card_items_card ON card_items (card_id, position);

-- An answer to a list card: how many items were asked and remembered, and
-- the items before it (for undo).
ALTER TABLE review_log ADD COLUMN items_asked INTEGER;
ALTER TABLE review_log ADD COLUMN items_recalled INTEGER;
ALTER TABLE review_log ADD COLUMN prev_items TEXT;

-- One row per item per answer; the optimizer learns from these instead of
-- the card's summary rating.
CREATE TABLE item_review_log (
    id            INTEGER PRIMARY KEY,
    review_log_id INTEGER NOT NULL REFERENCES review_log(id) ON DELETE CASCADE,
    item_id       INTEGER NOT NULL REFERENCES card_items(id) ON DELETE CASCADE,
    rating        INTEGER NOT NULL,
    state_before  INTEGER NOT NULL,
    reviewed_at   INTEGER NOT NULL
);
CREATE INDEX item_review_log_item ON item_review_log (item_id, reviewed_at);
CREATE INDEX item_review_log_review ON item_review_log (review_log_id);
