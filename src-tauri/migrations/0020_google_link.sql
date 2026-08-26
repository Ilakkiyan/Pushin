-- The shared Google Calendar link: the part of the Google setup that is the same on every one of a
-- user's devices, so pairing a new device auto-applies it instead of making the user re-paste the
-- OAuth client and re-run consent.
--
-- Synced (see sync::schema::TABLES). The refresh token is NOT a column here: it travels as a
-- keychain-backed field on the changeset and lands in the peer's OS keychain. The per-device
-- plumbing (access token, expiry, and the `sync_token` incremental pull cursor) stays in
-- `calendar_accounts`, which is deliberately not synced.
CREATE TABLE IF NOT EXISTS google_link (
  id            INTEGER PRIMARY KEY,
  provider      TEXT NOT NULL DEFAULT 'google',
  email         TEXT NOT NULL DEFAULT '',
  calendar_id   TEXT NOT NULL DEFAULT 'primary',
  client_id     TEXT NOT NULL DEFAULT '',
  client_secret TEXT NOT NULL DEFAULT '',
  updated_at    TEXT NOT NULL DEFAULT ''
);
-- Deliberately NOT unique on `provider`: two devices can each connect Google *before* they are
-- paired, and a UNIQUE index would make the first sync session fail on the incoming row rather than
-- reconcile it. `db::get_google_link` instead picks a deterministic winner and `adopt_google_link`
-- prunes the losers, so the mesh converges on one link.
CREATE INDEX IF NOT EXISTS idx_google_link_provider ON google_link(provider);
