-- Every new card asks for the user's highest position; without this index
-- that read every card, which made big imports slow down as they went.
CREATE INDEX cards_position ON cards (user_id, position);
