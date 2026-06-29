//! SQLite persistence, owned entirely by the Rust core (rusqlite).
//! The frontend never touches the DB directly — it goes through Tauri commands.

use crate::model::*;
use crate::scheduler::SchedulePref;
use anyhow::Result;
use chrono::NaiveTime;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

const MIGRATION_0001: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_google.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_habits.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_habit_duration.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_project_archive.sql");
const MIGRATION_0006: &str = include_str!("../migrations/0006_notes.sql");
const MIGRATION_0007: &str = include_str!("../migrations/0007_habit_cadence.sql");
const MIGRATION_0008: &str = include_str!("../migrations/0008_pages.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_brain.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_labels.sql");
const MIGRATION_0011: &str = include_str!("../migrations/0011_booking_public.sql");
const MIGRATION_0012: &str = include_str!("../migrations/0012_context_index.sql");
const MIGRATION_0013: &str = include_str!("../migrations/0013_people.sql");
const MIGRATION_0014: &str = include_str!("../migrations/0014_focus_sessions.sql");
const MIGRATION_0016: &str = include_str!("../migrations/0016_vault_files.sql");

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Register sync's change-capture function before migrating: the 0015 triggers reference it.
    crate::sync::register_functions(&conn)?;
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
    if version < 10 {
        conn.execute_batch(MIGRATION_0010)?;
        conn.pragma_update(None, "user_version", 10)?;
    }
    if version < 11 {
        conn.execute_batch(MIGRATION_0011)?;
        conn.pragma_update(None, "user_version", 11)?;
    }
    if version < 12 {
        conn.execute_batch(MIGRATION_0012)?;
        conn.pragma_update(None, "user_version", 12)?;
    }
    if version < 13 {
        conn.execute_batch(MIGRATION_0013)?;
        conn.pragma_update(None, "user_version", 13)?;
    }
    if version < 14 {
        conn.execute_batch(MIGRATION_0014)?;
        conn.pragma_update(None, "user_version", 14)?;
    }
    if version < 15 {
        // Device-sync substrate: uuid/updated_hlc/dirty columns + change-capture triggers +
        // tombstones/peers/self tables. Generated from the synced-table registry (sync::schema).
        conn.execute_batch(&crate::sync::schema::migration_sql())?;
        conn.pragma_update(None, "user_version", 15)?;
    }
    if version < 16 {
        // Two-way markdown vault: notes.rel_path (page → file mapping).
        conn.execute_batch(MIGRATION_0016)?;
        conn.pragma_update(None, "user_version", 16)?;
    }
    ensure_booking_public_fields(conn)?;
    Ok(())
}

fn now_iso() -> String {
    chrono::Local::now().naive_local().format(DT_FMT).to_string()
}

fn slugify_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "event".into() } else { out }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("could not generate booking token: {e}"))?;
    Ok(hex::encode(bytes))
}

fn ensure_booking_public_fields(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, String, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, name, slug, share_token FROM event_types")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows
    };
    for (id, name, slug, token) in rows {
        let next_slug = if slug.trim().is_empty() {
            format!("{}-{id}", slugify_name(&name))
        } else {
            slug
        };
        let next_token = if token.trim().is_empty() { random_token()? } else { token };
        conn.execute(
            "UPDATE event_types SET slug = ?1, share_token = ?2 WHERE id = ?3",
            params![next_slug, next_token, id],
        )?;
    }
    Ok(())
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

/// Record the on-disk file a vault page maps to (two-way markdown vault). `None` clears it.
pub fn set_page_rel_path(conn: &Connection, page_id: i64, rel_path: Option<&str>) -> Result<()> {
    conn.execute("UPDATE notes SET rel_path = ?2 WHERE id = ?1", params![page_id, rel_path])?;
    Ok(())
}

/// Clear the file mapping for whatever page points at `rel_path` (its file was deleted externally).
/// The page itself survives — an external `rm` shouldn't destroy the note.
pub fn unlink_rel_path(conn: &Connection, rel_path: &str) -> Result<()> {
    conn.execute("UPDATE notes SET rel_path = NULL WHERE rel_path = ?1", params![rel_path])?;
    Ok(())
}

/// The page currently mapped to `rel_path`, if any (the file→page lookup for the watcher).
#[allow(dead_code)] // used by the Phase 3e files→DB watcher
pub fn page_id_for_rel_path(conn: &Connection, rel_path: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT id FROM notes WHERE rel_path = ?1 LIMIT 1", params![rel_path], |r| r.get(0))
        .optional()?)
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
        slug: r.get("slug")?,
        share_token: r.get("share_token")?,
        enabled: r.get::<_, i64>("enabled")? != 0,
    })
}

pub fn list_event_types(conn: &Connection) -> Result<Vec<EventType>> {
    let mut stmt = conn.prepare("SELECT * FROM event_types ORDER BY id")?;
    let rows: Vec<EventType> = stmt.query_map([], row_to_event_type)?.collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn insert_event_type(conn: &Connection, name: &str, duration: i64, buffer: i64, color: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO event_types(name, duration_minutes, buffer_minutes, color, slug, share_token, enabled)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1)",
        params![name, duration, buffer, color, slugify_name(name), random_token()?],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute("UPDATE event_types SET slug = ?1 WHERE id = ?2", params![format!("{}-{id}", slugify_name(name)), id])?;
    Ok(id)
}

pub fn update_event_type(
    conn: &Connection,
    id: i64,
    name: &str,
    duration: i64,
    buffer: i64,
    color: &str,
    enabled: bool,
) -> Result<EventType> {
    conn.execute(
        "UPDATE event_types
         SET name = ?1, duration_minutes = ?2, buffer_minutes = ?3, color = ?4, slug = ?5, enabled = ?6
         WHERE id = ?7",
        params![name, duration, buffer, color, format!("{}-{id}", slugify_name(name)), if enabled { 1 } else { 0 }, id],
    )?;
    get_event_type(conn, id)
}

pub fn get_event_type(conn: &Connection, id: i64) -> Result<EventType> {
    let mut stmt = conn.prepare("SELECT * FROM event_types WHERE id = ?1")?;
    Ok(stmt.query_row(params![id], row_to_event_type)?)
}

pub fn public_event_type(conn: &Connection, token: &str, slug: &str) -> Result<Option<EventType>> {
    let mut stmt = conn.prepare("SELECT * FROM event_types WHERE share_token = ?1 AND slug = ?2 AND enabled = 1 LIMIT 1")?;
    let mut rows = stmt.query_map(params![token, slug], row_to_event_type)?;
    Ok(rows.next().transpose()?)
}

pub fn regenerate_event_type_token(conn: &Connection, id: i64) -> Result<EventType> {
    conn.execute("UPDATE event_types SET share_token = ?1 WHERE id = ?2", params![random_token()?, id])?;
    get_event_type(conn, id)
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
                event_id: r.get("event_id")?,
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
    event_title: &str,
    name: &str,
    email: &str,
    start: &str,
    end: &str,
) -> Result<i64> {
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO events(title, start, end, kind, source, created_at)
         VALUES(?1, ?2, ?3, 'fixed', 'manual', ?4)",
        params![event_title, start, end, now_iso()],
    )?;
    let event_id = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO bookings(event_type_id, event_id, invitee_name, invitee_email, start, end, status, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'confirmed', ?7)",
        params![event_type_id, event_id, name, email, start, end, now_iso()],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

pub fn cancel_booking(conn: &mut Connection, id: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let event_id: Option<i64> = tx.query_row("SELECT event_id FROM bookings WHERE id = ?1", params![id], |r| r.get(0))?;
    tx.execute("UPDATE bookings SET status = 'cancelled' WHERE id = ?1", params![id])?;
    if let Some(event_id) = event_id {
        tx.execute("DELETE FROM events WHERE id = ?1", params![event_id])?;
    }
    tx.commit()?;
    Ok(())
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

// ---------- Context Engine: the cross-entity recall index (entity_index) ----------

/// Insert or replace the recall-index row for one entity. `embedding` is None when the embed backend
/// is unavailable — the row is still keyword-searchable. `text_hash` lets the reindexer skip
/// unchanged rows; `embedding_model` lets it reindex when the model (and thus vector dims) changes.
pub fn upsert_entity_index(
    conn: &Connection,
    kind: EntityKind,
    entity_id: i64,
    text: &str,
    text_hash: &str,
    embedding: Option<&[u8]>,
    embedding_model: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO entity_index(entity_kind, entity_id, text, text_hash, embedding, embedding_model, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(entity_kind, entity_id) DO UPDATE SET
           text = excluded.text,
           text_hash = excluded.text_hash,
           embedding = excluded.embedding,
           embedding_model = excluded.embedding_model,
           updated_at = excluded.updated_at",
        params![kind.as_str(), entity_id, text, text_hash, embedding, embedding_model, now_iso()],
    )?;
    Ok(())
}

/// Drop an entity's index row (e.g. when the entity is deleted).
pub fn delete_entity_index(conn: &Connection, kind: EntityKind, entity_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM entity_index WHERE entity_kind = ?1 AND entity_id = ?2",
        params![kind.as_str(), entity_id],
    )?;
    Ok(())
}

/// Candidates for cross-entity recall, each carrying its raw embedding bytes (None = not indexed).
/// Empty `kinds` returns every indexed entity; otherwise only the requested kinds.
pub fn entity_index_for_recall(conn: &Connection, kinds: &[EntityKind]) -> Result<Vec<ContextItem>> {
    let mut sql = "SELECT entity_kind, entity_id, text, embedding FROM entity_index".to_string();
    if !kinds.is_empty() {
        let placeholders = kinds.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        sql.push_str(&format!(" WHERE entity_kind IN ({placeholders})"));
    }
    let mut stmt = conn.prepare(&sql)?;
    let kind_strs: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
    let rows = stmt.query_map(params_from_iter(kind_strs.iter()), |r| {
        Ok((
            r.get::<_, String>("entity_kind")?,
            r.get::<_, i64>("entity_id")?,
            r.get::<_, String>("text")?,
            r.get::<_, Option<Vec<u8>>>("embedding")?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (kind_s, id, text, embedding) = row?;
        if let Some(kind) = EntityKind::from_str(&kind_s) {
            out.push(ContextItem { kind, id, text, score: None, embedding });
        }
    }
    Ok(out)
}

/// Map of `entity_id` → stored `text_hash` for a kind, so the reindexer can skip unchanged rows.
pub fn entity_index_hashes(conn: &Connection, kind: EntityKind) -> Result<std::collections::HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT entity_id, text_hash FROM entity_index WHERE entity_kind = ?1")?;
    let rows = stmt.query_map(params![kind.as_str()], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Stored index state per entity of a kind (hash + whether it has a vector + which model produced it),
/// for the reindex pipeline's skip/re-embed decision.
pub fn entity_index_meta(conn: &Connection, kind: EntityKind) -> Result<std::collections::HashMap<i64, crate::context::IndexState>> {
    let mut stmt = conn.prepare(
        "SELECT entity_id, text_hash, embedding IS NOT NULL AS has_emb, embedding_model FROM entity_index WHERE entity_kind = ?1",
    )?;
    let rows = stmt.query_map(params![kind.as_str()], |r| {
        Ok((
            r.get::<_, i64>("entity_id")?,
            crate::context::IndexState {
                text_hash: r.get::<_, String>("text_hash")?,
                has_embedding: r.get::<_, i64>("has_emb")? != 0,
                model: r.get::<_, Option<String>>("embedding_model")?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Project every indexable entity (tasks, events, pages) to its embeddable `ContextItem` — the input
/// to the reindex sweep. Blank-text entities are skipped. `embedding`/`score` are left unset here.
pub fn entities_for_index(conn: &Connection) -> Result<Vec<ContextItem>> {
    let mut out = Vec::new();
    let mut push = |kind: EntityKind, id: i64, text: String| {
        if !text.trim().is_empty() {
            out.push(ContextItem { kind, id, text, score: None, embedding: None });
        }
    };
    for t in list_tasks(conn)? {
        push(EntityKind::Task, t.id, crate::context::task_text(&t));
    }
    for e in list_events(conn)? {
        push(EntityKind::Event, e.id, crate::context::event_text(&e));
    }
    // Pages live in `notes`; `list_pages` strips bodies, so read title+content directly.
    let pages: Vec<(i64, Option<String>, String)> = {
        let mut stmt = conn.prepare("SELECT id, title, content FROM notes WHERE archived = 0")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>("id")?, r.get::<_, Option<String>>("title")?, r.get::<_, String>("content")?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (id, title, content) in pages {
        // Only index pages with real body content. A title-only page (e.g. a blank daily note) has
        // nothing to recall on — embedding its bare title/date is pure noise.
        if content.trim().is_empty() {
            continue;
        }
        push(EntityKind::Page, id, crate::context::page_text(title.as_deref().unwrap_or(""), &content));
    }
    for p in list_people(conn)? {
        push(EntityKind::Person, p.id, crate::context::person_text(&p.name, &p.notes));
    }
    Ok(out)
}

/// 1-hop neighbors of an entity in the unified graph: page↔task/event via `entity_links`, and
/// page→page via `page_links` (both directions). Used by the Context Engine to expand recall hits
/// with their structurally-related entities. Returns possibly-duplicate `(kind, id)` pairs.
pub fn entity_neighbors(conn: &Connection, kind: EntityKind, id: i64) -> Result<Vec<(EntityKind, i64)>> {
    let mut out = Vec::new();
    match kind {
        EntityKind::Page => {
            let mut linked = conn.prepare("SELECT entity_kind, entity_id FROM entity_links WHERE page_id = ?1")?;
            for row in linked.query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (k, eid) = row?;
                if let Some(k) = EntityKind::from_str(&k) {
                    out.push((k, eid));
                }
            }
            let mut outgoing = conn.prepare("SELECT target_id FROM page_links WHERE source_id = ?1 AND target_id IS NOT NULL")?;
            for row in outgoing.query_map(params![id], |r| r.get::<_, i64>(0))? {
                out.push((EntityKind::Page, row?));
            }
            let mut incoming = conn.prepare("SELECT source_id FROM page_links WHERE target_id = ?1")?;
            for row in incoming.query_map(params![id], |r| r.get::<_, i64>(0))? {
                out.push((EntityKind::Page, row?));
            }
        }
        EntityKind::Task | EntityKind::Event => {
            let mut pages = conn.prepare("SELECT page_id FROM entity_links WHERE entity_kind = ?1 AND entity_id = ?2")?;
            for row in pages.query_map(params![kind.as_str(), id], |r| r.get::<_, i64>(0))? {
                out.push((EntityKind::Page, row?));
            }
        }
        _ => {}
    }
    Ok(out)
}

/// The most recently created tasks and events as `ContextItem`s — a low-priority recency tail for
/// assembled context (helps when recall is thin). Text mirrors the index projections.
pub fn recent_entities(conn: &Connection, limit: usize) -> Result<Vec<ContextItem>> {
    let mut out = Vec::new();
    let mut tasks = conn.prepare("SELECT id, title, notes FROM tasks ORDER BY created_at DESC, id DESC LIMIT ?1")?;
    for row in tasks.query_map(params![limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (id, title, notes) = row?;
        let text = if notes.trim().is_empty() { title.trim().to_string() } else { format!("{}\n{}", title.trim(), notes.trim()) };
        if !text.is_empty() {
            out.push(ContextItem { kind: EntityKind::Task, id, text, score: None, embedding: None });
        }
    }
    let mut events = conn.prepare("SELECT id, title FROM events ORDER BY created_at DESC, id DESC LIMIT ?1")?;
    for row in events.query_map(params![limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, title) = row?;
        let title = title.trim().to_string();
        if !title.is_empty() {
            out.push(ContextItem { kind: EntityKind::Event, id, text: title, score: None, embedding: None });
        }
    }
    Ok(out)
}

// ---------- People (the relationship layer / private CRM) ----------

fn row_to_person(r: &Row) -> rusqlite::Result<Person> {
    Ok(Person {
        id: r.get("id")?,
        name: r.get("name")?,
        email: r.get("email")?,
        notes: r.get("notes")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

pub fn insert_person(conn: &Connection, name: &str, email: Option<&str>, notes: &str) -> Result<i64> {
    let now = now_iso();
    let email = email.map(str::trim).filter(|e| !e.is_empty());
    conn.execute(
        "INSERT INTO people(name, email, notes, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, ?4)",
        params![name.trim(), email, notes, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Create or fetch a person by email (the dedupe key). When a row already exists for the email, its
/// id is returned (and a previously-blank name is filled in) rather than inserting a duplicate. With
/// no email we always insert (no reliable identity to dedupe on). Used to auto-create people from
/// booking invitees.
pub fn upsert_person_by_email(conn: &Connection, name: &str, email: Option<&str>) -> Result<i64> {
    let email = email.map(str::trim).filter(|e| !e.is_empty());
    if let Some(email) = email {
        let existing: Option<(i64, String)> = conn
            .query_row("SELECT id, name FROM people WHERE email = ?1", params![email], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((id, existing_name)) = existing {
            if existing_name.trim().is_empty() && !name.trim().is_empty() {
                conn.execute("UPDATE people SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name.trim(), now_iso(), id])?;
            }
            return Ok(id);
        }
    }
    insert_person(conn, name, email, "")
}

pub fn list_people(conn: &Connection) -> Result<Vec<Person>> {
    let mut stmt = conn.prepare("SELECT id, name, email, notes, created_at, updated_at FROM people ORDER BY name COLLATE NOCASE")?;
    let rows = stmt.query_map([], row_to_person)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_person(conn: &Connection, id: i64) -> Result<Person> {
    Ok(conn.query_row("SELECT id, name, email, notes, created_at, updated_at FROM people WHERE id = ?1", params![id], row_to_person)?)
}

pub fn update_person(conn: &Connection, id: i64, name: &str, email: Option<&str>, notes: &str) -> Result<Person> {
    let email = email.map(str::trim).filter(|e| !e.is_empty());
    conn.execute(
        "UPDATE people SET name = ?1, email = ?2, notes = ?3, updated_at = ?4 WHERE id = ?5",
        params![name.trim(), email, notes, now_iso(), id],
    )?;
    get_person(conn, id)
}

pub fn delete_person(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM people WHERE id = ?1", params![id])?;
    Ok(())
}

// ---------- Focus sessions (time-tracking) ----------

/// Elapsed minutes of a session; 0 while running (no `end`).
fn session_minutes(start: &str, end: Option<&str>) -> i64 {
    match (crate::scheduler::parse_dt(start), end.and_then(crate::scheduler::parse_dt)) {
        (Some(s), Some(e)) => (e - s).num_minutes().max(0),
        _ => 0,
    }
}

fn row_to_focus(r: &Row) -> rusqlite::Result<FocusSession> {
    let start: String = r.get("start")?;
    let end: Option<String> = r.get("end")?;
    let minutes = session_minutes(&start, end.as_deref());
    Ok(FocusSession { id: r.get("id")?, task_id: r.get("task_id")?, start, end, minutes })
}

/// Start a focus session on a task. Enforces a single active session: any still-running session is
/// stopped first (so the timer never double-counts).
pub fn start_focus(conn: &Connection, task_id: i64) -> Result<FocusSession> {
    let now = now_iso();
    conn.execute("UPDATE focus_sessions SET end = ?1 WHERE end IS NULL", params![now])?;
    conn.execute("INSERT INTO focus_sessions(task_id, start, end, created_at) VALUES(?1, ?2, NULL, ?2)", params![task_id, now])?;
    let id = conn.last_insert_rowid();
    Ok(conn.query_row("SELECT id, task_id, start, end FROM focus_sessions WHERE id = ?1", params![id], row_to_focus)?)
}

/// Stop a running focus session (no-op if already stopped).
pub fn stop_focus(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("UPDATE focus_sessions SET end = ?1 WHERE id = ?2 AND end IS NULL", params![now_iso(), id])?;
    Ok(())
}

/// The currently-running focus session, if any.
pub fn active_focus(conn: &Connection) -> Result<Option<FocusSession>> {
    Ok(conn
        .query_row("SELECT id, task_id, start, end FROM focus_sessions WHERE end IS NULL ORDER BY id DESC LIMIT 1", [], row_to_focus)
        .optional()?)
}

/// Total tracked minutes (completed sessions) for a task.
pub fn focus_minutes_for_task(conn: &Connection, task_id: i64) -> Result<i64> {
    let mut stmt = conn.prepare("SELECT start, end FROM focus_sessions WHERE task_id = ?1 AND end IS NOT NULL")?;
    let rows = stmt.query_map(params![task_id], |r| {
        let start: String = r.get(0)?;
        let end: Option<String> = r.get(1)?;
        Ok(session_minutes(&start, end.as_deref()))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<i64>>>()?.into_iter().sum())
}

/// `(estimated_minutes, actual focus minutes)` for completed, focus-tracked tasks — the learning
/// samples for the adaptive estimate (`scheduler::estimation_factor`).
pub fn estimation_samples(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let mut out = Vec::new();
    for t in list_tasks(conn)? {
        if t.status != "done" || t.estimated_minutes <= 0 {
            continue;
        }
        let actual = focus_minutes_for_task(conn, t.id)?;
        if actual > 0 {
            out.push((t.estimated_minutes, actual));
        }
    }
    Ok(out)
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
        // Embeddings are device-local (re-derived, never synced), so writing one must NOT mark the
        // note dirty — otherwise re-embedding (e.g. a reindex) would needlessly re-ship the note.
        crate::sync::with_capture_suppressed(|| {
            conn.execute("UPDATE notes SET embedding = ?2, embedding_model = ?3 WHERE id = ?1", params![id, emb, model])
        })?;
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

// ---------- Labels (cross-cutting taxonomy over any entity) ----------

fn row_to_label(r: &Row, count: i64) -> rusqlite::Result<Label> {
    Ok(Label {
        id: r.get("id")?,
        name: r.get("name")?,
        color: r.get("color")?,
        icon: r.get("icon")?,
        group_name: r.get("group_name")?,
        archived: r.get::<_, i64>("archived")? != 0,
        pref_window_start: r.get("pref_window_start")?,
        pref_window_end: r.get("pref_window_end")?,
        pref_min_chunk: r.get("pref_min_chunk")?,
        pref_max_chunk: r.get("pref_max_chunk")?,
        pref_batch: r.get::<_, i64>("pref_batch")? != 0,
        created_at: r.get("created_at")?,
        count,
    })
}

/// All non-archived labels with their usage counts, ordered by group then name.
pub fn list_labels(conn: &Connection) -> Result<Vec<Label>> {
    let mut stmt = conn.prepare(
        "SELECT l.*, COUNT(el.label_id) AS cnt
         FROM labels l LEFT JOIN entity_labels el ON el.label_id = l.id
         WHERE l.archived = 0
         GROUP BY l.id
         ORDER BY l.group_name IS NULL, lower(l.group_name), lower(l.name)",
    )?;
    let rows = stmt.query_map([], |r| {
        let count: i64 = r.get("cnt")?;
        row_to_label(r, count)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Insert a label (errors if the name already exists — the UI validates first).
pub fn create_label(conn: &Connection, input: &LabelInput) -> Result<i64> {
    conn.execute(
        "INSERT INTO labels(name, color, icon, group_name, pref_window_start, pref_window_end,
                            pref_min_chunk, pref_max_chunk, pref_batch, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            input.name.trim(), input.color, input.icon, input.group_name,
            input.pref_window_start, input.pref_window_end, input.pref_min_chunk, input.pref_max_chunk,
            input.pref_batch as i64, now_iso()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Find a label by name (case-insensitive), creating a minimal one (name + color) if absent. Used by
/// the inline/quick picker's "create on the fly".
pub fn get_or_create_label(conn: &Connection, name: &str, color: &str) -> Result<i64> {
    let name = name.trim();
    let existing: rusqlite::Result<i64> =
        conn.query_row("SELECT id FROM labels WHERE lower(name) = lower(?1)", params![name], |r| r.get(0));
    match existing {
        Ok(id) => Ok(id),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            conn.execute(
                "INSERT INTO labels(name, color, created_at) VALUES(?1, ?2, ?3)",
                params![name, color, now_iso()],
            )?;
            Ok(conn.last_insert_rowid())
        }
        Err(e) => Err(e.into()),
    }
}

/// Update a label's display + scheduling prefs.
pub fn update_label(conn: &Connection, id: i64, input: &LabelInput) -> Result<()> {
    conn.execute(
        "UPDATE labels SET name = ?2, color = ?3, icon = ?4, group_name = ?5, pref_window_start = ?6,
                          pref_window_end = ?7, pref_min_chunk = ?8, pref_max_chunk = ?9, pref_batch = ?10
         WHERE id = ?1",
        params![
            id, input.name.trim(), input.color, input.icon, input.group_name,
            input.pref_window_start, input.pref_window_end, input.pref_min_chunk, input.pref_max_chunk,
            input.pref_batch as i64
        ],
    )?;
    Ok(())
}

/// Delete a label (its `entity_labels` rows cascade away).
pub fn delete_label(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM labels WHERE id = ?1", params![id])?;
    Ok(())
}

/// Merge `from` into `into`: re-point its taggings (skipping dupes), then delete `from`.
pub fn merge_labels(conn: &Connection, from: i64, into: i64) -> Result<()> {
    if from == into {
        return Ok(());
    }
    conn.execute("UPDATE OR IGNORE entity_labels SET label_id = ?2 WHERE label_id = ?1", params![from, into])?;
    conn.execute("DELETE FROM labels WHERE id = ?1", params![from])?; // cascades any leftover dupes
    Ok(())
}

/// Replace the full set of labels on an entity (like `set_page_links`).
pub fn set_entity_labels(conn: &Connection, kind: &str, entity_id: i64, label_ids: &[i64]) -> Result<()> {
    conn.execute("DELETE FROM entity_labels WHERE entity_kind = ?1 AND entity_id = ?2", params![kind, entity_id])?;
    for lid in label_ids {
        conn.execute(
            "INSERT OR IGNORE INTO entity_labels(label_id, entity_kind, entity_id) VALUES(?1, ?2, ?3)",
            params![lid, kind, entity_id],
        )?;
    }
    Ok(())
}

/// The labels on a given entity.
pub fn labels_for(conn: &Connection, kind: &str, entity_id: i64) -> Result<Vec<Label>> {
    let mut stmt = conn.prepare(
        "SELECT l.* FROM entity_labels el JOIN labels l ON l.id = el.label_id
         WHERE el.entity_kind = ?1 AND el.entity_id = ?2 AND l.archived = 0
         ORDER BY l.group_name IS NULL, lower(l.group_name), lower(l.name)",
    )?;
    let rows = stmt.query_map(params![kind, entity_id], |r| row_to_label(r, 0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Labels for many entities of one kind, keyed by entity id. Used by dense calendar views so they
/// can color/filter without doing one IPC round-trip per event/block.
pub fn labels_for_entities(conn: &Connection, kind: &str, ids: &[i64]) -> Result<std::collections::BTreeMap<i64, Vec<Label>>> {
    let mut out: std::collections::BTreeMap<i64, Vec<Label>> = ids.iter().copied().map(|id| (id, Vec::new())).collect();
    if ids.is_empty() {
        return Ok(out);
    }

    let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT el.entity_id AS tagged_entity_id, l.*
         FROM entity_labels el JOIN labels l ON l.id = el.label_id
         WHERE el.entity_kind = ? AND el.entity_id IN ({placeholders}) AND l.archived = 0
         ORDER BY el.entity_id, l.group_name IS NULL, lower(l.group_name), lower(l.name)"
    );
    let mut values = Vec::with_capacity(ids.len() + 1);
    values.push(rusqlite::types::Value::Text(kind.to_string()));
    values.extend(ids.iter().map(|id| rusqlite::types::Value::Integer(*id)));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values), |r| {
        let entity_id: i64 = r.get("tagged_entity_id")?;
        Ok((entity_id, row_to_label(r, 0)?))
    })?;
    for row in rows {
        let (entity_id, label) = row?;
        out.entry(entity_id).or_default().push(label);
    }
    Ok(out)
}

/// The free text of any labelable entity, for keyword auto-labeling. `kind` is the label-kind string
/// ("task"/"event"/"page"/"person"/"habit"/"project"). None when the kind is unknown or no such row.
pub fn entity_text(conn: &Connection, kind: &str, id: i64) -> Result<Option<String>> {
    let text: Option<String> = match kind {
        "task" => conn.query_row("SELECT title || ' ' || notes FROM tasks WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        "event" => conn.query_row("SELECT title FROM events WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        "page" => conn.query_row("SELECT COALESCE(title, '') || ' ' || content FROM notes WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        "person" => conn.query_row("SELECT name || ' ' || notes FROM people WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        "habit" => conn.query_row("SELECT name FROM habits WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        "project" => conn.query_row("SELECT name FROM projects WHERE id = ?1", params![id], |r| r.get(0)).optional()?,
        _ => None,
    };
    Ok(text)
}

/// True if `needle` occurs in `hay` on word boundaries (both lowercase) — so "work" matches "more
/// work" but not "homework". Avoids false-positive auto-labels from substrings.
fn word_bounded_contains(hay: &str, needle: &str) -> bool {
    let (hb, nb) = (hay.as_bytes(), needle.as_bytes());
    let mut start = 0;
    while let Some(pos) = hay[start..].find(needle) {
        let i = start + pos;
        let before_ok = i == 0 || !hb[i - 1].is_ascii_alphanumeric();
        let after = i + nb.len();
        let after_ok = after >= hb.len() || !hb[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
    }
    false
}

/// Keyword auto-labeling: existing labels whose name (≥3 chars) appears as a whole word/phrase,
/// case-insensitively, in `text` and isn't already applied. The deterministic core surfaced as
/// confirm-chips in the picker. Pure.
pub fn suggest_labels_from(labels: &[Label], text: &str, applied: &[i64]) -> Vec<Label> {
    let hay = text.to_lowercase();
    labels
        .iter()
        .filter(|l| {
            let name = l.name.trim().to_lowercase();
            name.len() >= 3 && !applied.contains(&l.id) && word_bounded_contains(&hay, &name)
        })
        .cloned()
        .collect()
}

/// Every entity tagged with a label (for the cross-cutting filtered view). Returns (kind, id) refs;
/// the frontend resolves them to titles from its loaded store.
pub fn entities_for_label(conn: &Connection, label_id: i64) -> Result<Vec<EntityRef>> {
    let mut stmt = conn.prepare("SELECT entity_kind, entity_id FROM entity_labels WHERE label_id = ?1")?;
    let rows = stmt.query_map(params![label_id], |r| Ok(EntityRef { kind: r.get(0)?, id: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Resolve each task's labels into a merged `SchedulePref` the scheduler honors: window = the union
/// (widest) of its labels' windows; min_chunk = the strictest (max); batch = any. Tasks with no
/// actionable labels are omitted (no pref → no effect).
pub fn resolve_task_prefs(conn: &Connection, task_ids: &[i64]) -> Result<std::collections::HashMap<i64, SchedulePref>> {
    let hm = |s: Option<String>| -> Option<NaiveTime> { s.and_then(|v| NaiveTime::parse_from_str(&v, "%H:%M").ok()) };
    let mut out = std::collections::HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT l.pref_window_start, l.pref_window_end, l.pref_min_chunk, l.pref_batch
         FROM entity_labels el JOIN labels l ON l.id = el.label_id
         WHERE el.entity_kind = 'task' AND el.entity_id = ?1 AND l.archived = 0",
    )?;
    for &tid in task_ids {
        let (mut win_start, mut win_end): (Option<NaiveTime>, Option<NaiveTime>) = (None, None);
        let mut min_chunk: Option<i64> = None;
        let mut batch = false;
        let rows = stmt.query_map(params![tid], |r| {
            Ok((
                hm(r.get(0)?),
                hm(r.get(1)?),
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })?;
        for row in rows {
            let (ws, we, mc, b) = row?;
            if let (Some(ws), Some(we)) = (ws, we) {
                win_start = Some(win_start.map_or(ws, |c| c.min(ws)));
                win_end = Some(win_end.map_or(we, |c| c.max(we)));
            }
            if let Some(mc) = mc.filter(|&m| m > 0) {
                min_chunk = Some(min_chunk.map_or(mc, |c| c.max(mc)));
            }
            batch = batch || b;
        }
        let window = match (win_start, win_end) {
            (Some(s), Some(e)) if e > s => Some((s, e)),
            _ => None,
        };
        if window.is_some() || min_chunk.is_some() || batch {
            out.insert(tid, SchedulePref { window, min_chunk, batch });
        }
    }
    Ok(out)
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
    crate::sync::register_functions(&conn).unwrap();
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
    fn entity_index_upsert_and_recall_roundtrip() {
        let conn = mem();
        let emb = crate::hermes::vec_to_blob(&[0.1, 0.2, 0.3]);
        upsert_entity_index(&conn, EntityKind::Task, 7, "write the report", "h1", Some(&emb), Some("bge")).unwrap();
        upsert_entity_index(&conn, EntityKind::Event, 9, "team sync", "h2", None, None).unwrap();

        // Empty kinds → everything; filtered kinds → only that kind.
        assert_eq!(entity_index_for_recall(&conn, &[]).unwrap().len(), 2);
        let tasks = entity_index_for_recall(&conn, &[EntityKind::Task]).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, 7);
        assert_eq!(tasks[0].text, "write the report");
        assert_eq!(tasks[0].embedding.as_deref(), Some(emb.as_slice()));
        // The unindexed event still surfaces, just without a vector.
        assert!(entity_index_for_recall(&conn, &[EntityKind::Event]).unwrap()[0].embedding.is_none());

        // Upsert replaces in place (no duplicate PK), and hashes are retrievable.
        upsert_entity_index(&conn, EntityKind::Task, 7, "write the Q3 report", "h1b", Some(&emb), Some("bge")).unwrap();
        assert_eq!(entity_index_for_recall(&conn, &[EntityKind::Task]).unwrap().len(), 1);
        assert_eq!(entity_index_hashes(&conn, EntityKind::Task).unwrap().get(&7).map(String::as_str), Some("h1b"));

        delete_entity_index(&conn, EntityKind::Task, 7).unwrap();
        assert!(entity_index_for_recall(&conn, &[EntityKind::Task]).unwrap().is_empty());
    }

    #[test]
    fn estimation_samples_are_completed_focus_tracked_tasks() {
        let conn = mem();
        let done = insert_task(&conn, None, "Wrote it", "", 60, None, 2, 30, 120, &[]).unwrap();
        let other = insert_task(&conn, None, "Not done", "", 60, None, 2, 30, 120, &[]).unwrap();
        // 90 minutes of real focus on the done task (1.5× its 60-min estimate).
        conn.execute(
            "INSERT INTO focus_sessions(task_id, start, end, created_at) VALUES(?1, ?2, ?3, ?2)",
            params![done, "2026-06-15T10:00:00", "2026-06-15T11:30:00"],
        )
        .unwrap();
        // A focus session on a NOT-done task should not become a sample.
        conn.execute(
            "INSERT INTO focus_sessions(task_id, start, end, created_at) VALUES(?1, ?2, ?3, ?2)",
            params![other, "2026-06-15T10:00:00", "2026-06-15T10:30:00"],
        )
        .unwrap();
        set_task_status(&conn, done, "done").unwrap();

        let samples = estimation_samples(&conn).unwrap();
        assert_eq!(samples, vec![(60, 90)], "only the completed, focus-tracked task is a sample");
    }

    #[test]
    fn focus_sessions_track_one_active_at_a_time() {
        let conn = mem();
        let t1 = insert_task(&conn, None, "Write", "", 60, None, 2, 30, 120, &[]).unwrap();
        let t2 = insert_task(&conn, None, "Read", "", 60, None, 2, 30, 120, &[]).unwrap();

        let s1 = start_focus(&conn, t1).unwrap();
        assert_eq!(active_focus(&conn).unwrap().unwrap().id, s1.id, "session is active");
        assert!(s1.end.is_none());

        // Starting a second session stops the first (single active).
        let s2 = start_focus(&conn, t2).unwrap();
        assert_eq!(active_focus(&conn).unwrap().unwrap().id, s2.id);
        assert!(active_focus(&conn).unwrap().unwrap().task_id == t2);

        stop_focus(&conn, s2.id).unwrap();
        assert!(active_focus(&conn).unwrap().is_none(), "nothing active after stop");

        // The first task accrued some (>=0) tracked minutes; deleting the task cascades its sessions.
        let _ = focus_minutes_for_task(&conn, t1).unwrap();
        delete_task(&conn, t1).unwrap();
        assert_eq!(focus_minutes_for_task(&conn, t1).unwrap(), 0);
    }

    #[test]
    fn suggest_labels_matches_names_in_entity_text() {
        let conn = mem();
        let school = get_or_create_label(&conn, "School", "#0ea5e9").unwrap();
        get_or_create_label(&conn, "Work", "#10b981").unwrap();
        let tid = insert_task(&conn, None, "Do school homework", "", 60, None, 2, 30, 120, &[]).unwrap();

        let text = entity_text(&conn, "task", tid).unwrap().unwrap();
        let labels = list_labels(&conn).unwrap();
        let suggestions = suggest_labels_from(&labels, &text, &[]);
        assert!(suggestions.iter().any(|l| l.name == "School"), "School appears in the task text");
        assert!(!suggestions.iter().any(|l| l.name == "Work"), "Work does not appear");

        // Already-applied labels are excluded from suggestions.
        assert!(suggest_labels_from(&labels, &text, &[school]).iter().all(|l| l.name != "School"));
        // Unknown kinds / missing rows yield no text.
        assert!(entity_text(&conn, "bogus", tid).unwrap().is_none());
    }

    #[test]
    fn people_upsert_dedupes_by_email_and_indexes() {
        let conn = mem();
        let a = upsert_person_by_email(&conn, "Ava Stone", Some("ava@example.com")).unwrap();
        // Same email → same person (no duplicate), and a blank name backfills.
        let b = upsert_person_by_email(&conn, "", Some("ava@example.com")).unwrap();
        assert_eq!(a, b, "same email dedupes to one person");
        assert_eq!(list_people(&conn).unwrap().len(), 1);
        assert_eq!(get_person(&conn, a).unwrap().name, "Ava Stone");
        // No email → always a new row.
        upsert_person_by_email(&conn, "Anon", None).unwrap();
        assert_eq!(list_people(&conn).unwrap().len(), 2);

        // People flow into the cross-entity index.
        update_person(&conn, a, "Ava Stone", Some("ava@example.com"), "met at the conference").unwrap();
        let items = entities_for_index(&conn).unwrap();
        let person = items.iter().find(|it| it.kind == EntityKind::Person && it.id == a).unwrap();
        assert_eq!(person.text, "Ava Stone\nmet at the conference");

        delete_person(&conn, a).unwrap();
        assert_eq!(list_people(&conn).unwrap().len(), 1);
    }

    #[test]
    fn entity_neighbors_walks_links_both_ways() {
        let conn = mem();
        let task = insert_task(&conn, None, "Prep deck", "", 60, None, 2, 30, 120, &[]).unwrap();
        let hub = insert_page(&conn, "Project hub", None, "see [[Sub page]]", None, None, None).unwrap();
        let sub = insert_page(&conn, "Sub page", None, "details", None, None, None).unwrap();
        link_entity(&conn, hub, "task", task).unwrap();
        set_page_links(&conn, hub, &["Sub page".to_string()]).unwrap();

        // From the page: its linked task, its outgoing page link.
        let from_page = entity_neighbors(&conn, EntityKind::Page, hub).unwrap();
        assert!(from_page.contains(&(EntityKind::Task, task)));
        assert!(from_page.contains(&(EntityKind::Page, sub)));
        // From the task: the page that references it.
        assert_eq!(entity_neighbors(&conn, EntityKind::Task, task).unwrap(), vec![(EntityKind::Page, hub)]);
        // From the sub page: the backlink from the hub.
        assert!(entity_neighbors(&conn, EntityKind::Page, sub).unwrap().contains(&(EntityKind::Page, hub)));
    }

    #[test]
    fn entities_for_index_projects_all_kinds_and_skips_blanks() {
        let conn = mem();
        let tid = insert_task(&conn, None, "Write report", "for Q3", 60, None, 2, 30, 120, &[]).unwrap();
        let eid = insert_event(&conn, "Team sync", "2026-06-15T10:00:00", "2026-06-15T10:30:00", "fixed").unwrap();
        let pid = insert_page(&conn, "Trip plan", None, "pack bags", None, None, None).unwrap();
        // A whitespace-only event title should be skipped (nothing to embed).
        insert_event(&conn, "   ", "2026-06-15T12:00:00", "2026-06-15T12:30:00", "fixed").unwrap();

        let items = entities_for_index(&conn).unwrap();
        let find = |k: EntityKind, id: i64| items.iter().find(|it| it.kind == k && it.id == id);
        assert_eq!(find(EntityKind::Task, tid).unwrap().text, "Write report\nfor Q3");
        assert_eq!(find(EntityKind::Event, eid).unwrap().text, "Team sync");
        assert_eq!(find(EntityKind::Page, pid).unwrap().text, "Trip plan\npack bags");
        assert_eq!(items.iter().filter(|it| it.kind == EntityKind::Event).count(), 1, "blank-title event skipped");
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

    // ---- Labels ----

    fn label(name: &str) -> LabelInput {
        LabelInput {
            name: name.into(),
            color: "#0ea5e9".into(),
            icon: None,
            group_name: None,
            pref_window_start: None,
            pref_window_end: None,
            pref_min_chunk: None,
            pref_max_chunk: None,
            pref_batch: false,
        }
    }

    #[test]
    fn label_get_or_create_is_case_insensitive_and_idempotent() {
        let conn = mem();
        let a = get_or_create_label(&conn, "Health", "#10b981").unwrap();
        let b = get_or_create_label(&conn, "  health  ", "#000").unwrap();
        assert_eq!(a, b, "same name (case/space-insensitive) → same label");
        assert_eq!(list_labels(&conn).unwrap().len(), 1);
    }

    #[test]
    fn entity_labels_replace_set_and_query_both_ways() {
        let conn = mem();
        let work = create_label(&conn, &label("Work")).unwrap();
        let deep = create_label(&conn, &label("Deep")).unwrap();
        let admin = create_label(&conn, &label("Admin")).unwrap();

        set_entity_labels(&conn, "task", 5, &[work, deep]).unwrap();
        assert_eq!(labels_for(&conn, "task", 5).unwrap().iter().map(|l| l.id).collect::<std::collections::BTreeSet<_>>(), [deep, work].into());

        // Replace (not append): now only Admin.
        set_entity_labels(&conn, "task", 5, &[admin]).unwrap();
        let ids: Vec<i64> = labels_for(&conn, "task", 5).unwrap().iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![admin]);

        // Cross-cutting: Work tags a task AND a page; entities_for_label sees both kinds.
        set_entity_labels(&conn, "task", 9, &[work]).unwrap();
        set_entity_labels(&conn, "page", 2, &[work]).unwrap();
        let mut refs: Vec<(String, i64)> = entities_for_label(&conn, work).unwrap().into_iter().map(|e| (e.kind, e.id)).collect();
        refs.sort();
        assert_eq!(refs, vec![("page".into(), 2), ("task".into(), 9)]);

        // list_labels carries usage counts.
        let counts: std::collections::HashMap<i64, i64> = list_labels(&conn).unwrap().into_iter().map(|l| (l.id, l.count)).collect();
        assert_eq!(counts[&work], 2);
        assert_eq!(counts[&admin], 1);
    }

    #[test]
    fn labels_for_entities_returns_a_batched_map_with_empty_entries() {
        let conn = mem();
        let work = create_label(&conn, &label("Work")).unwrap();
        let deep = create_label(&conn, &label("Deep")).unwrap();
        let page_only = create_label(&conn, &label("Page")).unwrap();

        set_entity_labels(&conn, "task", 1, &[work, deep]).unwrap();
        set_entity_labels(&conn, "task", 3, &[work]).unwrap();
        set_entity_labels(&conn, "page", 1, &[page_only]).unwrap();

        let by_task = labels_for_entities(&conn, "task", &[1, 2, 3]).unwrap();
        assert_eq!(by_task[&1].iter().map(|l| l.id).collect::<Vec<_>>(), vec![deep, work]);
        assert!(by_task[&2].is_empty(), "requested but untagged entities get an empty list");
        assert_eq!(by_task[&3].iter().map(|l| l.id).collect::<Vec<_>>(), vec![work]);
        assert!(labels_for_entities(&conn, "task", &[]).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_label_cascades_its_taggings() {
        let conn = mem();
        let l = create_label(&conn, &label("Temp")).unwrap();
        set_entity_labels(&conn, "event", 3, &[l]).unwrap();
        delete_label(&conn, l).unwrap();
        assert!(labels_for(&conn, "event", 3).unwrap().is_empty());
        assert!(entities_for_label(&conn, l).unwrap().is_empty());
    }

    #[test]
    fn resolve_task_prefs_merges_label_scheduling() {
        let conn = mem();
        let mut deep = label("Deep work");
        deep.pref_window_start = Some("09:00".into());
        deep.pref_window_end = Some("12:00".into());
        deep.pref_min_chunk = Some(60);
        let deep_id = create_label(&conn, &deep).unwrap();

        let mut focus = label("Focus");
        focus.pref_window_start = Some("06:00".into()); // union → widest window 06:00–13:00
        focus.pref_window_end = Some("13:00".into());
        focus.pref_min_chunk = Some(90); // strictest → 90
        let focus_id = create_label(&conn, &focus).unwrap();

        set_entity_labels(&conn, "task", 1, &[deep_id, focus_id]).unwrap();
        set_entity_labels(&conn, "task", 2, &[create_label(&conn, &label("Plain")).unwrap()]).unwrap();

        let prefs = resolve_task_prefs(&conn, &[1, 2, 3]).unwrap();
        let p = prefs.get(&1).expect("task 1 has actionable labels");
        let (ws, we) = p.window.unwrap();
        assert_eq!((ws, we), (NaiveTime::from_hms_opt(6, 0, 0).unwrap(), NaiveTime::from_hms_opt(13, 0, 0).unwrap()));
        assert_eq!(p.min_chunk, Some(90), "strictest min-chunk wins");
        assert!(!prefs.contains_key(&2), "a non-actionable label yields no pref");
        assert!(!prefs.contains_key(&3), "an untagged task yields no pref");
    }

    #[test]
    fn merge_repoints_taggings_dedupes_and_removes_source() {
        let conn = mem();
        let from = create_label(&conn, &label("Errand")).unwrap();
        let into = create_label(&conn, &label("Errands")).unwrap();
        // task 1 has both (a dup after merge); task 2 has only `from`.
        set_entity_labels(&conn, "task", 1, &[from, into]).unwrap();
        set_entity_labels(&conn, "task", 2, &[from]).unwrap();

        merge_labels(&conn, from, into).unwrap();
        // `from` is gone; both tasks now carry `into` exactly once.
        assert!(list_labels(&conn).unwrap().iter().all(|l| l.id != from));
        assert_eq!(labels_for(&conn, "task", 1).unwrap().iter().map(|l| l.id).collect::<Vec<_>>(), vec![into]);
        assert_eq!(labels_for(&conn, "task", 2).unwrap().iter().map(|l| l.id).collect::<Vec<_>>(), vec![into]);
    }
}
