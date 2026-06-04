//! SQLite persistence, owned entirely by the Rust core (rusqlite).
//! The frontend never touches the DB directly — it goes through Tauri commands.

use crate::model::*;
use anyhow::Result;
use rusqlite::{params, Connection, Row};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_google.sql");

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(MIGRATION_0001)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    if version < 2 {
        conn.execute_batch(MIGRATION_0002)?;
        conn.pragma_update(None, "user_version", 2)?;
    }
    Ok(())
}

fn now_iso() -> String {
    chrono::Local::now().naive_local().format(DT_FMT).to_string()
}

// ---------- Settings ----------

pub fn get_settings(conn: &Connection) -> Result<Settings> {
    let row: Option<String> = conn
        .query_row("SELECT value_json FROM settings WHERE key = 'app'", [], |r| r.get(0))
        .ok();
    match row {
        Some(json) => Ok(serde_json::from_str(&json).unwrap_or_default()),
        None => Ok(Settings::default()),
    }
}

pub fn save_settings(conn: &Connection, s: &Settings) -> Result<()> {
    let json = serde_json::to_string(s)?;
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES('app', ?1)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![json],
    )?;
    Ok(())
}

// ---------- Projects ----------

fn row_to_project(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get("id")?,
        name: r.get("name")?,
        color: r.get("color")?,
        created_at: r.get("created_at")?,
    })
}

pub fn list_projects(conn: &Connection) -> Result<Vec<Project>> {
    let mut stmt = conn.prepare("SELECT * FROM projects ORDER BY created_at")?;
    let rows = stmt.query_map([], row_to_project)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn insert_project(conn: &Connection, name: &str, color: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO projects(name, color, created_at) VALUES(?1, ?2, ?3)",
        params![name, color, now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

// ---------- Tasks ----------

fn row_to_task(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        title: r.get("title")?,
        notes: r.get("notes")?,
        estimated_minutes: r.get("estimated_minutes")?,
        deadline: r.get("deadline")?,
        earliest_start: r.get("earliest_start")?,
        priority: r.get("priority")?,
        min_chunk_minutes: r.get("min_chunk_minutes")?,
        max_chunk_minutes: r.get("max_chunk_minutes")?,
        status: r.get("status")?,
        created_at: r.get("created_at")?,
        depends_on: Vec::new(),
    })
}

pub fn list_tasks(conn: &Connection) -> Result<Vec<Task>> {
    let mut stmt = conn.prepare("SELECT * FROM tasks ORDER BY created_at")?;
    let mut tasks: Vec<Task> = stmt.query_map([], row_to_task)?.collect::<rusqlite::Result<_>>()?;

    let mut dep_stmt = conn.prepare("SELECT depends_on_task_id FROM task_deps WHERE task_id = ?1")?;
    for t in &mut tasks {
        let deps = dep_stmt.query_map(params![t.id], |r| r.get::<_, i64>(0))?;
        t.depends_on = deps.collect::<rusqlite::Result<_>>()?;
    }
    Ok(tasks)
}

/// Insert a task. `deps` are task ids it depends on (must already exist).
#[allow(clippy::too_many_arguments)]
pub fn insert_task(
    conn: &Connection,
    project_id: Option<i64>,
    title: &str,
    notes: &str,
    estimated_minutes: i64,
    deadline: Option<&str>,
    priority: i64,
    min_chunk: i64,
    max_chunk: i64,
    deps: &[i64],
) -> Result<i64> {
    conn.execute(
        "INSERT INTO tasks(project_id, title, notes, estimated_minutes, deadline, priority,
                           min_chunk_minutes, max_chunk_minutes, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'todo', ?9)",
        params![project_id, title, notes, estimated_minutes, deadline, priority, min_chunk, max_chunk, now_iso()],
    )?;
    let id = conn.last_insert_rowid();
    for d in deps {
        conn.execute(
            "INSERT OR IGNORE INTO task_deps(task_id, depends_on_task_id) VALUES(?1, ?2)",
            params![id, d],
        )?;
    }
    Ok(id)
}

pub fn add_task_dep(conn: &Connection, task_id: i64, depends_on_task_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO task_deps(task_id, depends_on_task_id) VALUES(?1, ?2)",
        params![task_id, depends_on_task_id],
    )?;
    Ok(())
}

pub fn set_task_status(conn: &Connection, id: i64, status: &str) -> Result<()> {
    conn.execute("UPDATE tasks SET status = ?2 WHERE id = ?1", params![id, status])?;
    Ok(())
}

pub fn delete_task(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------- Events ----------

fn row_to_event(r: &Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: r.get("id")?,
        title: r.get("title")?,
        start: r.get("start")?,
        end: r.get("end")?,
        kind: r.get("kind")?,
        source: r.get("source")?,
        created_at: r.get("created_at")?,
        provider: r.get("provider")?,
        external_id: r.get("external_id")?,
        account_id: r.get("account_id")?,
        etag: r.get("etag")?,
    })
}

pub fn list_events(conn: &Connection) -> Result<Vec<Event>> {
    let mut stmt = conn.prepare("SELECT * FROM events ORDER BY start")?;
    let rows: Vec<Event> = stmt.query_map([], row_to_event)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn insert_event(conn: &Connection, title: &str, start: &str, end: &str, kind: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO events(title, start, end, kind, source, created_at)
         VALUES(?1, ?2, ?3, ?4, 'manual', ?5)",
        params![title, start, end, kind, now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_event(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM events WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn update_event(conn: &Connection, id: i64, title: &str, start: &str, end: &str) -> Result<()> {
    conn.execute(
        "UPDATE events SET title = ?2, start = ?3, end = ?4 WHERE id = ?1",
        params![id, title, start, end],
    )?;
    Ok(())
}

// ---------- Blocks ----------

fn row_to_block(r: &Row) -> rusqlite::Result<Block> {
    Ok(Block {
        id: r.get("id")?,
        task_id: r.get("task_id")?,
        start: r.get("start")?,
        end: r.get("end")?,
        locked: r.get::<_, i64>("locked")? != 0,
        provider: r.get("provider")?,
        external_id: r.get("external_id")?,
        sync_state: r.get("sync_state")?,
    })
}

pub fn list_blocks(conn: &Connection) -> Result<Vec<Block>> {
    let mut stmt = conn.prepare("SELECT * FROM blocks ORDER BY start")?;
    let rows: Vec<Block> = stmt.query_map([], row_to_block)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Replace all unlocked blocks with freshly scheduled ones (locked blocks survive).
pub fn replace_unlocked_blocks(conn: &mut Connection, new_blocks: &[Block]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM blocks WHERE locked = 0", [])?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO blocks(task_id, start, end, locked) VALUES(?1, ?2, ?3, 0)",
        )?;
        for b in new_blocks {
            stmt.execute(params![b.task_id, b.start, b.end])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn set_block_locked(conn: &Connection, id: i64, locked: bool, start: &str, end: &str) -> Result<()> {
    conn.execute(
        "UPDATE blocks SET locked = ?2, start = ?3, end = ?4 WHERE id = ?1",
        params![id, locked as i64, start, end],
    )?;
    Ok(())
}

// ---------- Google account ----------

pub fn get_google_account(conn: &Connection) -> Result<Option<GoogleAccount>> {
    let row = conn
        .query_row(
            "SELECT id, email, calendar_id, sync_token, access_token, refresh_token, token_expiry, connected_at
             FROM calendar_accounts WHERE provider = 'google' LIMIT 1",
            [],
            |r| {
                Ok(GoogleAccount {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    calendar_id: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "primary".into()),
                    sync_token: r.get(3)?,
                    access_token: r.get(4)?,
                    refresh_token: r.get(5)?,
                    token_expiry: r.get(6)?,
                    connected_at: r.get(7)?,
                })
            },
        )
        .ok();
    Ok(row)
}

/// Replace the single Google account with fresh tokens after a successful OAuth.
pub fn save_google_account(
    conn: &Connection,
    email: &str,
    calendar_id: &str,
    access_token: &str,
    refresh_token: &str,
    token_expiry: &str,
) -> Result<()> {
    conn.execute("DELETE FROM calendar_accounts WHERE provider = 'google'", [])?;
    conn.execute(
        "INSERT INTO calendar_accounts(provider, email, calendar_id, access_token, refresh_token, token_expiry, sync_token, connected_at)
         VALUES('google', ?1, ?2, ?3, ?4, ?5, NULL, ?6)",
        params![email, calendar_id, access_token, refresh_token, token_expiry, now_iso()],
    )?;
    Ok(())
}

pub fn update_google_tokens(conn: &Connection, id: i64, access_token: &str, token_expiry: &str) -> Result<()> {
    conn.execute(
        "UPDATE calendar_accounts SET access_token = ?2, token_expiry = ?3 WHERE id = ?1",
        params![id, access_token, token_expiry],
    )?;
    Ok(())
}

pub fn update_google_sync_token(conn: &Connection, id: i64, sync_token: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE calendar_accounts SET sync_token = ?2 WHERE id = ?1",
        params![id, sync_token],
    )?;
    Ok(())
}

pub fn delete_google_account(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM calendar_accounts WHERE provider = 'google'", [])?;
    Ok(())
}

// ---------- Google event sync helpers ----------

pub fn find_event_by_external(conn: &Connection, external_id: &str) -> Result<Option<Event>> {
    let row = conn
        .query_row("SELECT * FROM events WHERE external_id = ?1 LIMIT 1", params![external_id], row_to_event)
        .ok();
    Ok(row)
}

/// Insert an event pulled from Google.
pub fn insert_google_event(conn: &Connection, title: &str, start: &str, end: &str, external_id: &str, etag: Option<&str>) -> Result<i64> {
    conn.execute(
        "INSERT INTO events(title, start, end, kind, source, created_at, provider, external_id, etag)
         VALUES(?1, ?2, ?3, 'fixed', 'google', ?4, 'google', ?5, ?6)",
        params![title, start, end, now_iso(), external_id, etag],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Update an event's core fields + etag (used when Google reports a change).
pub fn update_event_synced(conn: &Connection, id: i64, title: &str, start: &str, end: &str, etag: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE events SET title = ?2, start = ?3, end = ?4, etag = ?5 WHERE id = ?1",
        params![id, title, start, end, etag],
    )?;
    Ok(())
}

pub fn delete_events_by_external(conn: &Connection, external_id: &str) -> Result<()> {
    conn.execute("DELETE FROM events WHERE external_id = ?1", params![external_id])?;
    Ok(())
}

/// Record that a local event was pushed to Google.
pub fn mark_event_pushed(conn: &Connection, id: i64, external_id: &str, etag: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE events SET provider = 'google', external_id = ?2, etag = ?3 WHERE id = ?1",
        params![id, external_id, etag],
    )?;
    Ok(())
}

// ---------- Event types & bookings (booking-page seam) ----------

fn row_to_event_type(r: &Row) -> rusqlite::Result<EventType> {
    Ok(EventType {
        id: r.get("id")?,
        name: r.get("name")?,
        duration_minutes: r.get("duration_minutes")?,
        buffer_minutes: r.get("buffer_minutes")?,
        color: r.get("color")?,
    })
}

pub fn list_event_types(conn: &Connection) -> Result<Vec<EventType>> {
    let mut stmt = conn.prepare("SELECT * FROM event_types ORDER BY id")?;
    let rows: Vec<EventType> = stmt.query_map([], row_to_event_type)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn insert_event_type(conn: &Connection, name: &str, duration: i64, buffer: i64, color: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO event_types(name, duration_minutes, buffer_minutes, color) VALUES(?1, ?2, ?3, ?4)",
        params![name, duration, buffer, color],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_event_type(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM event_types WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn list_bookings(conn: &Connection) -> Result<Vec<Booking>> {
    let mut stmt = conn.prepare("SELECT * FROM bookings ORDER BY start")?;
    let rows: Vec<Booking> = stmt
        .query_map([], |r| {
            Ok(Booking {
                id: r.get("id")?,
                event_type_id: r.get("event_type_id")?,
                invitee_name: r.get("invitee_name")?,
                invitee_email: r.get("invitee_email")?,
                start: r.get("start")?,
                end: r.get("end")?,
                status: r.get("status")?,
                created_at: r.get("created_at")?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Create a booking AND mirror it as a fixed event so the scheduler avoids it.
pub fn insert_booking(
    conn: &mut Connection,
    event_type_id: i64,
    name: &str,
    email: &str,
    start: &str,
    end: &str,
) -> Result<i64> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO bookings(event_type_id, invitee_name, invitee_email, start, end, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, 'confirmed', ?6)",
        params![event_type_id, name, email, start, end, now_iso()],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO events(title, start, end, kind, source, created_at)
         VALUES(?1, ?2, ?3, 'fixed', 'manual', ?4)",
        params![format!("Call: {name}"), start, end, now_iso()],
    )?;
    tx.commit()?;
    Ok(id)
}
