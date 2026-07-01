-- Distinguish AI-tracked memory facts from user vault pages. NULL / 'user' = a normal vault page;
-- 'memory' = a durable fact the AI stored (from the chat "Remember this?" chip). Memory rows are
-- HIDDEN from the vault tree + pages list and surfaced in Settings ▸ AI Memory instead — but they
-- still feed on-device recall. Sync-safe (0015's change-capture reads columns dynamically).
ALTER TABLE notes ADD COLUMN origin TEXT;
