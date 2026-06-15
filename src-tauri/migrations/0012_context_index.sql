-- Cross-entity recall substrate for the Context Engine (see CONTEXT_ENGINE_PLAN.md).
-- One polymorphic row per indexed entity, mirroring entity_labels/entity_links. Lets recall span
-- tasks/events/pages (and later people/goals) through a single query and a single embedding store.
CREATE TABLE IF NOT EXISTS entity_index (
  entity_kind     TEXT    NOT NULL,           -- 'task' | 'event' | 'page' | 'person' | 'goal'
  entity_id       INTEGER NOT NULL,
  text            TEXT    NOT NULL,           -- projected, embeddable text for this entity
  text_hash       TEXT    NOT NULL,           -- stable hash of `text`; skip re-embedding when unchanged
  embedding       BLOB,                       -- little-endian f32; NULL until indexed (graceful degradation)
  embedding_model TEXT,                       -- model that produced `embedding` (dims change ⇒ reindex)
  updated_at      TEXT    NOT NULL,
  PRIMARY KEY (entity_kind, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_entity_index_kind ON entity_index(entity_kind);
