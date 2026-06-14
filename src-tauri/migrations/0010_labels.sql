-- Labels: a flat, user-defined, cross-cutting taxonomy that applies to ANY entity (task/event/habit/
-- page/project) — the layer above the rigid structural types. Optionally "actionable": a label can
-- carry scheduling preferences the deterministic scheduler honors (preferred time-of-day window, min/
-- max block, batching). All pref_* NULL/0 = a purely organizational label.
CREATE TABLE IF NOT EXISTS labels (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    name              TEXT NOT NULL,
    color             TEXT NOT NULL,
    icon              TEXT,                      -- optional emoji
    group_name        TEXT,                      -- optional flat group: 'Context' | 'Area' | 'Energy' | custom
    archived          INTEGER NOT NULL DEFAULT 0,
    pref_window_start TEXT,                       -- 'HH:MM' preferred time-of-day window start
    pref_window_end   TEXT,                       -- 'HH:MM' window end
    pref_min_chunk    INTEGER,                    -- min block minutes for tasks carrying this label
    pref_max_chunk    INTEGER,                    -- max block minutes
    pref_batch        INTEGER NOT NULL DEFAULT 0, -- cluster same-label tasks adjacently
    created_at        TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_labels_name ON labels(lower(name));

-- Polymorphic join: a label applied to an entity. Mirrors entity_links (0009). One label may tag many
-- entities; one entity may carry many labels. Cascades when the label is deleted; rows for a deleted
-- entity are swept lazily by reads (no FK to tasks/events/etc, to stay decoupled like entity_links).
CREATE TABLE IF NOT EXISTS entity_labels (
    label_id    INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL,                    -- 'task' | 'event' | 'habit' | 'page' | 'project'
    entity_id   INTEGER NOT NULL,
    PRIMARY KEY (label_id, entity_kind, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_entity_labels_entity ON entity_labels(entity_kind, entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_labels_label  ON entity_labels(label_id);
