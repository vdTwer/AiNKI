-- Time the card sat behind the desktop's Pause cover: a break the user
-- called themselves (a glass of water, the door). Never counted in
-- think_ms or answer_ms. NULL when not measured.
ALTER TABLE review_log ADD COLUMN paused_ms INTEGER;
