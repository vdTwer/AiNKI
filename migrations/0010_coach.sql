-- The card coach flags cards by simple rules; "Keep as is" is remembered
-- here. A leech or split flag comes back after a few more lapses (`lapses`
-- is the count when it was dismissed); a duplicate stays dismissed.
CREATE TABLE coach_dismissals (
    card_id     INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL,
    lapses      INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (card_id, kind)
);
