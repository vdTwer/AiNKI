-- Merged decks copy cards. A copy remembers the card it all started from
-- (the original's own original, for a copy of a copy), so the coach doesn't
-- call a copy a duplicate of its original. No foreign key: the original may
-- be deleted, and its copies stay one family.
ALTER TABLE cards ADD COLUMN copy_of INTEGER;
