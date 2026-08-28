-- File-level vault sync: replicate the FILES in the vault folder (attachments, PDFs, images —
-- anything that is not a page mirror) between paired devices, alongside the row-level sync of
-- pages themselves.
--
-- Two tables, deliberately:
--
--   `vault_file_index` is the SHARED truth — one row per synced file, carrying its content hash
--   and size. It joins the synced-table registry (sync::schema), so its rows replicate as LWW like
--   any other table and its deletes leave tombstones. It arrived after the frozen 0015 generator
--   ran, so (like `google_link` in 0020 and `ics_subscriptions` in 0022) the migration applies
--   0015's columns/backfill/triggers itself — the DDL for those comes from the TableSpec so the
--   two can never drift. This .sql file is the documentation.
--
--   `vault_file_seen` is DEVICE-LOCAL scan bookkeeping: the (mtime, size) a file had the last time
--   we hashed it, so a rescan can skip re-reading a 200 MB attachment that hasn't changed. It must
--   NOT sync — mtime differs on every device by construction, and syncing it would have two devices
--   dirtying each other's rows forever.
--
-- The `uuid` is DERIVED from `rel_path` rather than random. Two already-paired devices that both
-- hold the same attachment would otherwise each mint a random uuid for it, and applying the peer's
-- row would hit the UNIQUE(rel_path) index and abort the ENTIRE changeset batch — the failure mode
-- one .ics feed once caused for all of sync (9ae15ce). Deriving it makes both devices arrive at the
-- same id independently, so LWW just merges them.
CREATE TABLE IF NOT EXISTS vault_file_index (
  id         INTEGER PRIMARY KEY,
  rel_path   TEXT NOT NULL UNIQUE,
  hash       TEXT NOT NULL,
  size       INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS vault_file_seen (
  rel_path TEXT PRIMARY KEY,
  mtime    INTEGER NOT NULL DEFAULT 0,
  size     INTEGER NOT NULL DEFAULT 0,
  hash     TEXT NOT NULL DEFAULT ''
);
