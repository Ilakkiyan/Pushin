-- Habit tracker: daily habits + a per-day completion log used to derive streaks
-- and consistency metrics. `day` is a naive-local calendar date ("YYYY-MM-DD"); a row's
-- presence means the habit was completed that day.

CREATE TABLE IF NOT EXISTS habits (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL DEFAULT '#22c55e',
    cadence     TEXT NOT NULL DEFAULT 'daily',
    archived    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS habit_logs (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    habit_id  INTEGER NOT NULL REFERENCES habits(id) ON DELETE CASCADE,
    day       TEXT NOT NULL,   -- YYYY-MM-DD
    UNIQUE(habit_id, day)
);

CREATE INDEX IF NOT EXISTS idx_habit_logs_habit ON habit_logs(habit_id);
