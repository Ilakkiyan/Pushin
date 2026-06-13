-- Second-brain roadmap: bridge the calendar and the vault, and prep frictionless capture.
-- Daily Notes: a page that IS a calendar day's note (NULL on normal pages); one page per date.
ALTER TABLE notes ADD COLUMN daily_date TEXT;            -- 'YYYY-MM-DD'
CREATE UNIQUE INDEX IF NOT EXISTS idx_notes_daily ON notes(daily_date) WHERE daily_date IS NOT NULL;

-- A lightweight inbox flag for one-box quick capture (Phase 8). 0 = normal page.
ALTER TABLE notes ADD COLUMN inbox INTEGER NOT NULL DEFAULT 0;

-- Generic link between a page and another entity (a task or event), powering cross-references so
-- the calendar becomes an index into your knowledge. Cascades when the page is deleted; rows for a
-- deleted task/event are swept lazily by the read queries (no FK to tasks/events to stay decoupled).
CREATE TABLE IF NOT EXISTS entity_links (
    page_id     INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,                           -- 'task' | 'event'
    entity_id   INTEGER NOT NULL,
    PRIMARY KEY (page_id, entity_kind, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_entity_links_entity ON entity_links(entity_kind, entity_id);
