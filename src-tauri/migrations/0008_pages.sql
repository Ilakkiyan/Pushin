-- Vault: evolve the flat Hermes `notes` table into full documents ("pages"). The table keeps its
-- name (preserves already-computed embeddings; recall still scores over `content`, the derived
-- plaintext). New columns add a Notion-style title/icon, an Obsidian-style page tree (parent_id),
-- and the BlockNote block JSON. Legacy notes have NULL title/content_json and are read as a plain
-- paragraph doc with a title derived from their first line.
ALTER TABLE notes ADD COLUMN title TEXT;                 -- NULL on legacy rows → derived on read
ALTER TABLE notes ADD COLUMN icon TEXT;                  -- optional emoji
ALTER TABLE notes ADD COLUMN parent_id INTEGER REFERENCES notes(id) ON DELETE SET NULL;
ALTER TABLE notes ADD COLUMN content_json TEXT;          -- BlockNote block array (JSON); NULL = legacy
ALTER TABLE notes ADD COLUMN sort_order REAL NOT NULL DEFAULT 0;
ALTER TABLE notes ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_notes_parent ON notes(parent_id);

-- One row per wikilink between pages, recomputed on save. target_id is resolved when the linked
-- page exists; an unresolved ("ghost") link keeps target_id NULL and carries target_title so it can
-- resolve later when a matching page is created. The graph/backlinks views resolve any remaining
-- NULLs by title at read time.
CREATE TABLE IF NOT EXISTS page_links (
    source_id    INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    target_id    INTEGER REFERENCES notes(id) ON DELETE CASCADE,
    target_title TEXT NOT NULL,
    PRIMARY KEY (source_id, target_title)
);
CREATE INDEX IF NOT EXISTS idx_links_target ON page_links(target_id);
CREATE INDEX IF NOT EXISTS idx_links_source ON page_links(source_id);
