-- Offline answer ids are only unique per user, so one user can't block
-- another's answers by guessing ids.
DROP INDEX review_log_client_id;
CREATE UNIQUE INDEX review_log_client_id ON review_log (user_id, client_id) WHERE client_id IS NOT NULL;

-- Rotated refresh tokens are kept briefly to detect reuse (token theft).
ALTER TABLE oauth_tokens ADD COLUMN used_at INTEGER;
