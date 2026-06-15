-- Focus Mode / time-tracking (ROADMAP Phase 4). A row per focus session on a task; `end` is NULL
-- while running. Actual minutes are derived from start/end. This is the actuals data the adaptive
-- scheduler will later use to learn real task durations.
CREATE TABLE IF NOT EXISTS focus_sessions (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id    INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  start      TEXT NOT NULL,
  end        TEXT,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_focus_task ON focus_sessions(task_id);
