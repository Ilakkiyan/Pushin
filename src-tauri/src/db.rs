//! SQLite persistence, owned entirely by the Rust core (rusqlite).
//! The frontend never touches the DB directly — it goes through Tauri commands.

use crate::model::*;
use anyhow::Result;
use rusqlite::{params, Connection, Row};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_google.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_habits.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_habit_duration.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_project_archive.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_notes.sql");
const MIGRATION_0007: &str = include_str!("../migrations/0007_habit_cadence.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_pages.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_brain.sql");

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
    if version < 3 {
        conn.execute_batch(MIGRATION_0003)?;
        conn.pragma_update(None, "user_version", 3)?;
    }
    if version < 4 {
        conn.execute_batch(MIGRATION_0004)?;
        conn.pragma_update(None, "user_version", 4)?;
    }
    if version < 5 {
        conn.execute_batch(MIGRATION_0005)?;
        conn.pragma_update(None, "user_version", 5)?;
    }
    if version < 6 {
        conn.execute_batch(MIGRATION_0006)?;
        conn.pragma_update(None, "user_version", 6)?;
    }
    if version < 7 {
        conn.execute_batch(MIGRATION_0007)?;
        conn.pragma_update(None, "user_version", 7)?;
    }
    if version < 8 {
        conn.execute_batch(MIGRATION_0008)?;
        conn.pragma_update(None, "user_version", 8)?;
    }
    if version < 9 {
        conn.execute_batch(MIGRATION_0009)?;
        conn.pragma_update(None, "user_version", 9)?;
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
        archived_at: r.get("archived_at")?,
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

/// Delete a project. Its tasks survive — the FK is `ON DELETE SET NULL`, so they
/// fall back to the "No project" bucket rather than being destroyed.
pub fn delete_project(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM projects WHERE id = ?1", params![id])?;
    Ok(())
}

/// Mark a project complete (move to the Completed bin) or restore it to active.
/// Completing also finishes any still-open tasks so they leave the schedule.
pub fn set_project_archived(conn: &Connection, id: i64, archived: bool) -> Result<()> {
    let archived_at = if archived { Some(now_iso()) } else { None };
    conn.execute(
        "UPDATE projects SET archived_at = ?2 WHERE id = ?1",
        params![id, archived_at],
    )?;
    if archived {
        conn.execute(
            "UPDATE tasks SET status = 'done' WHERE project_id = ?1 AND status != 'done'",
            params![id],
        )?;
    }
    Ok(())
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

    // All deps in a single query, grouped by task — avoids an N+1 query-per-task.
    let mut by_task: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    let mut dep_stmt = conn.prepare("SELECT task_id, depends_on_task_id FROM task_deps")?;
    let rows = dep_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (task_id, dep) = row?;
        by_task.entry(task_id).or_default().push(dep);
    }
    for t in &mut tasks {
        if let Some(deps) = by_task.remove(&t.id) {
            t.depends_on = deps;
        }
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
//
// OAuth tokens live in the OS keychain (`crate::secrets`), never in plaintext SQLite. The
// `calendar_accounts` row keeps only non-secret metadata; the token columns stay NULL on modern
// installs and exist solely as a graceful fallback when the keychain is unavailable (and as the
// source for the one-time migration of legacy plaintext tokens in `get_google_account`).

const KC_ACCESS: &str = "google-access-token";
const KC_REFRESH: &str = "google-refresh-token";

/// Resolve a token: prefer the keychain; otherwise migrate a legacy plaintext column value into
/// the keychain (nulling the column) or, if the keychain is unavailable, return the column value.
fn resolve_token(conn: &Connection, id: i64, kc_key: &str, null_col_sql: &str, legacy: Option<String>) -> Option<String> {
    if let Some(v) = crate::secrets::get(kc_key) {
        return Some(v);
    }
    let v = legacy.filter(|s| !s.is_empty())?;
    if crate::secrets::set(kc_key, &v) {
        let _ = conn.execute(null_col_sql, params![id]); // migrated in → drop the plaintext copy
    }
    Some(v)
}

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
    let Some(mut acct) = row else { return Ok(None) };
    acct.access_token = resolve_token(
        conn,
        acct.id,
        KC_ACCESS,
        "UPDATE calendar_accounts SET access_token = NULL WHERE id = ?1",
        acct.access_token.take(),
    );
    acct.refresh_token = resolve_token(
        conn,
        acct.id,
        KC_REFRESH,
        "UPDATE calendar_accounts SET refresh_token = NULL WHERE id = ?1",
        acct.refresh_token.take(),
    );
    Ok(Some(acct))
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
    // Tokens to the keychain; only write the DB column if the keychain is unavailable.
    let db_access = (!crate::secrets::set(KC_ACCESS, access_token)).then_some(access_token);
    let db_refresh = (!crate::secrets::set(KC_REFRESH, refresh_token)).then_some(refresh_token);
    conn.execute(
        "INSERT INTO calendar_accounts(provider, email, calendar_id, access_token, refresh_token, token_expiry, sync_token, connected_at)
         VALUES('google', ?1, ?2, ?3, ?4, ?5, NULL, ?6)",
        params![email, calendar_id, db_access, db_refresh, token_expiry, now_iso()],
    )?;
    Ok(())
}

pub fn update_google_tokens(conn: &Connection, id: i64, access_token: &str, token_expiry: &str) -> Result<()> {
    let db_access = (!crate::secrets::set(KC_ACCESS, access_token)).then_some(access_token);
    conn.execute(
        "UPDATE calendar_accounts SET access_token = ?2, token_expiry = ?3 WHERE id = ?1",
        params![id, db_access, token_expiry],
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
    crate::secrets::clear(KC_ACCESS);
    crate::secrets::clear(KC_REFRESH);
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

// ---------- Habits ----------

/// CSV "1,3" ↔ weekday list. Empty/whitespace → empty vec.
fn parse_days_csv(s: &str) -> Vec<u8> {
    s.split(',').filter_map(|p| p.trim().parse::<u8>().ok()).filter(|d| (1..=7).contains(d)).collect()
}
fn days_to_csv(days: &[u8]) -> String {
    days.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(",")
}

fn row_to_habit(r: &Row) -> rusqlite::Result<Habit> {
    Ok(Habit {
        id: r.get("id")?,
        name: r.get("name")?,
        color: r.get("color")?,
        cadence: r.get("cadence")?,
        days: parse_days_csv(&r.get::<_, String>("days").unwrap_or_default()),
        interval_days: r.get::<_, i64>("interval_days").unwrap_or(1),
        duration_minutes: r.get("duration_minutes")?,
        archived: r.get::<_, i64>("archived")? != 0,
        created_at: r.get("created_at")?,
    })
}

pub fn list_habits(conn: &Connection) -> Result<Vec<Habit>> {
    let mut stmt = conn.prepare("SELECT * FROM habits WHERE archived = 0 ORDER BY created_at")?;
    let rows: Vec<Habit> = stmt.query_map([], row_to_habit)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_habit(conn: &Connection, name: &str, color: &str, cadence: &str, days: &[u8], interval_days: i64, duration_minutes: i64) -> Result<i64> {
    conn.execute(
        "INSERT INTO habits(name, color, cadence, days, interval_days, duration_minutes, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![name, color, cadence, days_to_csv(days), interval_days.max(1), duration_minutes, now_iso()],
    )?;
    Ok(conn.last_insert_rowid())
}

#[allow(clippy::too_many_arguments)]
pub fn update_habit(conn: &Connection, id: i64, name: &str, color: &str, cadence: &str, days: &[u8], interval_days: i64, duration_minutes: i64) -> Result<()> {
    conn.execute(
        "UPDATE habits SET name = ?2, color = ?3, cadence = ?4, days = ?5, interval_days = ?6, duration_minutes = ?7 WHERE id = ?1",
        params![id, name, color, cadence, days_to_csv(days), interval_days.max(1), duration_minutes],
    )?;
    Ok(())
}

pub fn delete_habit(conn: &Connection, id: i64) -> Result<()> {
    // habit_logs cascade via the FK (foreign_keys pragma is ON).
    conn.execute("DELETE FROM habits WHERE id = ?1", params![id])?;
    Ok(())
}

/// The calendar days a habit was completed (each "YYYY-MM-DD"), for stats.
pub fn done_days_for_habit(conn: &Connection, habit_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT day FROM habit_logs WHERE habit_id = ?1")?;
    let rows = stmt.query_map(params![habit_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Toggle a habit's completion for a day. Returns true if it's now done, false if cleared.
pub fn toggle_habit_log(conn: &Connection, habit_id: i64, day: &str) -> Result<bool> {
    let removed = conn.execute(
        "DELETE FROM habit_logs WHERE habit_id = ?1 AND day = ?2",
        params![habit_id, day],
    )?;
    if removed > 0 {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO habit_logs(habit_id, day) VALUES(?1, ?2)",
        params![habit_id, day],
    )?;
    Ok(true)
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

// ---------- Notes (Hermes memory layer) ----------

fn row_to_note(r: &Row, indexed: bool) -> rusqlite::Result<Note> {
    Ok(Note {
        id: r.get("id")?,
        content: r.get("content")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        indexed,
        score: None,
    })
}

/// All notes, newest first. Carries no embedding payload (the `indexed` flag tells the UI whether
/// one exists); use `notes_for_recall` when the vectors are actually needed.
pub fn list_notes(conn: &Connection) -> Result<Vec<Note>> {
    let mut stmt = conn.prepare(
        "SELECT id, content, created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_note(r, indexed)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn insert_note(conn: &Connection, content: &str, embedding: Option<&[u8]>, model: Option<&str>) -> Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO notes(content, embedding, embedding_model, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?4)",
        params![content, embedding, model, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_note(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
    Ok(())
}

/// Notes paired with their raw embedding bytes (None when not indexed), for recall ranking.
pub fn notes_for_recall(conn: &Connection) -> Result<Vec<(Note, Option<Vec<u8>>)>> {
    let mut stmt = conn.prepare("SELECT id, content, created_at, updated_at, embedding FROM notes")?;
    let rows = stmt.query_map([], |r| {
        let emb: Option<Vec<u8>> = r.get("embedding")?;
        let note = row_to_note(r, emb.is_some())?;
        Ok((note, emb))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- Pages (vault: the notes table viewed as Notion-style documents) ----------

/// A display title for a page: its explicit `title`, else the first non-empty line of the body
/// (truncated), else "Untitled". Keeps legacy Hermes notes (no title column) readable in the tree.
pub fn derive_title(title: &Option<String>, content: &str) -> String {
    if let Some(t) = title {
        if !t.trim().is_empty() {
            return t.trim().to_string();
        }
    }
    let first = content.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if first.is_empty() {
        return "Untitled".to_string();
    }
    let truncated: String = first.chars().take(80).collect();
    truncated
}

/// Map a row to a `Page`. When `with_body` is false the heavy `content`/`content_json` are left
/// empty (the sidebar tree and graph don't need them); the title is still derived from the body.
fn row_to_page(r: &Row, indexed: bool, with_body: bool) -> rusqlite::Result<Page> {
    let raw_title: Option<String> = r.get("title")?;
    let content: String = r.get("content")?;
    let title = derive_title(&raw_title, &content);
    Ok(Page {
        id: r.get("id")?,
        title,
        icon: r.get("icon")?,
        parent_id: r.get("parent_id")?,
        content_json: if with_body { r.get("content_json")? } else { None },
        content: if with_body { content } else { String::new() },
        sort_order: r.get("sort_order")?,
        archived: r.get::<_, i64>("archived")? != 0,
        daily_date: r.get("daily_date")?,
        inbox: r.get::<_, i64>("inbox")? != 0,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
        indexed,
        score: None,
    })
}

/// All non-archived pages (lightweight: no bodies), ordered for the sidebar tree.
pub fn list_pages(conn: &Connection) -> Result<Vec<Page>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, icon, parent_id, content, sort_order, archived, daily_date, inbox,
                created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes WHERE archived = 0 ORDER BY sort_order, created_at",
    )?;
    let rows = stmt.query_map([], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, false)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A single page with its full body.
pub fn get_page(conn: &Connection, id: i64) -> Result<Page> {
    let page = conn.query_row(
        "SELECT id, title, icon, parent_id, content, content_json, sort_order, archived, daily_date, inbox,
                created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes WHERE id = ?1",
        params![id],
        |r| {
            let indexed: bool = r.get::<_, i64>("indexed")? != 0;
            row_to_page(r, indexed, true)
        },
    )?;
    Ok(page)
}

/// Create a page. `sort_order` is placed after existing siblings under the same parent.
pub fn insert_page(
    conn: &Connection,
    title: &str,
    parent_id: Option<i64>,
    content: &str,
    content_json: Option<&str>,
    embedding: Option<&[u8]>,
    model: Option<&str>,
) -> Result<i64> {
    let now = now_iso();
    let next_order: f64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM notes WHERE parent_id IS ?1",
        params![parent_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO notes(content, title, parent_id, content_json, sort_order, embedding, embedding_model, created_at, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![content, title, parent_id, content_json, next_order, embedding, model, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Update a page's title/icon/body. The embedding is rewritten only when a fresh one is supplied
/// (a transient embed failure leaves the prior vector intact rather than wiping it).
pub fn update_page(
    conn: &Connection,
    id: i64,
    title: &str,
    icon: Option<&str>,
    content: &str,
    content_json: Option<&str>,
    embedding: Option<&[u8]>,
    model: Option<&str>,
) -> Result<()> {
    let now = now_iso();
    conn.execute(
        "UPDATE notes SET title = ?2, icon = ?3, content = ?4, content_json = ?5, updated_at = ?6 WHERE id = ?1",
        params![id, title, icon, content, content_json, now],
    )?;
    if let Some(emb) = embedding {
        conn.execute("UPDATE notes SET embedding = ?2, embedding_model = ?3 WHERE id = ?1", params![id, emb, model])?;
    }
    Ok(())
}

/// Reparent / reorder a page in the tree.
pub fn move_page(conn: &Connection, id: i64, parent_id: Option<i64>, sort_order: f64) -> Result<()> {
    conn.execute(
        "UPDATE notes SET parent_id = ?2, sort_order = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, parent_id, sort_order, now_iso()],
    )?;
    Ok(())
}

/// A (title → id) map over all pages, used to resolve wikilink targets by title (titles may be
/// derived, so resolution happens in Rust rather than SQL). Lowercased keys for case-insensitivity.
fn title_index(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT id, title, content FROM notes WHERE archived = 0")?;
    let rows = stmt.query_map([], |r| {
        let id: i64 = r.get("id")?;
        let title: Option<String> = r.get("title")?;
        let content: String = r.get("content")?;
        Ok((derive_title(&title, &content).to_lowercase(), id))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (k, v) = row?;
        map.entry(k).or_insert(v); // first (lowest sort) wins on duplicate titles
    }
    Ok(map)
}

/// Replace the outgoing wikilinks for `source_id`. Each link is resolved to a target page by title
/// when one exists; otherwise it's stored as a ghost (target_id NULL) to resolve later.
pub fn set_page_links(conn: &Connection, source_id: i64, target_titles: &[String]) -> Result<()> {
    let index = title_index(conn)?;
    conn.execute("DELETE FROM page_links WHERE source_id = ?1", params![source_id])?;
    for title in target_titles {
        let t = title.trim();
        if t.is_empty() {
            continue;
        }
        let target_id = index.get(&t.to_lowercase()).copied().filter(|id| *id != source_id);
        conn.execute(
            "INSERT OR IGNORE INTO page_links(source_id, target_id, target_title) VALUES(?1, ?2, ?3)",
            params![source_id, target_id, t],
        )?;
    }
    Ok(())
}

/// Pages that link TO `target_id` (the "Linked references" panel). Resolves ghost links by title so
/// references created before this page existed still show up.
pub fn page_backlinks(conn: &Connection, target_id: i64) -> Result<Vec<Page>> {
    let this = get_page(conn, target_id)?;
    let this_title = this.title.to_lowercase();
    let mut stmt = conn.prepare(
        "SELECT DISTINCT n.id, n.title, n.icon, n.parent_id, n.content, n.sort_order, n.archived,
                n.daily_date, n.inbox, n.created_at, n.updated_at, n.embedding IS NOT NULL AS indexed
         FROM page_links l JOIN notes n ON n.id = l.source_id
         WHERE n.archived = 0 AND (l.target_id = ?1 OR (l.target_id IS NULL AND lower(l.target_title) = ?2))
         ORDER BY n.updated_at DESC, n.id DESC",
    )?;
    let rows = stmt.query_map(params![target_id, this_title], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, false)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Pages that *mention* this page's title in their body but don't yet link it (Obsidian-style
/// "unlinked mentions" — a discovery surface). Match is case-insensitive substring via `instr`,
/// excluding the page itself and any page that already links here (resolved or by title).
pub fn unlinked_mentions(conn: &Connection, page_id: i64) -> Result<Vec<Page>> {
    let this = get_page(conn, page_id)?;
    let needle = this.title.trim().to_lowercase();
    if needle.len() < 3 {
        return Ok(vec![]); // too-short titles ("a") would match everything
    }
    let mut stmt = conn.prepare(
        "SELECT id, title, icon, parent_id, content, sort_order, archived, daily_date, inbox,
                created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes
         WHERE archived = 0 AND id != ?1 AND instr(lower(content), ?2) > 0
           AND id NOT IN (
               SELECT source_id FROM page_links
               WHERE target_id = ?1 OR (target_id IS NULL AND lower(target_title) = ?3)
           )
         ORDER BY updated_at DESC, id DESC LIMIT 20",
    )?;
    let rows = stmt.query_map(params![page_id, needle, this.title.to_lowercase()], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, false)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Free-text search over page titles + bodies (lightweight rows, no body returned).
pub fn search_pages(conn: &Connection, query: &str) -> Result<Vec<Page>> {
    let like = format!("%{}%", query.trim());
    let mut stmt = conn.prepare(
        "SELECT id, title, icon, parent_id, content, sort_order, archived, daily_date, inbox,
                created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes WHERE archived = 0 AND (title LIKE ?1 OR content LIKE ?1)
         ORDER BY updated_at DESC, id DESC LIMIT 50",
    )?;
    let rows = stmt.query_map(params![like], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, false)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- Daily notes (a page that IS a calendar day) ----------

/// The page for `date` ('YYYY-MM-DD'), creating it (titled `title`) on first access. Idempotent.
pub fn get_or_create_daily(conn: &Connection, date: &str, title: &str) -> Result<Page> {
    let existing: rusqlite::Result<i64> =
        conn.query_row("SELECT id FROM notes WHERE daily_date = ?1", params![date], |r| r.get(0));
    let id = match existing {
        Ok(id) => id,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let now = now_iso();
            conn.execute(
                "INSERT INTO notes(content, title, daily_date, sort_order, created_at, updated_at)
                 VALUES('', ?1, ?2, 0, ?3, ?3)",
                params![title, date, now],
            )?;
            conn.last_insert_rowid()
        }
        Err(e) => return Err(e.into()),
    };
    get_page(conn, id)
}

// ---------- Inbox (one-box quick capture) ----------

/// Save a quick capture into the Inbox (a page flagged `inbox=1`, title derived from the text).
pub fn capture(conn: &Connection, content: &str, embedding: Option<&[u8]>, model: Option<&str>) -> Result<i64> {
    let now = now_iso();
    conn.execute(
        "INSERT INTO notes(content, inbox, sort_order, embedding, embedding_model, created_at, updated_at)
         VALUES(?1, 1, 0, ?2, ?3, ?4, ?4)",
        params![content, embedding, model, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The unsorted Inbox, newest first.
pub fn list_inbox(conn: &Connection) -> Result<Vec<Page>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, icon, parent_id, content, content_json, sort_order, archived, daily_date, inbox,
                created_at, updated_at, embedding IS NOT NULL AS indexed
         FROM notes WHERE inbox = 1 ORDER BY created_at DESC, id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, true)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Clear the inbox flag — the capture graduates into a normal vault page.
pub fn clear_inbox(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE notes SET inbox = 0, updated_at = ?2 WHERE id = ?1", params![id, now_iso()])?;
    Ok(())
}

// ---------- Entity links (page ↔ task/event) ----------

/// Link a page to a task or event (idempotent).
pub fn link_entity(conn: &Connection, page_id: i64, kind: &str, entity_id: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO entity_links(page_id, entity_kind, entity_id) VALUES(?1, ?2, ?3)",
        params![page_id, kind, entity_id],
    )?;
    Ok(())
}

/// Remove a page↔entity link.
pub fn unlink_entity(conn: &Connection, page_id: i64, kind: &str, entity_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM entity_links WHERE page_id = ?1 AND entity_kind = ?2 AND entity_id = ?3",
        params![page_id, kind, entity_id],
    )?;
    Ok(())
}

/// The tasks/events a page references (for the editor's "Linked tasks & events" strip).
pub fn page_entities(conn: &Connection, page_id: i64) -> Result<Vec<EntityRef>> {
    let mut stmt = conn.prepare("SELECT entity_kind, entity_id FROM entity_links WHERE page_id = ?1")?;
    let rows = stmt.query_map(params![page_id], |r| Ok(EntityRef { kind: r.get(0)?, id: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The (non-archived) pages that reference a given task/event (for a "Notes" affordance on it).
pub fn entity_pages(conn: &Connection, kind: &str, entity_id: i64) -> Result<Vec<Page>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.title, n.icon, n.parent_id, n.content, n.sort_order, n.archived, n.daily_date,
                n.inbox, n.created_at, n.updated_at, n.embedding IS NOT NULL AS indexed
         FROM entity_links l JOIN notes n ON n.id = l.page_id
         WHERE n.archived = 0 AND l.entity_kind = ?1 AND l.entity_id = ?2
         ORDER BY n.updated_at DESC, n.id DESC",
    )?;
    let rows = stmt.query_map(params![kind, entity_id], |r| {
        let indexed: bool = r.get::<_, i64>("indexed")? != 0;
        row_to_page(r, indexed, false)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The whole vault graph: every non-archived page as a node, every resolved wikilink as an edge.
/// Ghost links (target_id NULL) are resolved by title here so the graph reflects current pages.
pub fn page_graph(conn: &Connection) -> Result<PageGraph> {
    let index = title_index(conn)?;
    let pages = list_pages(conn)?;
    let valid: std::collections::HashSet<i64> = pages.iter().map(|p| p.id).collect();

    // Collect distinct, resolved, self-loop-free directed edges.
    let mut edge_set: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();
    let mut stmt = conn.prepare("SELECT source_id, target_id, target_title FROM page_links")?;
    let rows = stmt.query_map([], |r| {
        let source: i64 = r.get("source_id")?;
        let target_id: Option<i64> = r.get("target_id")?;
        let target_title: String = r.get("target_title")?;
        Ok((source, target_id, target_title))
    })?;
    for row in rows {
        let (source, target_id, target_title) = row?;
        let target = target_id.or_else(|| index.get(&target_title.to_lowercase()).copied());
        if let Some(target) = target {
            if source != target && valid.contains(&source) && valid.contains(&target) {
                edge_set.insert((source, target));
            }
        }
    }

    // Node degree = number of incident edges (in + out).
    let mut degree: std::collections::HashMap<i64, u32> = std::collections::HashMap::new();
    for (s, t) in &edge_set {
        *degree.entry(*s).or_insert(0) += 1;
        *degree.entry(*t).or_insert(0) += 1;
    }
    let nodes = pages
        .iter()
        .map(|p| GraphNode {
            id: p.id,
            title: p.title.clone(),
            parent_id: p.parent_id,
            degree: degree.get(&p.id).copied().unwrap_or(0),
        })
        .collect();
    let edges = edge_set.into_iter().map(|(source, target)| GraphEdge { source, target }).collect();
    Ok(PageGraph { nodes, edges })
}

/// A fresh in-memory, fully-migrated connection for tests across the crate (booking, commands, …).
#[cfg(test)]
pub(crate) fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    migrate(&conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        super::test_conn()
    }

    #[test]
    fn daily_note_is_idempotent() {
        let conn = mem();
        let a = get_or_create_daily(&conn, "2026-06-14", "Sat").unwrap();
        let b = get_or_create_daily(&conn, "2026-06-14", "Sat").unwrap();
        assert_eq!(a.id, b.id, "same date returns the same page");
        assert_eq!(a.daily_date.as_deref(), Some("2026-06-14"));
        // A different date is a distinct page; daily pages still show up in the tree listing.
        let c = get_or_create_daily(&conn, "2026-06-15", "Sun").unwrap();
        assert_ne!(a.id, c.id);
        assert_eq!(list_pages(&conn).unwrap().len(), 2);
    }

    #[test]
    fn entity_links_round_trip_and_cascade() {
        let conn = mem();
        let page = insert_page(&conn, "Project notes", None, "", None, None, None).unwrap();
        link_entity(&conn, page, "task", 42).unwrap();
        link_entity(&conn, page, "event", 7).unwrap();
        link_entity(&conn, page, "task", 42).unwrap(); // idempotent

        let refs = page_entities(&conn, page).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(entity_pages(&conn, "task", 42).unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![page]);
        assert!(entity_pages(&conn, "task", 99).unwrap().is_empty());

        unlink_entity(&conn, page, "event", 7).unwrap();
        assert_eq!(page_entities(&conn, page).unwrap().len(), 1);

        // Deleting the page cascades its entity links away.
        delete_note(&conn, page).unwrap();
        assert!(entity_pages(&conn, "task", 42).unwrap().is_empty());
    }

    #[test]
    fn title_derivation() {
        // Explicit title wins (and is trimmed).
        assert_eq!(derive_title(&Some("  Roadmap  ".into()), "body"), "Roadmap");
        // Falls back to the first non-empty body line for legacy notes.
        assert_eq!(derive_title(&None, "\n\nFirst line\nsecond"), "First line");
        assert_eq!(derive_title(&Some("".into()), "Derived"), "Derived");
        // Empty everything → Untitled.
        assert_eq!(derive_title(&None, "   "), "Untitled");
        // Long first lines are truncated to 80 chars.
        let long = "x".repeat(200);
        assert_eq!(derive_title(&None, &long).chars().count(), 80);
    }

    #[test]
    fn links_resolve_into_graph_and_backlinks() {
        let conn = mem();
        let a = insert_page(&conn, "Alpha", None, "", None, None, None).unwrap();
        let b = insert_page(&conn, "Beta", None, "", None, None, None).unwrap();

        // Alpha links to Beta (by title) and to a not-yet-existing "Gamma" (ghost).
        set_page_links(&conn, a, &["Beta".into(), "Gamma".into()]).unwrap();

        let g = page_graph(&conn).unwrap();
        assert_eq!(g.nodes.len(), 2, "ghost targets are not nodes");
        assert_eq!(g.edges, vec![GraphEdge { source: a, target: b }]);

        // Beta's backlinks include Alpha; degree reflects the single resolved edge.
        let back = page_backlinks(&conn, b).unwrap();
        assert_eq!(back.iter().map(|p| p.id).collect::<Vec<_>>(), vec![a]);

        // Create Gamma → the ghost link resolves at read time without rewriting page_links.
        let c = insert_page(&conn, "Gamma", None, "", None, None, None).unwrap();
        let g2 = page_graph(&conn).unwrap();
        assert!(g2.edges.contains(&GraphEdge { source: a, target: c }));
        assert_eq!(g2.edges.len(), 2);
        assert_eq!(page_backlinks(&conn, c).unwrap().len(), 1);
    }

    #[test]
    fn unlinked_mentions_finds_mentions_without_links() {
        let conn = mem();
        let target = insert_page(&conn, "Budget", None, "", None, None, None).unwrap();
        let mentions_it = insert_page(&conn, "Meeting", None, "Discussed the Budget at length", None, None, None).unwrap();
        let links_it = insert_page(&conn, "Plan", None, "see Budget", None, None, None).unwrap();
        set_page_links(&conn, links_it, &["Budget".into()]).unwrap();
        insert_page(&conn, "Unrelated", None, "nothing here", None, None, None).unwrap();

        let found = unlinked_mentions(&conn, target).unwrap();
        let ids: Vec<i64> = found.iter().map(|p| p.id).collect();
        assert!(ids.contains(&mentions_it), "page that mentions but doesn't link shows up");
        assert!(!ids.contains(&links_it), "already-linked page is excluded");
        assert!(!ids.contains(&target), "the page itself is excluded");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn self_links_are_ignored() {
        let conn = mem();
        let a = insert_page(&conn, "Solo", None, "", None, None, None).unwrap();
        set_page_links(&conn, a, &["Solo".into()]).unwrap();
        assert!(page_graph(&conn).unwrap().edges.is_empty());
    }

    #[test]
    fn delete_cascades_links_and_orphans_children() {
        let conn = mem();
        let parent = insert_page(&conn, "Parent", None, "", None, None, None).unwrap();
        let child = insert_page(&conn, "Child", Some(parent), "", None, None, None).unwrap();
        set_page_links(&conn, child, &["Parent".into()]).unwrap();

        delete_note(&conn, parent).unwrap();
        // Child survives, re-parented to root (ON DELETE SET NULL).
        let pages = list_pages(&conn).unwrap();
        let surviving = pages.iter().find(|p| p.id == child).expect("child kept");
        assert_eq!(surviving.parent_id, None);
        // The link to the deleted parent cascaded away.
        assert!(page_graph(&conn).unwrap().edges.is_empty());
    }

    // ============================ Pressure tests (new features) ============================

    fn p(conn: &Connection, title: &str, parent: Option<i64>, body: &str) -> i64 {
        insert_page(conn, title, parent, body, None, None, None).unwrap()
    }

    // ---- Daily notes ----

    #[test]
    fn daily_note_survives_edits_and_is_flagged() {
        let conn = mem();
        let d = get_or_create_daily(&conn, "2026-06-14", "Sat").unwrap();
        assert!(d.daily_date.is_some() && !d.inbox);
        // Editing the daily note must not lose its daily_date (re-fetch by date returns same id).
        update_page(&conn, d.id, "Sat", None, "did stuff", Some("[]"), None, None).unwrap();
        let again = get_or_create_daily(&conn, "2026-06-14", "Sat").unwrap();
        assert_eq!(again.id, d.id);
        assert_eq!(again.content, "did stuff");
        assert_eq!(again.daily_date.as_deref(), Some("2026-06-14"));
    }

    // ---- Entity links ----

    #[test]
    fn entity_links_distinguish_kind_and_order_by_recency() {
        let conn = mem();
        let older = p(&conn, "Older", None, "");
        let newer = p(&conn, "Newer", None, "");
        // task 5 and event 5 are different entities.
        link_entity(&conn, older, "task", 5).unwrap();
        link_entity(&conn, newer, "event", 5).unwrap();
        assert_eq!(entity_pages(&conn, "task", 5).unwrap().len(), 1);
        assert_eq!(entity_pages(&conn, "event", 5).unwrap().len(), 1);

        // Two pages linking the same task come back newest-updated first.
        link_entity(&conn, older, "task", 9).unwrap();
        update_page(&conn, newer, "Newer", None, "touch", None, None, None).unwrap();
        link_entity(&conn, newer, "task", 9).unwrap();
        let pages = entity_pages(&conn, "task", 9).unwrap();
        assert_eq!(pages.iter().map(|p| p.id).collect::<Vec<_>>(), vec![newer, older]);

        // Unlinking something that isn't linked is a no-op (no error, no change).
        unlink_entity(&conn, older, "event", 123).unwrap();
        assert_eq!(page_entities(&conn, older).unwrap().len(), 2);
    }

    // ---- Inbox / quick capture ----

    #[test]
    fn inbox_capture_list_and_graduate() {
        let conn = mem();
        let a = capture(&conn, "first thought", None, None).unwrap();
        let _b = capture(&conn, "second thought", None, None).unwrap();
        let inbox = list_inbox(&conn).unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].content, "second thought", "newest first");
        assert!(inbox.iter().all(|p| p.inbox));

        // Graduating clears the flag; the page persists and leaves the inbox.
        clear_inbox(&conn, a).unwrap();
        let inbox = list_inbox(&conn).unwrap();
        assert_eq!(inbox.len(), 1);
        let kept = get_page(&conn, a).unwrap();
        assert!(!kept.inbox);
        assert_eq!(kept.content, "first thought");
    }

    // ---- Page links / graph ----

    #[test]
    fn graph_dedupes_links_and_counts_degree() {
        let conn = mem();
        let a = p(&conn, "A", None, "");
        let b = p(&conn, "B", None, "");
        let c = p(&conn, "C", None, "");
        // A → B (with a duplicate title that must collapse), A → C, C → A (bidirectional with A).
        set_page_links(&conn, a, &["B".into(), "b".into(), "C".into()]).unwrap();
        set_page_links(&conn, c, &["A".into()]).unwrap();

        let g = page_graph(&conn).unwrap();
        assert_eq!(g.edges.len(), 3, "A→B, A→C, C→A — the duplicate B collapses");
        let deg = |id: i64| g.nodes.iter().find(|n| n.id == id).unwrap().degree;
        assert_eq!(deg(a), 3); // A→B, A→C, C→A all touch A
        assert_eq!(deg(b), 1);
        assert_eq!(deg(c), 2);
    }

    #[test]
    fn set_page_links_replaces_previous_set() {
        let conn = mem();
        let a = p(&conn, "A", None, "");
        p(&conn, "B", None, "");
        let c = p(&conn, "C", None, "");
        set_page_links(&conn, a, &["B".into()]).unwrap();
        set_page_links(&conn, a, &["C".into()]).unwrap(); // replaces, not appends
        let g = page_graph(&conn).unwrap();
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].target, c, "links now point only to C");
    }

    #[test]
    fn archived_pages_drop_out_of_listings_and_graph() {
        let conn = mem();
        let a = p(&conn, "Alive", None, "mentions Ghost");
        let ghost = p(&conn, "Ghost", None, "");
        set_page_links(&conn, a, &["Ghost".into()]).unwrap();
        conn.execute("UPDATE notes SET archived = 1 WHERE id = ?1", params![ghost]).unwrap();

        assert!(list_pages(&conn).unwrap().iter().all(|p| p.id != ghost), "archived hidden from tree");
        assert!(search_pages(&conn, "Ghost").unwrap().iter().all(|p| p.id != ghost), "archived hidden from search");
        // The edge to an archived node is dropped from the graph.
        assert!(page_graph(&conn).unwrap().edges.is_empty());
    }

    // ---- Unlinked mentions ----

    #[test]
    fn unlinked_mentions_case_insensitive_short_title_and_ghost_excluded() {
        let conn = mem();
        let budget = p(&conn, "Budget", None, "");
        let m1 = p(&conn, "Notes", None, "we cut the BUDGET hard"); // case-insensitive match
        let ghostlinker = p(&conn, "Plan", None, "talk to budget team");
        // Plan links Budget by title even though it also mentions it → excluded as already-linked.
        set_page_links(&conn, ghostlinker, &["Budget".into()]).unwrap();

        let found = unlinked_mentions(&conn, budget).unwrap();
        let ids: Vec<i64> = found.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![m1], "only the genuinely-unlinked mention");

        // A title under 3 chars matches too much → guarded to empty.
        let short = p(&conn, "Q", None, "");
        p(&conn, "X", None, "Q1 results and Quarterly stuff");
        assert!(unlinked_mentions(&conn, short).unwrap().is_empty());
    }

    // ---- Search ----

    #[test]
    fn search_matches_title_and_body_case_insensitively() {
        let conn = mem();
        let t = p(&conn, "Quarterly Review", None, "nothing special");
        let b = p(&conn, "Random", None, "the QUARTERLY numbers");
        let hits: Vec<i64> = search_pages(&conn, "quarterly").unwrap().iter().map(|p| p.id).collect();
        assert!(hits.contains(&t) && hits.contains(&b));
        assert!(search_pages(&conn, "zebra").unwrap().is_empty());
    }

    // ---- Reparent / move ----

    #[test]
    fn move_page_reparents_and_reorders() {
        let conn = mem();
        let a = p(&conn, "A", None, "");
        let b = p(&conn, "B", None, "");
        move_page(&conn, b, Some(a), 2.5).unwrap();
        let moved = get_page(&conn, b).unwrap();
        assert_eq!(moved.parent_id, Some(a));
        assert_eq!(moved.sort_order, 2.5);
        // Back to root.
        move_page(&conn, b, None, 0.0).unwrap();
        assert_eq!(get_page(&conn, b).unwrap().parent_id, None);
    }

    // ---- content_json round-trip ----

    #[test]
    fn content_json_round_trips_through_get_page() {
        let conn = mem();
        let id = insert_page(&conn, "Doc", None, "plain text", Some("[{\"type\":\"paragraph\"}]"), None, None).unwrap();
        let got = get_page(&conn, id).unwrap();
        assert_eq!(got.content_json.as_deref(), Some("[{\"type\":\"paragraph\"}]"));
        // list_pages is lightweight: no body shipped.
        let listed = list_pages(&conn).unwrap().into_iter().find(|p| p.id == id).unwrap();
        assert_eq!(listed.content, "");
        assert!(listed.content_json.is_none());
    }
}
