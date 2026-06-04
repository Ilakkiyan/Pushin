-- Google Calendar account: OAuth tokens + incremental sync state.
-- (Tokens live in the app-data SQLite for now; moving them to the OS keychain is a
-- hardening follow-up.)
ALTER TABLE calendar_accounts ADD COLUMN access_token  TEXT;
ALTER TABLE calendar_accounts ADD COLUMN refresh_token TEXT;
ALTER TABLE calendar_accounts ADD COLUMN token_expiry  TEXT;   -- ISO; when access_token expires
ALTER TABLE calendar_accounts ADD COLUMN calendar_id   TEXT;   -- target calendar ("primary")
