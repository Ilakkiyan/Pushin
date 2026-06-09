-- Hermes: the on-device memory layer. Each note is freeform text the user gives Pushin; the
-- embedding (a little-endian f32 vector) is computed on-device and used for semantic recall.
-- It's NULL until/unless an embedding backend is available (recall falls back to keyword search).
CREATE TABLE IF NOT EXISTS notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    embedding BLOB,           -- little-endian f32[]; NULL = not yet indexed
    embedding_model TEXT,     -- which model produced `embedding`
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notes_created ON notes(created_at);
