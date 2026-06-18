//! Translate between local SQLite rows and device-independent wire changes.
//!
//! - [`build_outbox`] reads everything marked `dirty` (rows + tombstones), stamps each with a fresh
//!   HLC, clears the dirty flag, and returns [`Change`]s with foreign keys rewritten as the
//!   referenced rows' `uuid`s.
//! - [`apply_changes`] takes remote [`Change`]s, resolves those uuids back to local ids, and applies
//!   them last-writer-wins (higher HLC wins), honoring tombstones.
//!
//! Both run with change-capture suppressed ([`super::with_capture_suppressed`]) so our writes here
//! don't re-mark rows dirty. Callers hold the DB lock.

use super::hlc::{self, HlcState};
use super::schema::{self, TableSpec, INTERNAL_COLS};
use super::with_capture_suppressed;
use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Upsert,
    Delete,
}

/// One row-level change on the wire. For `Upsert`, `fields` maps column → JSON value, where FK
/// columns and the polymorphic id column carry the *referenced row's uuid* (or null) rather than a
/// local integer id. `Delete` carries no fields.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Change {
    pub table: String,
    pub uuid: String,
    pub op: Op,
    pub hlc: String,
    #[serde(default)]
    pub fields: Map<String, Json>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub applied: usize,
    pub skipped: usize,
    /// Highest HLC seen in the batch — the caller advances its per-peer watermark to this.
    pub max_hlc: String,
}

// ---------------- value <-> json ----------------

fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Integer(i) => Json::from(*i),
        Value::Real(f) => Json::from(*f),
        Value::Text(s) => Json::from(s.clone()),
        // Blobs aren't in any synced column today, but keep the codec total just in case.
        Value::Blob(b) => {
            use base64::Engine as _;
            let mut o = Map::new();
            o.insert("$b64".into(), Json::from(base64::engine::general_purpose::STANDARD.encode(b)));
            Json::Object(o)
        }
    }
}

fn json_to_value(j: &Json) -> Value {
    match j {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(*b as i64),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Json::String(s) => Value::Text(s.clone()),
        Json::Object(o) => {
            use base64::Engine as _;
            if let Some(Json::String(b64)) = o.get("$b64") {
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    return Value::Blob(bytes);
                }
            }
            Value::Null
        }
        Json::Array(_) => Value::Null,
    }
}

// ---------------- schema helpers ----------------

/// The uuid of a row in `table` by its local integer id.
fn uuid_of(conn: &Connection, table: &str, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(&format!("SELECT uuid FROM {table} WHERE id = ?1"), params![id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// The local integer id of a row in `table` by its uuid.
fn id_of(conn: &Connection, table: &str, uuid: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(&format!("SELECT id FROM {table} WHERE uuid = ?1"), params![uuid], |r| r.get(0))
        .optional()?)
}

/// Columns with a NOT NULL constraint (so we know which unresolved FKs can be nulled vs must skip).
fn notnull_cols(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| {
        let name: String = r.get("name")?;
        let notnull: i64 = r.get("notnull")?;
        Ok((name, notnull))
    })?;
    let mut set = HashSet::new();
    for r in rows {
        let (name, notnull) = r?;
        if notnull != 0 {
            set.insert(name);
        }
    }
    Ok(set)
}

/// Read all columns of one row (by uuid) as a name→Value map.
fn read_row(conn: &Connection, table: &str, uuid: &str) -> Result<Option<HashMap<String, Value>>> {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} WHERE uuid = ?1"))?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let row = stmt
        .query_row(params![uuid], |r| {
            let mut m = HashMap::new();
            for (i, c) in cols.iter().enumerate() {
                m.insert(c.clone(), r.get::<_, Value>(i)?);
            }
            Ok(m)
        })
        .optional()?;
    Ok(row)
}

// ---------------- build (local → wire) ----------------

/// Assign a fresh HLC to every locally-changed row + tombstone (`dirty = 1`) and clear the flag.
/// `dirty` means "written locally, not yet stamped"; after stamping, a row's `updated_hlc` is what
/// peers pull against. Idempotent: a clean DB stamps nothing. Runs with capture suppressed so our
/// own writes here don't re-dirty. Returns the highest HLC stamped (or "" if nothing).
pub fn stamp_dirty(conn: &Connection, node: &str, clock: &mut HlcState, now_ms: u64) -> Result<String> {
    with_capture_suppressed(|| stamp_dirty_inner(conn, node, clock, now_ms))
}

fn stamp_dirty_inner(conn: &Connection, node: &str, clock: &mut HlcState, now_ms: u64) -> Result<String> {
    let mut max_hlc = String::new();
    for spec in schema::TABLES {
        let dirty_uuids: Vec<String> = {
            let mut stmt = conn.prepare(&format!("SELECT uuid FROM {} WHERE dirty = 1", spec.name))?;
            let it = stmt.query_map([], |r| r.get::<_, String>(0))?;
            it.collect::<rusqlite::Result<_>>()?
        };
        for uuid in dirty_uuids {
            let (w, c) = clock.tick(now_ms);
            let h = hlc::encode(w, c, node);
            conn.execute(
                &format!("UPDATE {} SET updated_hlc = ?1, dirty = 0 WHERE uuid = ?2", spec.name),
                params![h, uuid],
            )?;
            if h > max_hlc { max_hlc = h; }
        }
    }
    // Tombstones from local deletes.
    let tombs: Vec<(String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT entity_table, entity_uuid FROM sync_tombstones WHERE dirty = 1")?;
        let it = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        it.collect::<rusqlite::Result<_>>()?
    };
    for (table, uuid) in tombs {
        let (w, c) = clock.tick(now_ms);
        let h = hlc::encode(w, c, node);
        conn.execute(
            "UPDATE sync_tombstones SET hlc = ?1, dirty = 0 WHERE entity_table = ?2 AND entity_uuid = ?3",
            params![h, table, uuid],
        )?;
        if h > max_hlc { max_hlc = h; }
    }
    Ok(max_hlc)
}

/// Collect every change with an HLC strictly greater than `since` — the delta a peer pulls. Pass
/// `""` for a full snapshot. Read-only; call [`stamp_dirty`] first so local edits have HLCs.
pub fn changes_since(conn: &Connection, since: &str) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    for spec in schema::TABLES {
        let uuids: Vec<String> = {
            let mut stmt = conn.prepare(&format!(
                "SELECT uuid FROM {} WHERE updated_hlc IS NOT NULL AND updated_hlc > ?1", spec.name))?;
            let it = stmt.query_map(params![since], |r| r.get::<_, String>(0))?;
            it.collect::<rusqlite::Result<_>>()?
        };
        for uuid in uuids {
            let row = match read_row(conn, spec.name, &uuid)? { Some(r) => r, None => continue };
            let hlc = match row.get("updated_hlc") {
                Some(Value::Text(h)) => h.clone(),
                _ => continue,
            };
            let fields = build_fields(conn, spec, &row)?;
            out.push(Change { table: spec.name.into(), uuid, op: Op::Upsert, hlc, fields });
        }
    }
    // Tombstones (deletes).
    let mut stmt = conn.prepare(
        "SELECT entity_table, entity_uuid, hlc FROM sync_tombstones WHERE hlc > ?1")?;
    let rows = stmt.query_map(params![since], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?;
    for r in rows {
        let (table, uuid, hlc) = r?;
        if schema::spec(&table).is_none() { continue; }
        out.push(Change { table, uuid, op: Op::Delete, hlc, fields: Map::new() });
    }
    Ok(out)
}

/// Convenience for the simple "stamp everything dirty, then emit a full snapshot" flow (tests + a
/// first sync). Equivalent to [`stamp_dirty`] followed by [`changes_since`]`("")`.
pub fn build_outbox(conn: &Connection, node: &str, clock: &mut HlcState, now_ms: u64) -> Result<Vec<Change>> {
    stamp_dirty(conn, node, clock, now_ms)?;
    changes_since(conn, "")
}

/// Rewrite a local row into wire fields: drop internal/skip columns; replace FK ids and the
/// polymorphic id with the referenced rows' uuids.
fn build_fields(conn: &Connection, spec: &TableSpec, row: &HashMap<String, Value>) -> Result<Map<String, Json>> {
    let mut map = Map::new();
    for (col, val) in row {
        if INTERNAL_COLS.contains(&col.as_str()) || spec.skip.contains(&col.as_str()) {
            continue;
        }
        // Simple FK → referenced uuid.
        if let Some((_, ref_table)) = spec.fks.iter().find(|(c, _)| c == col) {
            let j = match val {
                Value::Integer(id) => uuid_of(conn, ref_table, *id)?.map(Json::from).unwrap_or(Json::Null),
                _ => Json::Null,
            };
            map.insert(col.clone(), j);
            continue;
        }
        // Polymorphic id column → uuid of (kind_to_table(kind)).
        if let Some((kind_col, id_col)) = spec.poly {
            if col == id_col {
                let kind = row.get(kind_col).and_then(|v| match v {
                    Value::Text(s) => Some(s.clone()),
                    _ => None,
                });
                let j = match (kind.as_deref().and_then(schema::kind_to_table), val) {
                    (Some(t), Value::Integer(id)) => uuid_of(conn, t, *id)?.map(Json::from).unwrap_or(Json::Null),
                    _ => Json::Null,
                };
                map.insert(col.clone(), j);
                continue;
            }
        }
        map.insert(col.clone(), value_to_json(val));
    }
    Ok(map)
}

// ---------------- apply (wire → local) ----------------

/// Apply remote changes last-writer-wins. `clock` is advanced past every change seen.
pub fn apply_changes(
    conn: &Connection,
    clock: &mut HlcState,
    now_ms: u64,
    changes: &[Change],
) -> Result<ApplyStats> {
    with_capture_suppressed(|| apply_changes_inner(conn, clock, now_ms, changes))
}

fn apply_changes_inner(
    conn: &Connection,
    clock: &mut HlcState,
    now_ms: u64,
    changes: &[Change],
) -> Result<ApplyStats> {
    let mut stats = ApplyStats::default();

    // Advance our clock past everything we received, so future local writes order after them.
    for ch in changes {
        if let Some((w, c, _)) = hlc::decode(&ch.hlc) {
            clock.observe(now_ms, w, c);
        }
        if ch.hlc > stats.max_hlc {
            stats.max_hlc = ch.hlc.clone();
        }
    }

    // Deletes don't depend on order; do them first so a later resurrecting upsert is clean.
    for ch in changes.iter().filter(|c| c.op == Op::Delete) {
        if apply_delete(conn, ch)? {
            stats.applied += 1;
        } else {
            stats.skipped += 1;
        }
    }

    // Upserts in dependency order, with a fixpoint loop for FKs that arrive out of order
    // (e.g. notes.parent_id self-references, or a child whose parent is later in the batch).
    let mut pending: Vec<&Change> = order_upserts(changes);
    loop {
        let mut progressed = false;
        let mut still = Vec::new();
        for ch in pending {
            match apply_upsert(conn, ch, false)? {
                Some(true) => { stats.applied += 1; progressed = true; }
                Some(false) => { stats.skipped += 1; progressed = true; } // resolved but LWW-skipped
                None => still.push(ch), // unresolved FK → retry next pass
            }
        }
        pending = still;
        if pending.is_empty() || !progressed {
            break;
        }
    }
    // Final forced pass: null out unresolved nullable FKs; rows still missing a required FK are skipped.
    for ch in pending {
        match apply_upsert(conn, ch, true)? {
            Some(true) => stats.applied += 1,
            _ => stats.skipped += 1,
        }
    }

    Ok(stats)
}

fn apply_delete(conn: &Connection, ch: &Change) -> Result<bool> {
    let table = &ch.table;
    // Record/raise the tombstone to the latest delete HLC.
    let existing_tomb: Option<String> = conn
        .query_row(
            "SELECT hlc FROM sync_tombstones WHERE entity_table = ?1 AND entity_uuid = ?2",
            params![table, ch.uuid],
            |r| r.get(0),
        )
        .optional()?;
    let tomb_hlc = match &existing_tomb {
        Some(h) if h.as_str() >= ch.hlc.as_str() => h.clone(),
        _ => ch.hlc.clone(),
    };
    conn.execute(
        "INSERT INTO sync_tombstones(entity_table, entity_uuid, hlc, dirty) VALUES(?1, ?2, ?3, 0) \
         ON CONFLICT(entity_table, entity_uuid) DO UPDATE SET hlc = ?3, dirty = 0",
        params![table, ch.uuid, tomb_hlc],
    )?;

    // Delete the row only if the delete is newer than the row's last write (LWW).
    let row_hlc: Option<String> = conn
        .query_row(
            &format!("SELECT updated_hlc FROM {table} WHERE uuid = ?1"),
            params![ch.uuid],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    match row_hlc {
        Some(rh) if rh.as_str() >= ch.hlc.as_str() => Ok(false), // a newer local update wins
        Some(_) => {
            conn.execute(&format!("DELETE FROM {table} WHERE uuid = ?1"), params![ch.uuid])?;
            Ok(true)
        }
        None => Ok(false), // nothing to delete locally; tombstone recorded for any future upsert
    }
}

/// Returns `Some(true)` applied, `Some(false)` resolved-but-skipped (LWW/tombstone), `None` deferred
/// (an FK couldn't be resolved yet). When `force`, unresolved nullable FKs become NULL.
fn apply_upsert(conn: &Connection, ch: &Change, force: bool) -> Result<Option<bool>> {
    let spec = match schema::spec(&ch.table) {
        Some(s) => s,
        None => return Ok(Some(false)),
    };
    let table = spec.name;

    // A delete with a >= HLC already won — drop this upsert.
    let tomb: Option<String> = conn
        .query_row(
            "SELECT hlc FROM sync_tombstones WHERE entity_table = ?1 AND entity_uuid = ?2",
            params![table, ch.uuid],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(th) = tomb {
        if th.as_str() >= ch.hlc.as_str() {
            return Ok(Some(false));
        }
    }

    // LWW vs an existing local row.
    let existing_hlc: Option<String> = conn
        .query_row(
            &format!("SELECT updated_hlc FROM {table} WHERE uuid = ?1"),
            params![ch.uuid],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let exists = existing_hlc.is_some();
    if let Some(eh) = &existing_hlc {
        if eh.as_str() >= ch.hlc.as_str() {
            return Ok(Some(false)); // our copy is newer-or-equal
        }
    }

    // Resolve field values into a column→Value list, translating FK/poly uuids back to local ids.
    let notnull = notnull_cols(conn, table)?;
    let mut cols: Vec<String> = Vec::new();
    let mut vals: Vec<Value> = Vec::new();
    for (col, jval) in &ch.fields {
        // Simple FK.
        if let Some((_, ref_table)) = spec.fks.iter().find(|(c, _)| c == col) {
            match resolve_ref(conn, ref_table, jval)? {
                RefRes::Ok(v) => { cols.push(col.clone()); vals.push(v); }
                RefRes::Missing => {
                    if force && !notnull.contains(col) { cols.push(col.clone()); vals.push(Value::Null); }
                    else if force { return Ok(Some(false)); } // required FK absent → skip row
                    else { return Ok(None); } // defer
                }
            }
            continue;
        }
        // Polymorphic id.
        if let Some((kind_col, id_col)) = spec.poly {
            if col == id_col {
                let kind = ch.fields.get(kind_col).and_then(|v| v.as_str());
                let ref_table = kind.and_then(schema::kind_to_table);
                match ref_table {
                    Some(t) => match resolve_ref(conn, t, jval)? {
                        RefRes::Ok(v) => { cols.push(col.clone()); vals.push(v); }
                        RefRes::Missing => {
                            if force { return Ok(Some(false)); } else { return Ok(None); }
                        }
                    },
                    None => return Ok(Some(false)), // unsynced kind (e.g. goal) → skip
                }
                continue;
            }
        }
        cols.push(col.clone());
        vals.push(json_to_value(jval));
    }

    // Append sync bookkeeping columns.
    cols.push("uuid".into());
    vals.push(Value::Text(ch.uuid.clone()));
    cols.push("updated_hlc".into());
    vals.push(Value::Text(ch.hlc.clone()));
    cols.push("dirty".into());
    vals.push(Value::Integer(0));

    if exists {
        let set: Vec<String> = cols.iter().enumerate().map(|(i, c)| format!("{c} = ?{}", i + 1)).collect();
        let sql = format!(
            "UPDATE {table} SET {} WHERE uuid = ?{}",
            set.join(", "),
            cols.len() + 1
        );
        let mut binds = vals.clone();
        binds.push(Value::Text(ch.uuid.clone()));
        conn.execute(&sql, rusqlite::params_from_iter(binds.iter()))?;
    } else {
        let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            cols.join(", "),
            placeholders.join(", ")
        );
        conn.execute(&sql, rusqlite::params_from_iter(vals.iter()))?;
    }
    Ok(Some(true))
}

enum RefRes {
    Ok(Value),
    Missing,
}

/// Resolve a wire FK value (a referenced uuid string, or null) to a local id Value.
fn resolve_ref(conn: &Connection, ref_table: &str, jval: &Json) -> Result<RefRes> {
    match jval {
        Json::Null => Ok(RefRes::Ok(Value::Null)),
        Json::String(uuid) => match id_of(conn, ref_table, uuid)? {
            Some(id) => Ok(RefRes::Ok(Value::Integer(id))),
            None => Ok(RefRes::Missing),
        },
        _ => Ok(RefRes::Ok(Value::Null)),
    }
}

/// Upserts sorted by table dependency order (the order tables appear in [`schema::TABLES`]).
fn order_upserts(changes: &[Change]) -> Vec<&Change> {
    let mut v: Vec<&Change> = changes.iter().filter(|c| c.op == Op::Upsert).collect();
    let rank = |t: &str| schema::TABLES.iter().position(|s| s.name == t).unwrap_or(usize::MAX);
    v.sort_by_key(|c| rank(&c.table));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use serde_json::json;

    fn uuid_col(conn: &Connection, table: &str, id: i64) -> String {
        conn.query_row(&format!("SELECT uuid FROM {table} WHERE id = ?1"), params![id], |r| r.get(0))
            .unwrap()
    }
    fn scalar_i64(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }
    fn insert_project(conn: &Connection, name: &str) -> i64 {
        conn.execute("INSERT INTO projects(name, color, created_at) VALUES(?1, '#fff', 't')", params![name]).unwrap();
        conn.last_insert_rowid()
    }
    fn insert_task(conn: &Connection, title: &str, project_id: Option<i64>) -> i64 {
        conn.execute(
            "INSERT INTO tasks(title, project_id, created_at) VALUES(?1, ?2, 't')",
            params![title, project_id],
        ).unwrap();
        conn.last_insert_rowid()
    }
    fn insert_note(conn: &Connection, content: &str, parent: Option<i64>) -> i64 {
        conn.execute(
            "INSERT INTO notes(content, parent_id, created_at, updated_at) VALUES(?1, ?2, 't', 't')",
            params![content, parent],
        ).unwrap();
        conn.last_insert_rowid()
    }

    // ---- migration + trigger behavior ----

    #[test]
    fn insert_stamps_uuid_and_dirty_update_redirties_delete_tombstones() {
        let c = db::test_conn();
        let tid = insert_task(&c, "T", None);
        let (uuid, dirty): (Option<String>, i64) = c
            .query_row("SELECT uuid, dirty FROM tasks WHERE id = ?1", params![tid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert!(uuid.is_some(), "insert trigger should stamp a uuid");
        assert_eq!(dirty, 1, "new rows start dirty");

        // Building the outbox clears dirty.
        let mut clock = HlcState::default();
        build_outbox(&c, "A", &mut clock, 1000).unwrap();
        assert_eq!(scalar_i64(&c, "SELECT dirty FROM tasks"), 0);

        // A normal UPDATE re-dirties.
        c.execute("UPDATE tasks SET title = 'T2' WHERE id = ?1", params![tid]).unwrap();
        assert_eq!(scalar_i64(&c, "SELECT dirty FROM tasks"), 1);

        // A DELETE leaves a tombstone.
        let u = uuid.unwrap();
        c.execute("DELETE FROM tasks WHERE id = ?1", params![tid]).unwrap();
        assert_eq!(
            scalar_i64(&c, "SELECT count(*) FROM sync_tombstones WHERE entity_table = 'tasks'"),
            1
        );
        let tomb_uuid: String = c
            .query_row("SELECT entity_uuid FROM sync_tombstones WHERE entity_table='tasks'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tomb_uuid, u);
    }

    #[test]
    fn capture_suppression_prevents_redirty() {
        let c = db::test_conn();
        let tid = insert_task(&c, "T", None);
        let mut clock = HlcState::default();
        build_outbox(&c, "A", &mut clock, 1000).unwrap();
        // An UPDATE inside with_capture_suppressed must NOT set dirty.
        super::super::with_capture_suppressed(|| {
            c.execute("UPDATE tasks SET title = 'inner' WHERE id = ?1", params![tid]).unwrap();
        });
        assert_eq!(scalar_i64(&c, "SELECT dirty FROM tasks"), 0);
    }

    // ---- build translates foreign keys to uuids ----

    #[test]
    fn build_rewrites_fk_as_referenced_uuid() {
        let c = db::test_conn();
        let pid = insert_project(&c, "Proj");
        let proj_uuid = uuid_col(&c, "projects", pid);
        insert_task(&c, "T", Some(pid));

        let mut clock = HlcState::default();
        let changes = build_outbox(&c, "A", &mut clock, 1000).unwrap();
        let task = changes.iter().find(|ch| ch.table == "tasks").unwrap();
        assert_eq!(task.op, Op::Upsert);
        assert_eq!(task.fields["project_id"], json!(proj_uuid));
        assert!(!task.fields.contains_key("id"), "local id must never go on the wire");
        assert!(!task.fields.contains_key("uuid"));
    }

    // ---- the central convergence property ----

    /// Copy every change produced on `from` into `to`. Returns the changes (so callers can assert).
    fn sync_one_way(from: &Connection, from_node: &str, from_clock: &mut HlcState,
                    to: &Connection, to_clock: &mut HlcState, now: u64) -> Vec<Change> {
        let changes = build_outbox(from, from_node, from_clock, now).unwrap();
        apply_changes(to, to_clock, now, &changes).unwrap();
        changes
    }

    #[test]
    fn two_devices_converge_with_fks_and_polymorphic_links() {
        let a = db::test_conn();
        let b = db::test_conn();
        let (mut ca, mut cb) = (HlcState::default(), HlcState::default());

        // On A: a project, a task in it, a parent+child note (self-ref FK), a label, and a
        // polymorphic entity_label linking the label to the task.
        let pid = insert_project(&a, "Work");
        let tid = insert_task(&a, "Write report", Some(pid));
        let parent = insert_note(&a, "Parent page", None);
        let _child = insert_note(&a, "Child page", Some(parent));
        a.execute("INSERT INTO labels(name, color, created_at) VALUES('deep', '#000', 't')", []).unwrap();
        let lid = a.last_insert_rowid();
        a.execute(
            "INSERT INTO entity_labels(label_id, entity_kind, entity_id) VALUES(?1, 'task', ?2)",
            params![lid, tid],
        ).unwrap();

        let task_uuid = uuid_col(&a, "tasks", tid);
        let label_uuid = uuid_col(&a, "labels", lid);

        let sent = sync_one_way(&a, "A", &mut ca, &b, &mut cb, 1000);
        let watermark = sent.iter().map(|c| c.hlc.clone()).max().unwrap_or_default();

        // B has the task, with project_id resolved to B's LOCAL project id.
        let (b_title, b_pid): (String, Option<i64>) = b
            .query_row("SELECT title, project_id FROM tasks WHERE uuid = ?1", params![task_uuid], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(b_title, "Write report");
        let b_proj_id: i64 = b.query_row("SELECT id FROM projects WHERE name = 'Work'", [], |r| r.get(0)).unwrap();
        assert_eq!(b_pid, Some(b_proj_id));

        // The self-referential child note resolved its parent locally.
        let b_child_parent: Option<i64> = b
            .query_row("SELECT parent_id FROM notes WHERE content = 'Child page'", [], |r| r.get(0))
            .unwrap();
        let b_parent_id: i64 = b.query_row("SELECT id FROM notes WHERE content = 'Parent page'", [], |r| r.get(0)).unwrap();
        assert_eq!(b_child_parent, Some(b_parent_id));

        // The polymorphic label link resolved BOTH the label_id and the entity_id locally.
        let b_label_id: i64 = b.query_row("SELECT id FROM labels WHERE uuid = ?1", params![label_uuid], |r| r.get(0)).unwrap();
        let b_task_id: i64 = b.query_row("SELECT id FROM tasks WHERE uuid = ?1", params![task_uuid], |r| r.get(0)).unwrap();
        let (el_label, el_kind, el_entity): (i64, String, i64) = b
            .query_row("SELECT label_id, entity_kind, entity_id FROM entity_labels", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        assert_eq!((el_label, el_kind.as_str(), el_entity), (b_label_id, "task", b_task_id));

        // Echo prevention: B has nothing NEW past the watermark it just received (and no local
        // edits made anything dirty), so a delta pull returns empty.
        stamp_dirty(&b, "B", &mut cb, 1100).unwrap();
        let echo = changes_since(&b, &watermark).unwrap();
        assert!(echo.is_empty(), "applied rows must not re-ship past the watermark");
    }

    #[test]
    fn last_writer_wins_and_delete_propagates() {
        let a = db::test_conn();
        let b = db::test_conn();
        let (mut ca, mut cb) = (HlcState::default(), HlcState::default());

        let tid = insert_task(&a, "Original", None);
        let task_uuid = uuid_col(&a, "tasks", tid);
        sync_one_way(&a, "A", &mut ca, &b, &mut cb, 1000);

        // B edits the task later (higher HLC); push B→A; A must adopt B's value.
        b.execute("UPDATE tasks SET title = 'B wins' WHERE uuid = ?1", params![task_uuid]).unwrap();
        sync_one_way(&b, "B", &mut cb, &a, &mut ca, 2000);
        let a_title: String = a.query_row("SELECT title FROM tasks WHERE uuid = ?1", params![task_uuid], |r| r.get(0)).unwrap();
        assert_eq!(a_title, "B wins");

        // A stale write loses: craft an upsert with an older HLC than what A holds.
        let stale = Change {
            table: "tasks".into(),
            uuid: task_uuid.clone(),
            op: Op::Upsert,
            hlc: hlc::encode(1, 0, "Z"),
            fields: {
                let mut m = Map::new();
                m.insert("title".into(), json!("stale"));
                m.insert("notes".into(), json!(""));
                m.insert("estimated_minutes".into(), json!(30));
                m.insert("priority".into(), json!(2));
                m.insert("min_chunk_minutes".into(), json!(30));
                m.insert("max_chunk_minutes".into(), json!(120));
                m.insert("status".into(), json!("todo"));
                m.insert("created_at".into(), json!("t"));
                m.insert("project_id".into(), Json::Null);
                m.insert("deadline".into(), Json::Null);
                m.insert("earliest_start".into(), Json::Null);
                m
            },
        };
        apply_changes(&a, &mut ca, 2100, &[stale]).unwrap();
        let a_title: String = a.query_row("SELECT title FROM tasks WHERE uuid = ?1", params![task_uuid], |r| r.get(0)).unwrap();
        assert_eq!(a_title, "B wins", "older HLC must not overwrite");

        // Delete on A propagates to B.
        a.execute("DELETE FROM tasks WHERE uuid = ?1", params![task_uuid]).unwrap();
        sync_one_way(&a, "A", &mut ca, &b, &mut cb, 3000);
        let b_count: i64 = b
            .query_row("SELECT count(*) FROM tasks WHERE uuid = ?1", params![task_uuid], |r| r.get(0))
            .unwrap();
        assert_eq!(b_count, 0, "delete must propagate");
    }
}
