-- How long an answer took, measured in the browser to the millisecond.
-- think_ms runs from the front showing to the answer showing, answer_ms
-- from there to the rating; neither counts time the tab was hidden, which
-- away_ms holds. input is how the rating was given: 'key', 'mouse',
-- 'touch' or 'pen'. All NULL when the page had no JavaScript, for agent
-- reviews, and for reviews from before this was measured.
ALTER TABLE review_log ADD COLUMN think_ms INTEGER;
ALTER TABLE review_log ADD COLUMN answer_ms INTEGER;
ALTER TABLE review_log ADD COLUMN away_ms INTEGER;
ALTER TABLE review_log ADD COLUMN input TEXT;
