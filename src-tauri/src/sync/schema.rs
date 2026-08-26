//! The registry of synced tables + the generated `0015_sync` migration.
//!
//! ONE source of truth: [`TABLES`] drives both the migration (which columns/triggers each table
//! gets) and the changeset translation (which columns are foreign keys to rewrite as uuids). Adding
//! a table to sync = adding one [`TableSpec`] entry.

/// How one table participates in sync.
pub struct TableSpec {
    /// SQL table name.
    pub name: &'static str,
    /// Foreign-key columns: `(column, referenced_table)`. On the wire the integer id is replaced by
    /// the referenced row's `uuid`; on apply it's resolved back to a local id.
    pub fks: &'static [(&'static str, &'static str)],
    /// A polymorphic reference `(kind_column, id_column)` — `id_column` points at a row of whatever
    /// table `kind_column` names (see [`kind_to_table`]). Mirrors `entity_links`/`entity_labels`.
    pub poly: Option<(&'static str, &'static str)>,
    /// Columns excluded from the wire payload: device-local plumbing + re-derivable vectors.
    pub skip: &'static [&'static str],
    /// Wire-only fields backed by the OS keychain, as `(wire_field, keychain_key)`. They are read
    /// from [`crate::secrets`] when building a payload and written straight back to it on apply —
    /// they never touch a SQLite column. Used to carry the Google refresh token between a user's
    /// own devices without ever persisting it in plaintext.
    pub secrets: &'static [(&'static str, &'static str)],
    /// The `user_version` whose migration CREATES this table. [`migration_sql`] is the frozen `0015`
    /// migration, so it must only touch tables that already existed then; anything added later
    /// (`google_link`) carries its own `ALTER`s via [`table_sync_sql`] from its own migration.
    pub added_in: u32,
}

/// The schema version at which device sync was introduced — the cutoff [`migration_sql`] emits for.
pub const SYNC_MIGRATION_VERSION: u32 = 15;

/// All synced tables, in dependency order (parents before children) so apply resolves FKs in one
/// forward pass for the common case; remaining cycles (e.g. `notes.parent_id` self-ref) are handled
/// by the deferred-fixup loop in `changeset::apply_changes`.
pub const TABLES: &[TableSpec] = &[
    // The shared Google Calendar link: one row describing the connected account + the user's OAuth
    // client, so pairing a new device auto-applies the Google setup instead of re-running consent.
    // The refresh token rides along as a keychain-backed secret field (never a SQLite column); the
    // per-device plumbing (access token, expiry, `sync_token` pull cursor) stays in
    // `calendar_accounts` and is deliberately NOT synced.
    TableSpec { name: "google_link", fks: &[], poly: None, skip: &[],
        secrets: &[("refresh_token", crate::db::KC_REFRESH)], added_in: 20 },
    // Read-only .ics feed subscriptions. The user's decision is that importing a feed on one device
    // shows up on all of them, so the SUBSCRIPTION replicates. `last_synced` does not: every device
    // fetches the feed on its own schedule, so it is a per-device fact like Google's `sync_token`.
    //
    // The feed's mirrored EVENTS are deliberately NOT synced (see the `events` spec below).
    TableSpec { name: "ics_subscriptions", fks: &[], poly: None, skip: &["last_synced"],
        secrets: &[], added_in: 22 },
    TableSpec { name: "projects",    fks: &[], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "labels",      fks: &[], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "people",      fks: &[], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "event_types", fks: &[], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "habits",      fks: &[], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    // `provider`/`external_id` DO sync: they are the Google event's global identity, and without
    // them a peer would see a synced event as "never pushed" and insert a duplicate into Google
    // (see `calendar::google::sync`). `account_id` is a local FK and `etag` a per-device
    // concurrency token, so both stay device-local.
    // `ics_sub_id` stays off the wire even though `ics_subscriptions` now syncs, because the feed's
    // mirrored events do not replicate — each device re-derives them from the URL. Two reasons:
    // `db::replace_ics_events` refreshes a feed by DELETING every one of its events and re-inserting
    // them, so replicating those rows would have two devices tombstoning and re-creating each other's
    // copies on every refresh (the churn `plan_block_mirror` exists to avoid for Google blocks); and
    // the events are re-derivable from the feed, like embeddings, so shipping them buys nothing.
    // Historically this column went on the wire as a raw LOCAL rowid, which failed the FK check on a
    // peer without that feed and aborted the ENTIRE batch — one .ics feed blocked all sync (9ae15ce).
    TableSpec { name: "events", fks: &[], poly: None,
        skip: &["account_id", "etag", "ics_sub_id"], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    // Embeddings are re-derived locally; never ship 384-dim vectors.
    TableSpec { name: "notes", fks: &[("parent_id", "notes")], poly: None,
        skip: &["embedding", "embedding_model"], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "tasks", fks: &[("project_id", "projects")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "bookings",
        fks: &[("event_type_id", "event_types"), ("event_id", "events")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "task_deps",
        fks: &[("task_id", "tasks"), ("depends_on_task_id", "tasks")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    // Blocks (scheduler output) sync as plain LWW too — the Google plumbing columns stay local.
    // We don't auto-reschedule on apply, so there's no cross-device scheduler ping-pong.
    TableSpec { name: "blocks", fks: &[("task_id", "tasks")], poly: None,
        skip: &["provider", "external_id", "sync_state"], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "habit_logs", fks: &[("habit_id", "habits")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "focus_sessions", fks: &[("task_id", "tasks")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "page_links",
        fks: &[("source_id", "notes"), ("target_id", "notes")], poly: None, skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "entity_links", fks: &[("page_id", "notes")],
        poly: Some(("entity_kind", "entity_id")), skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
    TableSpec { name: "entity_labels", fks: &[("label_id", "labels")],
        poly: Some(("entity_kind", "entity_id")), skip: &[], secrets: &[], added_in: SYNC_MIGRATION_VERSION },
];

/// Columns that are sync-internal or local and never appear in a payload's field set.
pub const INTERNAL_COLS: &[&str] = &["id", "uuid", "updated_hlc", "dirty"];

/// Map a polymorphic `entity_kind` value to the table it references. `None` = a kind with no synced
/// table (e.g. `goal`), whose links we skip.
pub fn kind_to_table(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "task" => "tasks",
        "event" => "events",
        "habit" => "habits",
        "page" => "notes",
        "project" => "projects",
        "person" => "people",
        _ => return None,
    })
}

/// Look up a table's spec by name.
pub fn spec(name: &str) -> Option<&'static TableSpec> {
    TABLES.iter().find(|t| t.name == name)
}

/// Build the full `0015_sync` migration SQL from [`TABLES`]. Generated (not hand-written) so the
/// table list can't drift between the migration and the changeset logic.
///
/// NOTE: SQLite forbids *expression* defaults in `ALTER TABLE ADD COLUMN`, so `uuid` is added
/// nullable, existing rows are backfilled here, and new rows get a uuid from an `AFTER INSERT`
/// trigger — no app code has to set it.
pub fn migration_sql() -> String {
    let mut s = String::new();

    // Sync bookkeeping tables.
    s.push_str(
        "CREATE TABLE IF NOT EXISTS sync_tombstones (\n  \
            entity_table TEXT NOT NULL,\n  \
            entity_uuid  TEXT NOT NULL,\n  \
            hlc          TEXT NOT NULL DEFAULT '',\n  \
            dirty        INTEGER NOT NULL DEFAULT 1,\n  \
            PRIMARY KEY (entity_table, entity_uuid)\n);\n\
        CREATE TABLE IF NOT EXISTS sync_peers (\n  \
            node_id        TEXT PRIMARY KEY,\n  \
            name           TEXT NOT NULL DEFAULT '',\n  \
            last_seen      TEXT,\n  \
            last_acked_hlc TEXT NOT NULL DEFAULT ''\n);\n\
        CREATE TABLE IF NOT EXISTS sync_self (\n  \
            k TEXT PRIMARY KEY,\n  v TEXT NOT NULL\n);\n",
    );

    // Only the tables that existed at 0015 — a later arrival (google_link, 0020) applies its own
    // `table_sync_sql` from its own migration, and does not exist yet when this runs on a fresh DB.
    for t in TABLES.iter().filter(|t| t.added_in <= SYNC_MIGRATION_VERSION) {
        s.push_str(&table_sync_sql(t));
    }

    s
}

/// The sync columns, backfill, unique index and capture triggers for ONE table.
///
/// Split out of [`migration_sql`] because `0015` only ever runs once: a table added to [`TABLES`]
/// afterwards (e.g. `google_link` in `0020`) needs the same treatment applied by its own migration.
pub fn table_sync_sql(t: &TableSpec) -> String {
    let mut s = String::new();
    let n = t.name;
    // Columns. dirty defaults to 1 so backfilled + freshly-inserted rows start "needs push".
    s.push_str(&format!("ALTER TABLE {n} ADD COLUMN uuid TEXT;\n"));
    s.push_str(&format!("ALTER TABLE {n} ADD COLUMN updated_hlc TEXT;\n"));
    s.push_str(&format!("ALTER TABLE {n} ADD COLUMN dirty INTEGER NOT NULL DEFAULT 1;\n"));
    // Backfill global ids for existing rows (constant-expression UPDATE is fine post-ALTER).
    s.push_str(&format!(
        "UPDATE {n} SET uuid = lower(hex(randomblob(16))) WHERE uuid IS NULL;\n"
    ));
    s.push_str(&format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_{n}_uuid ON {n}(uuid);\n"
    ));
    // AFTER INSERT: stamp a uuid on any new row that doesn't have one (zero app changes).
    s.push_str(&format!(
        "CREATE TRIGGER IF NOT EXISTS trg_{n}_sync_ins AFTER INSERT ON {n} \
         WHEN NEW.uuid IS NULL BEGIN \
         UPDATE {n} SET uuid = lower(hex(randomblob(16))) WHERE rowid = NEW.rowid; END;\n"
    ));
    // AFTER UPDATE: mark dirty so the next sync ships it. Guarded by sync_capturing() so our own
    // apply/build writes don't re-dirty (echo prevention). recursive_triggers is OFF, so this
    // inner UPDATE doesn't re-fire the trigger.
    s.push_str(&format!(
        "CREATE TRIGGER IF NOT EXISTS trg_{n}_sync_upd AFTER UPDATE ON {n} \
         WHEN sync_capturing() = 1 BEGIN \
         UPDATE {n} SET dirty = 1 WHERE rowid = NEW.rowid; END;\n"
    ));
    // AFTER DELETE: record a tombstone so the delete propagates (hard DELETE leaves no trace).
    s.push_str(&format!(
        "CREATE TRIGGER IF NOT EXISTS trg_{n}_sync_del AFTER DELETE ON {n} \
         WHEN sync_capturing() = 1 AND OLD.uuid IS NOT NULL BEGIN \
         INSERT INTO sync_tombstones(entity_table, entity_uuid, hlc, dirty) \
         VALUES('{n}', OLD.uuid, '', 1) \
         ON CONFLICT(entity_table, entity_uuid) DO UPDATE SET dirty = 1; END;\n"
    ));

    s
}
