-- Pushin initial schema. Datetimes are stored as naive-local ISO strings
-- ("YYYY-MM-DDTHH:MM:SS") and interpreted in the user's local timezone.

CREATE TABLE IF NOT EXISTS projects (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    color       TEXT NOT NULL DEFAULT '#6366f1',
    created_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id         INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    title              TEXT NOT NULL,
    notes              TEXT NOT NULL DEFAULT '',
    estimated_minutes  INTEGER NOT NULL DEFAULT 30,
    deadline           TEXT,            -- ISO or NULL
    earliest_start     TEXT,            -- ISO or NULL
    priority           INTEGER NOT NULL DEFAULT 2,   -- 1 low .. 4 urgent
    min_chunk_minutes  INTEGER NOT NULL DEFAULT 30,
    max_chunk_minutes  INTEGER NOT NULL DEFAULT 120,
    status             TEXT NOT NULL DEFAULT 'todo',  -- todo|scheduled|in_progress|done
    created_at         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_deps (
    task_id            INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_task_id)
);

-- Immovable calendar items (meetings, busy time, or pulled from Google).
CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    title       TEXT NOT NULL,
    start       TEXT NOT NULL,
    end         TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'fixed',   -- fixed|busy
    source      TEXT NOT NULL DEFAULT 'manual',  -- manual|import|google
    created_at  TEXT NOT NULL,
    -- Google-sync seam (unused until GoogleProvider lands):
    provider    TEXT,
    external_id TEXT,
    account_id  INTEGER REFERENCES calendar_accounts(id) ON DELETE SET NULL,
    etag        TEXT
);

-- Scheduler output: when a task is actually worked.
CREATE TABLE IF NOT EXISTS blocks (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id     INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    start       TEXT NOT NULL,
    end         TEXT NOT NULL,
    locked      INTEGER NOT NULL DEFAULT 0,  -- 1 = user-pinned, treated as fixed
    -- Google-sync seam:
    provider    TEXT,
    external_id TEXT,
    sync_state  TEXT
);

CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);

-- Google Calendar seam. OAuth tokens live in the OS keychain, NOT here.
CREATE TABLE IF NOT EXISTS calendar_accounts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    provider     TEXT NOT NULL,           -- 'google'
    email        TEXT NOT NULL,
    sync_token   TEXT,
    connected_at TEXT NOT NULL
);

-- Booking-page seam: the offerings someone can book.
CREATE TABLE IF NOT EXISTS event_types (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT NOT NULL,
    duration_minutes INTEGER NOT NULL DEFAULT 30,
    buffer_minutes   INTEGER NOT NULL DEFAULT 0,
    color            TEXT NOT NULL DEFAULT '#0ea5e9'
);

-- Booked slots -> become fixed events that the scheduler avoids.
CREATE TABLE IF NOT EXISTS bookings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type_id INTEGER NOT NULL REFERENCES event_types(id) ON DELETE CASCADE,
    invitee_name  TEXT NOT NULL,
    invitee_email TEXT NOT NULL,
    start         TEXT NOT NULL,
    end           TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'confirmed',
    created_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_blocks_task ON blocks(task_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_events_start ON events(start);
