-- Card kinds: how a card is studied. 'basic' is the usual text card; a
-- 'chess' card's front is a position and its back the move that answers it
-- (see src/chess.rs).
ALTER TABLE cards ADD COLUMN kind TEXT NOT NULL DEFAULT 'basic';
