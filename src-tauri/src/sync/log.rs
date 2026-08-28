//! A small in-memory ring of what sync actually did, readable from inside the app.
//!
//! Every diagnostic in `sync/` used to be an `eprintln!`. That is fine under `npm run tauri dev`,
//! where stderr goes to the terminal — and worth nothing on an installed build, which has no console
//! attached. Since the failures worth debugging are cross-device by definition, the device that
//! misbehaves is usually the one you cannot see a terminal for.
//!
//! So: keep the `eprintln!` for dev, and also push the same line here, where **Settings ▸ Devices ▸
//! Diagnostics** can show it and the user can copy it out.
//!
//! Deliberately a ring, not a file. It survives long enough to explain the session that just failed,
//! costs nothing when nothing is wrong, and cannot grow without bound on a device left running for a
//! month. Nothing here is load-bearing: a poisoned lock is swallowed, because a broken log must
//! never be the thing that breaks sync.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// How many lines to keep. A session logs a handful, so this is comfortably more than one bad
/// evening of 20-second retries.
const CAP: usize = 300;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncLogLine {
    /// Local wall-clock `HH:MM:SS` — enough to line two devices' logs up against each other.
    pub at: String,
    /// `"info"` | `"warn"` | `"error"`.
    pub level: &'static str,
    pub text: String,
}

fn ring() -> &'static Mutex<VecDeque<SyncLogLine>> {
    static RING: OnceLock<Mutex<VecDeque<SyncLogLine>>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAP)))
}

/// Record one line. Also mirrors to stderr so `tauri dev` keeps working the way it did.
pub fn note(level: &'static str, text: impl Into<String>) {
    let text = text.into();
    eprintln!("sync[{level}]: {text}");
    let line = SyncLogLine {
        at: chrono::Local::now().format("%H:%M:%S").to_string(),
        level,
        text,
    };
    if let Ok(mut r) = ring().lock() {
        if r.len() == CAP {
            r.pop_front();
        }
        r.push_back(line);
    }
}

pub fn info(text: impl Into<String>) {
    note("info", text)
}
pub fn warn(text: impl Into<String>) {
    note("warn", text)
}
pub fn error(text: impl Into<String>) {
    note("error", text)
}

/// Everything currently held, oldest first — what the Diagnostics panel renders and the copy button
/// puts on the clipboard.
pub fn lines() -> Vec<SyncLogLine> {
    ring().lock().map(|r| r.iter().cloned().collect()).unwrap_or_default()
}

/// Drop everything, so a user can clear the noise and reproduce cleanly.
pub fn clear() {
    if let Ok(mut r) = ring().lock() {
        r.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ring is process-global, and the suite runs tests on parallel threads — so these two must
    /// not interleave, or one's `clear()` lands in the middle of the other's assertions.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn lines_come_back_oldest_first_with_their_level() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear();
        info("started");
        warn("something odd");
        error("gave up");
        let got = lines();
        assert_eq!(
            got.iter().map(|l| (l.level, l.text.as_str())).collect::<Vec<_>>(),
            vec![("info", "started"), ("warn", "something odd"), ("error", "gave up")],
        );
        assert!(got[0].at.len() == 8, "HH:MM:SS, so two devices' logs line up: {:?}", got[0].at);
        clear();
    }

    #[test]
    fn the_ring_drops_the_oldest_rather_than_growing_without_bound() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // A device left running for a month retries every 20 seconds. The log has to be bounded, and
        // it has to keep the RECENT end — the failure you are looking at happened just now.
        clear();
        for i in 0..(CAP + 25) {
            info(format!("line {i}"));
        }
        let got = lines();
        assert_eq!(got.len(), CAP);
        assert_eq!(got.first().unwrap().text, format!("line {}", 25));
        assert_eq!(got.last().unwrap().text, format!("line {}", CAP + 24));
        clear();
    }
}
