-- Vault folders. A folder is a `notes` row like any other page — same tree (parent_id), same
-- move/rename/delete plumbing — flagged so the UI can draw it as a container instead of a document.
-- Modelling it as a page (rather than a separate table) means the existing drag-reparent, sort_order
-- and device-sync machinery all apply unchanged; 0015's change-capture reads columns dynamically, so
-- a plain ALTER is sync-safe.
--
-- Folders carry no body: they're skipped by embedding/recall, keyword search and the link graph, so
-- an empty container can never surface as a search hit or a stranded graph node.
ALTER TABLE notes ADD COLUMN is_folder INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_notes_folder ON notes(is_folder);
