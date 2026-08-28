//! The one shape every kind of sync reports itself with, so the UI has a single thing to render.
//!
//! Two very different engines feed this: the device mesh (rows, then file bytes) and Google Calendar
//! (pull, push, mirror). They are emitted on the same `sync-progress` event so the sidebar bar does
//! not have to know which one is running — it renders whatever arrived last and hides itself when a
//! `done` lands.
//!
//! `total == 0` means *indeterminate*: work is happening but its size is not known yet (the Google
//! pull, before we know how many events came back). The bar shows motion without a number rather
//! than inventing a percentage — a fake number that jumps to 90% and sits there is worse than no
//! number, because it teaches the user not to trust the bar.

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// The event name the frontend listens on.
pub const EVENT: &str = "sync-progress";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    /// Which engine: `"device"` or `"google"`.
    pub source: &'static str,
    /// What it is doing right now: `"rows"`, `"files"`, `"pull"`, `"push"`, `"mirror"`, `"done"`.
    pub phase: &'static str,
    /// Human label for the bar — a peer's device name, or "Google Calendar".
    pub label: String,
    /// Units finished and total units, in whatever unit the phase counts (bytes for files, items
    /// otherwise). `total == 0` = indeterminate.
    pub done: u64,
    pub total: u64,
    /// `false` on the final event of a run, which is what retires the bar.
    pub active: bool,
}

impl SyncProgress {
    pub fn new(source: &'static str, phase: &'static str, label: impl Into<String>) -> Self {
        SyncProgress { source, phase, label: label.into(), done: 0, total: 0, active: true }
    }
    pub fn at(mut self, done: u64, total: u64) -> Self {
        self.done = done;
        self.total = total;
        self
    }
    /// The closing event: the bar hides on this.
    pub fn finished(source: &'static str, label: impl Into<String>) -> Self {
        SyncProgress { source, phase: "done", label: label.into(), done: 0, total: 0, active: false }
    }
}

/// Emit one progress update. Best-effort by design — a dropped progress frame must never fail or
/// slow the sync it is describing, and the next frame (or the closing `done`) corrects the UI.
pub fn emit(app: &AppHandle, p: SyncProgress) {
    let _ = app.emit(EVENT, p);
}
