//! Two-way markdown vault: mirror vault pages to `.md` files in the user's chosen folder so they're
//! editable in Pushin OR any external editor and visible in the file manager.
//!
//! This module is the file-side plumbing. The *rule-based folder path* (Daily/by-date, Events/by-date,
//! the page tree → nested folders) is computed in the frontend (it has the page tree + entity links),
//! which passes the `rel_path` here; Rust just reads/writes bytes and (later) watches the folder. SQLite
//! stays the source of truth; `notes.rel_path` maps a page to its file.

use anyhow::Result;
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal_and_absolute() {
        assert!(safe_join("/vault", "Daily/x.md").is_some());
        assert!(safe_join("/vault", "../escape.md").is_none());
        assert!(safe_join("/vault", "/etc/passwd").is_none());
    }
}
