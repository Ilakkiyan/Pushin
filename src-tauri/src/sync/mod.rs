//! Device-to-device sync: a private peer-to-peer mesh (Iroh transport) carrying a custom changeset
//! log over the local SQLite, which stays the source of truth. See `virtual-noodling-hoare` plan.
//!
//! - [`schema`]   — the synced-table registry + the generated `0015_sync` migration.
//! - [`hlc`]      — Hybrid Logical Clock for last-writer-wins ordering across devices.
//! - [`changeset`] — read local changes → wire payloads (FK→uuid) and apply remote ones (LWW).
//!
//! Change capture is done with SQLite triggers (see [`schema`]), so the ~50 existing `db.rs`
//! mutation functions are untouched. The triggers consult [`sync_capturing`] so that our OWN writes
//! (building the outbox / applying remote changes) don't re-mark rows dirty — that would echo
//! forever. All DB access is serialized behind the app's `Mutex<Connection>`, so a process-global
//! flag is sufficient and race-free.

pub mod changeset;
pub mod engine;
pub mod hlc;
pub mod identity;
pub mod protocol;
pub mod schema;
pub mod state;
pub mod transport;

use rusqlite::functions::FunctionFlags;
use rusqlite::Connection;
use std::cell::Cell;

thread_local! {
    /// Whether trigger-based change capture is active on THIS thread. Default ON. A SQLite trigger
    /// always runs inline on the same thread as the write that fired it, so a thread-local exactly
    /// scopes suppression to the build/apply write in progress — and keeps parallel test threads
    /// (each with their own in-memory DB) from clobbering each other's flag.
    static CAPTURING: Cell<bool> = const { Cell::new(true) };
}

/// Register the `sync_capturing()` SQL function used by the change-capture triggers. Must be called
/// on every connection (app + tests) before any captured write runs.
pub fn register_functions(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "sync_capturing",
        0,
        FunctionFlags::SQLITE_UTF8,
        |_ctx| Ok(if CAPTURING.with(|c| c.get()) { 1i64 } else { 0i64 }),
    )
}

/// Run `f` with change-capture suppressed (the sync engine's build/apply path). Restores the prior
/// state afterward. Callers must already hold the DB lock; `f` must do its DB work synchronously on
/// the calling thread (no `.await` inside).
pub fn with_capture_suppressed<T>(f: impl FnOnce() -> T) -> T {
    let prev = CAPTURING.with(|c| c.replace(false));
    let out = f();
    CAPTURING.with(|c| c.set(prev));
    out
}
