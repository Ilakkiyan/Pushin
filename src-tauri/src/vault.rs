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

/// The largest file the vault will replicate to another device. Above this the file stays local:
/// the whole batch travels over one QUIC session held in memory, and a multi-gigabyte video would
/// stall every other change behind it. Skipped files are simply not indexed, so they never appear
/// in a peer's want-list.
pub const MAX_SYNC_FILE: u64 = 100 * 1024 * 1024;

/// One file found by [`scan_files`], with everything the index needs to decide whether it changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedFile {
    /// Vault-relative path, always `/`-separated (the wire form — a Windows backslash would not
    /// match the same file on a peer's mac).
    pub rel_path: String,
    /// SHA-256 of the bytes, lowercase hex. `None` until [`hash_file`] runs — a scan that can reuse
    /// the previous hash (same mtime + size) leaves it unset rather than re-reading the bytes.
    pub hash: Option<String>,
    pub size: u64,
    /// Modification time as whole seconds since the epoch; `0` when the platform won't say.
    pub mtime: i64,
}

/// SHA-256 of a file's contents, lowercase hex — the content address a peer asks for a blob by.
/// Streamed in 64 KB reads so hashing a 100 MB attachment doesn't materialise it in memory.
pub fn hash_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of bytes already in memory — the check a received blob has to pass before it is allowed
/// to land in the vault.
pub fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Is this a path the vault should never replicate?
///
/// Dot-files and dot-folders are editor/OS bookkeeping (`.obsidian/`, `.git/`, `.DS_Store`) — local
/// state that means nothing on another machine. `pages` holds the `rel_path` of every page mirror:
/// those files are re-derived from a `notes` row on each device, so replicating the file as well
/// would put two writers on one path (see the `vault_file_index` TableSpec).
pub fn is_excluded(rel_path: &str, pages: &std::collections::HashSet<String>) -> bool {
    if pages.contains(rel_path) {
        return true;
    }
    rel_path.split('/').any(|c| c.starts_with('.'))
}

/// Walk `vault_dir` and return every file eligible for sync.
///
/// Deliberately does NOT hash: hashing is the expensive half, and the caller skips it for files
/// whose `(mtime, size)` match what it saw last time. Symlinks are not followed — `symlink_metadata`
/// keeps a link that points back at an ancestor from walking forever, and a link's *target* is
/// either already inside the vault (so it gets visited on its own) or outside it (so it is not
/// vault content). Unreadable entries are skipped rather than failing the scan: one
/// permission-denied folder must not stop the other 300 files from syncing.
pub fn scan_files(vault_dir: &str, pages: &std::collections::HashSet<String>) -> Vec<ScannedFile> {
    let root = PathBuf::from(vault_dir);
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.file_type().is_symlink() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&root) else { continue };
            let rel_path = rel.to_string_lossy().replace('\\', "/");
            if rel_path.is_empty() || is_excluded(&rel_path, pages) {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && meta.len() <= MAX_SYNC_FILE {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.push(ScannedFile { rel_path, hash: None, size: meta.len(), mtime });
            }
        }
    }
    // Stable order so a scan is reproducible and its tests can assert on it.
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// Write bytes to `<vault>/<rel_path>`, creating parent folders. The binary counterpart of
/// [`write_file`], used when a peer's attachment lands.
pub fn write_bytes(vault_dir: &str, rel_path: &str, bytes: &[u8]) -> Result<()> {
    let abs = safe_join(vault_dir, rel_path).ok_or_else(|| anyhow::anyhow!("unsafe vault path: {rel_path}"))?;
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&abs, bytes)?;
    Ok(())
}

/// Read a vault file's raw bytes (the blob a peer asked for).
pub fn read_bytes(vault_dir: &str, rel_path: &str) -> Result<Vec<u8>> {
    let abs = safe_join(vault_dir, rel_path).ok_or_else(|| anyhow::anyhow!("unsafe vault path: {rel_path}"))?;
    Ok(std::fs::read(abs)?)
}

/// Delete a vault file that a peer tombstoned. Missing is success — the end state is the same, and
/// a delete that races another device's delete must not error.
pub fn delete_file(vault_dir: &str, rel_path: &str) -> Result<()> {
    let abs = safe_join(vault_dir, rel_path).ok_or_else(|| anyhow::anyhow!("unsafe vault path: {rel_path}"))?;
    match std::fs::remove_file(&abs) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
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

    #[test]
    fn safe_join_rejects_every_shape_of_escape() {
        // `rel_path` is computed in the frontend from a page TITLE, so it is effectively user input.
        // This is the boundary that keeps a title from writing outside the vault folder.
        for bad in [
            "../escape.md",
            "../../escape.md",
            "a/../../escape.md",
            "a/b/../../../escape.md",
            "/etc/passwd",
            "//server/share/x.md",
            "..",
            "../",
            "a/..",
        ] {
            assert!(safe_join("/vault", bad).is_none(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn safe_join_rejects_windows_specific_escapes() {
        // On Windows a bare `C:foo` is drive-RELATIVE, not absolute, so `is_absolute()` alone does
        // not catch it — the component walk has to reject the Prefix.
        for bad in [r"C:\Windows\System32\x.md", r"C:x.md", r"\\server\share\x.md", r"..\escape.md", r"\absolute.md"] {
            let joined = safe_join("/vault", bad);
            if let Some(p) = &joined {
                assert!(
                    p.starts_with("/vault"),
                    "{bad:?} escaped the vault: {p:?}"
                );
            }
        }
    }

    #[test]
    fn safe_join_accepts_ordinary_nested_paths() {
        for good in ["x.md", "Daily/2026-08/2026-08-27.md", "Work/Q3/Roadmap.md", "./x.md", "a/./b.md"] {
            let p = safe_join("/vault", good).unwrap_or_else(|| panic!("{good:?} should be allowed"));
            assert!(p.starts_with("/vault"), "{good:?} landed outside: {p:?}");
        }
    }

    #[test]
    fn safe_join_keeps_non_ascii_names_intact() {
        let p = safe_join("/vault", "\u{8a08}\u{753b}/\u{30e1}\u{30e2}.md").unwrap();
        assert!(p.to_string_lossy().contains("\u{30e1}\u{30e2}.md"));
    }

    #[test]
    fn a_current_dir_component_does_not_leave_a_dot_in_the_path() {
        let p = safe_join("/vault", "./Daily/./x.md").unwrap();
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(!s.contains("/./"), "unresolved current-dir component: {s}");
        assert!(s.ends_with("Daily/x.md"), "{s}");
    }

    #[test]
    fn content_hash_distinguishes_the_edits_the_echo_guard_has_to_notice() {
        // The guard compares hashes to decide whether a file event is our own write echoing back.
        // A collision on a near-identical edit would make an external change invisible.
        assert_ne!(content_hash("# Plan"), content_hash("# Plan "), "trailing whitespace matters");
        assert_ne!(content_hash("a\nb"), content_hash("a\r\nb"), "line endings matter");
        assert_ne!(content_hash(""), content_hash(" "));
        assert_ne!(content_hash("ab"), content_hash("ba"), "order matters");
        assert_eq!(content_hash(""), content_hash(""), "and it is stable");
    }

    #[test]
    fn content_hash_is_byte_oriented_and_handles_non_ascii() {
        assert_eq!(content_hash("caf\u{e9}"), content_hash("caf\u{e9}"));
        assert_ne!(content_hash("caf\u{e9}"), content_hash("cafe"));
        // Same glyph, different normalisation — different bytes on disk, so different hashes.
        assert_ne!(content_hash("caf\u{e9}"), content_hash("cafe\u{301}"));
    }

    #[test]
    fn write_and_read_round_trip_inside_a_real_folder() {
        let dir = std::env::temp_dir().join(format!("pushin_vault_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.to_string_lossy().to_string();

        // Nested parents are created on demand — a page inside three folders must not need them first.
        write_file(&root, "Work/Q3/Roadmap.md", "# Roadmap\n\nbody").unwrap();
        assert_eq!(read_file(&root, "Work/Q3/Roadmap.md").unwrap(), "# Roadmap\n\nbody");

        // Re-writing the same path replaces rather than appends.
        write_file(&root, "Work/Q3/Roadmap.md", "shorter").unwrap();
        assert_eq!(read_file(&root, "Work/Q3/Roadmap.md").unwrap(), "shorter");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_through_an_unsafe_path_fails_instead_of_escaping() {
        let dir = std::env::temp_dir().join(format!("pushin_vault_unsafe_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.to_string_lossy().to_string();

        let err = write_file(&root, "../escaped.md", "should never land").unwrap_err();
        assert!(err.to_string().contains("unsafe vault path"), "got {err}");
        assert!(!dir.parent().unwrap().join("escaped.md").exists(), "a file was written outside the vault");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_missing_file_is_an_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("pushin_vault_missing_{}", std::process::id()));
        let root = dir.to_string_lossy().to_string();
        assert!(read_file(&root, "nope.md").is_err());
    }

    /// A throwaway folder for the scan tests (the suite runs tests in parallel threads).
    fn scratch(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pushin_scan_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn a_scan_finds_nested_files_with_forward_slash_paths() {
        // The path is the identity of a file on the wire, so it has to be the SAME string on Windows
        // and macOS — a backslash here would make one device's `Work/a.pdf` a different file from
        // the other's.
        let root = scratch("nested");
        put(&root, "a.txt", b"1");
        put(&root, "Work/Q3/plan.pdf", b"2");
        let found = scan_files(&root.to_string_lossy(), &std::collections::HashSet::new());

        let paths: Vec<&str> = found.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["Work/Q3/plan.pdf", "a.txt"], "sorted, and always '/'-separated");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_scan_skips_dot_folders_and_page_mirrors() {
        let root = scratch("skip");
        put(&root, ".obsidian/workspace.json", b"{}");
        put(&root, ".DS_Store", b"junk");
        put(&root, "Daily/2026-08-27.md", b"# today");
        put(&root, "Daily/photo.jpg", b"img");

        let mut pages = std::collections::HashSet::new();
        pages.insert("Daily/2026-08-27.md".to_string());
        let found = scan_files(&root.to_string_lossy(), &pages);

        let paths: Vec<&str> = found.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["Daily/photo.jpg"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_scan_of_a_folder_that_is_not_there_is_empty_not_an_error() {
        // The vault folder can be on a drive that isn't mounted yet. That must degrade to "no files
        // this round", never take sync down with it.
        let missing = std::env::temp_dir().join("pushin_scan_definitely_not_here");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(scan_files(&missing.to_string_lossy(), &std::collections::HashSet::new()).is_empty());
    }

    #[test]
    fn hashing_a_file_and_hashing_its_bytes_agree() {
        // `hash_file` streams in 64 KB reads and `hash_bytes` works in memory; the receiver verifies
        // with one what the sender indexed with the other, so a disagreement would reject every
        // file over 64 KB.
        let root = scratch("hash");
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
        put(&root, "big.bin", &body);
        assert_eq!(hash_file(&root.join("big.bin")).unwrap(), hash_bytes(&body));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn is_excluded_covers_dot_components_at_any_depth() {
        let pages = std::collections::HashSet::new();
        assert!(is_excluded(".git/config", &pages));
        assert!(is_excluded("Work/.git/config", &pages));
        assert!(is_excluded("Work/.hidden.pdf", &pages));
        assert!(!is_excluded("Work/visible.pdf", &pages));
        // A page mirror is excluded by exact path, not by extension: another .md is fair game.
        let mut pages = std::collections::HashSet::new();
        pages.insert("Work/Plan.md".to_string());
        assert!(is_excluded("Work/Plan.md", &pages));
        assert!(!is_excluded("Work/Other.md", &pages));
    }

    #[test]
    fn deleting_a_file_that_is_already_gone_is_success() {
        // Two devices can delete the same attachment at once; the second delete to arrive must not
        // error, because the end state it wanted is already true.
        let root = scratch("delete");
        assert!(delete_file(&root.to_string_lossy(), "never-existed.txt").is_ok());
        put(&root, "real.txt", b"x");
        assert!(delete_file(&root.to_string_lossy(), "real.txt").is_ok());
        assert!(!root.join("real.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bytes_written_through_an_unsafe_path_never_land() {
        // Same boundary as `write_file`, on the path a PEER supplies rather than a page title.
        let root = scratch("unsafebytes");
        let err = write_bytes(&root.to_string_lossy(), "../escaped.bin", b"nope").unwrap_err();
        assert!(err.to_string().contains("unsafe vault path"), "got {err}");
        assert!(!root.parent().unwrap().join("escaped.bin").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
