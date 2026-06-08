-- Completed / archived projects.
-- archived_at is NULL for active projects, and an ISO timestamp once the
-- project has been marked complete and moved to the "Completed" bin.
ALTER TABLE projects ADD COLUMN archived_at TEXT;
