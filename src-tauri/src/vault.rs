//! Two-way markdown vault: mirror vault pages to `.md` files in the user's chosen folder so they're
//! editable in Pushin OR any external editor and visible in the file manager.
//!
//! This module is the file-side plumbing. The *rule-based folder path* (Daily/by-date, Events/by-date,
//! the page tree → nested folders) is computed in the frontend (it has the page tree + entity links),
//! which passes the `rel_path` here; Rust just reads/writes bytes and (later) watches the folder. SQLite
//! stays the source of truth; `notes.rel_path` maps a page to its file.

use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Per-page hash of the last bytes Pushin itself wrote to a file, keyed by `rel_path`. The watcher
/// skips any file event whose content hashes to the stored value, so in-app saves (which write the
/// file) don't echo back through the watcher and re-update the DB. Shared with `vault_write`.
pub type EchoGuard = Arc<Mutex<HashMap<String, u64>>>;

/// FNV-1a 64-bit — same cheap stable hash the Context Engine uses for `text_hash`.
pub fn content_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Resolve a vault-relative path to an absolute path *inside* the vault, rejecting traversal
/// (`..`, absolute components) so a bad `rel_path` can never escape the vault folder.
pub fn safe_join(vault_dir: &str, rel_path: &str) -> Option<PathBuf> {
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return None;
    }
    let mut out = PathBuf::from(vault_dir);
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            _ => return None, // ParentDir / RootDir / Prefix → reject
        }
    }
    Some(out)
}

/// Write a page's markdown to `<vault>/<rel_path>`, creating parent folders. No-op-safe re-write.
pub fn write_file(vault_dir: &str, rel_path: &str, markdown: &str) -> Result<()> {
    let abs = safe_join(vault_dir, rel_path).ok_or_else(|| anyhow::anyhow!("unsafe vault path: {rel_path}"))?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&abs, markdown)?;
    Ok(())
}

/// Read a vault file's markdown (used by the file→DB watcher path).
#[allow(dead_code)] // used by the Phase 3e files→DB watcher
pub fn read_file(vault_dir: &str, rel_path: &str) -> Result<String> {
    let abs = safe_join(vault_dir, rel_path).ok_or_else(|| anyhow::anyhow!("unsafe vault path: {rel_path}"))?;
    Ok(std::fs::read_to_string(abs)?)
}

/// A change the watcher saw on disk, forwarded to the frontend (which owns md→blocks). `kind` is
/// "update" (create/modify) or "remove".
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultChange {
    pub rel_path: String,
    pub content: String,
    pub kind: String,
}

/// Holds the live OS watcher; dropping it stops watching (so swapping/clearing the vault folder is
/// just replacing this in `AppState`).
pub struct VaultWatcher {
    _watcher: RecommendedWatcher,
}

/// Watch `vault_dir` recursively and emit a Tauri `vault-changed` event for every `.md` create/modify/
/// delete — except files Pushin just wrote (the echo guard). The frontend converts markdown→blocks and
/// upserts the page matched by `rel_path`. Best-effort and resilient: unreadable/mid-write files are
/// skipped (a later event catches the settled content).
pub fn start_watch(vault_dir: &str, app: AppHandle, echo: EchoGuard) -> Result<VaultWatcher> {
    let root = PathBuf::from(vault_dir);
    let root_for_handler = root.clone();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
            _ => return,
        }
        let removal = matches!(event.kind, EventKind::Remove(_));
        for path in event.paths {
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root_for_handler) else { continue };
            let rel_path = rel.to_string_lossy().replace('\\', "/");

            // A Remove event, or a path that no longer exists (e.g. the temp side of an atomic save).
            if removal || !path.exists() {
                let _ = app.emit(
                    "vault-changed",
                    VaultChange { rel_path, content: String::new(), kind: "remove".into() },
                );
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            // Echo guard: ignore the file we just wrote ourselves (matching content hash).
            if echo.lock().ok().and_then(|g| g.get(&rel_path).copied()) == Some(content_hash(&content)) {
                continue;
            }
            let _ = app.emit("vault-changed", VaultChange { rel_path, content, kind: "update".into() });
        }
    })?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    Ok(VaultWatcher { _watcher: watcher })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_stable_and_differs() {
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("world"));
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute() {
        assert!(safe_join("/vault", "Daily/x.md").is_some());
        assert!(safe_join("/vault", "../escape.md").is_none());
        assert!(safe_join("/vault", "/etc/passwd").is_none());
    }
}
