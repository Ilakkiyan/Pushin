-- Two-way markdown vault: map each vault page to its file on disk. `rel_path` is the path relative
-- to the user's chosen vault folder (e.g. "Daily/2026-06/2026-06-28.md"). NULL until the page has
-- been written to a file. Device-local-ish — the relative path is rule-based so it matches across
-- devices even though the vault root differs.
ALTER TABLE notes ADD COLUMN rel_path TEXT;
