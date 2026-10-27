-- Settings shows how many reviews the optimizer would learn from. With this
-- index SQLite counts them from the index alone instead of reading every
-- review row (each carries a snapshot of the card before it).
CREATE INDEX review_log_count ON review_log (user_id, source, rating, items_asked);
