-- Multicards: list cards generalized. `multi` says how the back is split
-- into items (list, sections or parts); NULL is a plain card.
ALTER TABLE cards ADD COLUMN multi TEXT;
UPDATE cards SET multi = 'list' WHERE is_list = 1;
ALTER TABLE cards DROP COLUMN is_list;
