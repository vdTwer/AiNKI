-- Pausing and archiving decks. A paused deck stays on the deck list; an
-- archived one is put away. Either way its cards rest: they are suspended,
-- and `held` marks the ones the deck suspended, so resuming it wakes only
-- those and leaves cards suspended by hand as they are.
ALTER TABLE decks ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
    CHECK (status IN ('active', 'paused', 'archived'));
ALTER TABLE cards ADD COLUMN held INTEGER NOT NULL DEFAULT 0;
