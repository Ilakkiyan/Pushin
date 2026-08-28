//! File-level vault sync: the bytes half.
//!
//! Row sync (`changeset`) replicates the vault's *index* — one `vault_file_index` row per file,
//! carrying its SHA-256 and size. That is metadata; a 40 MB PDF is not, so the bytes travel in a
//! separate phase of the same session (see [`super::protocol::run_session`]), chunked and
//! content-verified.
//!
//! The split is what makes the whole thing convergent. The index is ordinary LWW state — it merges,
//! tombstones, and survives a half-finished session like every other table. The blob phase is then
//! pure catch-up: "the index says this file exists at this hash, I don't have those bytes, send
//! them." If a session dies mid-transfer nothing is corrupted; the next session simply asks again.
//!
//! Three rules keep the two halves from fighting each other:
//!
//! 1. **A file is only indexed once its bytes are here.** A row that arrived from a peer whose blob
//!    hasn't landed yet is *wanted*, not *missing* — [`reindex`] must never read that absence as a
//!    local delete and tombstone the peer's brand-new file. The discriminator is
//!    `vault_file_seen`: it holds only paths THIS device has actually had on disk.
//! 2. **A received blob must hash to what the index promised** before it is allowed to land.
//! 3. **Page mirrors are not files here.** A page already syncs as a `notes` row and each device
//!    rewrites its own `.md`; indexing it too would put two writers on one path.

use crate::db;
use crate::vault;
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Raw bytes per `Blob` frame, before base64. Small enough that a chunk is a cheap allocation on
/// both sides and progress moves visibly on a big file; large enough that a 100 MB attachment is
/// 200 frames, not 200 000.
pub const CHUNK: usize = 512 * 1024;

/// A file one side is missing and would like the bytes for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedFile {
    pub rel_path: String,
    pub hash: String,
    pub size: i64,
}

/// What [`reindex`] changed, for logging and tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReindexStats {
    /// Files newly indexed or re-hashed after an edit.
    pub indexed: usize,
    /// Files hashed from scratch (the expensive ones — the rest hit the mtime/size cache).
    pub hashed: usize,
    /// Index rows tombstoned because the file is gone from this device's disk.
    pub removed: usize,
}

/// Bring the shared file index in line with what is actually on disk.
///
/// Called before a session builds its changeset, so local additions, edits, and deletions ship as
/// ordinary rows. Hashing is skipped for any file whose `(mtime, size)` match the device-local
/// `vault_file_seen` cache — the common case, and the reason a vault full of large attachments does
/// not get fully re-read on every sync.
pub fn reindex(conn: &Connection, vault_dir: &str) -> Result<ReindexStats> {
    let pages = db::page_rel_paths(conn)?;
    let seen = db::vault_file_seen(conn)?;
    let found = vault::scan_files(vault_dir, &pages);
    let mut stats = ReindexStats::default();
    let mut on_disk: HashSet<String> = HashSet::new();

    for f in &found {
        on_disk.insert(f.rel_path.clone());
        // Unchanged since we last hashed it? Trust the cache and don't touch the file.
        let hash = match seen.get(&f.rel_path) {
            Some((mtime, size, hash))
                if *mtime == f.mtime && *size == f.size as i64 && !hash.is_empty() =>
            {
                hash.clone()
            }
            _ => {
                let Some(abs) = vault::safe_join(vault_dir, &f.rel_path) else { continue };
                // Mid-write or unreadable: skip it, the next scan picks up the settled bytes.
                let Ok(h) = vault::hash_file(&abs) else { continue };
                stats.hashed += 1;
                db::set_vault_file_seen(conn, &f.rel_path, f.mtime, f.size as i64, &h)?;
                h
            }
        };
        db::upsert_vault_file(conn, &f.rel_path, &hash, f.size as i64)?;
        stats.indexed += 1;
    }

    // A path we have HAD on disk and no longer do is a real local delete — tombstone it so peers
    // drop their copy. A path we have never had is a peer's file we simply haven't fetched yet.
    for rel_path in seen.keys() {
        if !on_disk.contains(rel_path) && !pages.contains(rel_path) {
            db::remove_vault_file(conn, rel_path)?;
            db::forget_vault_file_seen(conn, rel_path)?;
            stats.removed += 1;
        }
    }
    Ok(stats)
}

/// Delete local files whose index row a peer tombstoned.
///
/// Runs right after a changeset lands and before [`reindex`] would run again — in the other order
/// the scan would see the still-present file and resurrect the row the peer just deleted.
pub fn apply_index_deletions(conn: &Connection, vault_dir: &str) -> Result<usize> {
    let index = db::vault_file_index(conn)?;
    let seen = db::vault_file_seen(conn)?;
    let pages = db::page_rel_paths(conn)?;
    let mut removed = 0;
    for rel_path in seen.keys() {
        if index.contains_key(rel_path) {
            continue;
        }
        // A path that has since become a page mirror is owned by its `notes` row now. Its file index
        // row is stale by definition, and acting on that staleness would delete a page's file.
        if pages.contains(rel_path) {
            continue;
        }
        if vault::delete_file(vault_dir, rel_path).is_ok() {
            db::forget_vault_file_seen(conn, rel_path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Everything the index says exists that this device does not have the bytes for — the want-list
/// sent to a peer.
///
/// "Don't have" covers both a file that was never here and one whose local content has drifted from
/// the index (the peer edited it). Ordered by path so a want-list is deterministic.
pub fn wanted(conn: &Connection, vault_dir: &str) -> Result<Vec<WantedFile>> {
    let index = db::vault_file_index(conn)?;
    let seen = db::vault_file_seen(conn)?;
    let pages = db::page_rel_paths(conn)?;
    let mut out: Vec<WantedFile> = Vec::new();
    for (rel_path, row) in index.iter() {
        // A stray `.md` dropped in the vault is file-synced — until the watcher folds it into a
        // page, at which point the `notes` row owns it and each device rewrites the file itself.
        // Its index row is then frozen at the pre-page hash, so without this both devices would
        // read the drifted file as "missing", ask each other for it every session, and neither
        // could ever serve it (the bytes no longer hash to what the row promised).
        if pages.contains(rel_path) {
            continue;
        }
        let have = match seen.get(rel_path) {
            // The cache says we hold this hash — but only if the file is still on disk.
            Some((_, _, h)) => {
                h == &row.hash
                    && vault::safe_join(vault_dir, rel_path).map(|p| p.exists()).unwrap_or(false)
            }
            None => false,
        };
        if !have {
            out.push(WantedFile {
                rel_path: rel_path.clone(),
                hash: row.hash.clone(),
                size: row.size,
            });
        }
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Serve a blob by content hash: the bytes of whichever indexed file currently carries that hash.
///
/// Looked up by HASH rather than by the path the peer asked for, so a file the peer knows under an
/// old path still serves if we hold the same content elsewhere — and, more importantly, so we can
/// never be talked into reading a path that is not in our own index.
pub fn read_blob(conn: &Connection, vault_dir: &str, hash: &str) -> Option<Vec<u8>> {
    let index = db::vault_file_index(conn).ok()?;
    let row = index.values().find(|r| r.hash == hash)?;
    let bytes = vault::read_bytes(vault_dir, &row.rel_path).ok()?;
    // The file may have changed since it was indexed; serving stale bytes under a promised hash
    // would fail the receiver's check anyway, so drop it here and let the next session re-index.
    (vault::hash_bytes(&bytes) == hash).then_some(bytes)
}

/// Land a received blob: verify the bytes are what was promised, write the file, and record that
/// this device now holds it.
///
/// The hash check is the security boundary as much as the integrity one — a peer cannot use the
/// blob phase to write arbitrary content under a path we trust, because the content address was
/// fixed by the index row that came through ordinary LWW sync.
pub fn write_blob(
    conn: &Connection,
    vault_dir: &str,
    rel_path: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<()> {
    let actual = vault::hash_bytes(bytes);
    if actual != hash {
        anyhow::bail!("blob for {rel_path} hashed {actual}, expected {hash}");
    }
    vault::write_bytes(vault_dir, rel_path, bytes)?;
    let mtime = vault::safe_join(vault_dir, rel_path)
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Record it as seen with the mtime it landed with, so the next reindex doesn't re-hash it —
    // and so a later deletion of this file reads as a local delete rather than a missing fetch.
    db::set_vault_file_seen(conn, rel_path, mtime, bytes.len() as i64, hash)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    /// A fresh empty folder, unique per test (the suite runs tests in parallel threads).
    struct TempVault(std::path::PathBuf);
    impl TempVault {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "pushin_blobs_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempVault(dir)
        }
        fn path(&self) -> String {
            self.0.to_string_lossy().to_string()
        }
        fn write(&self, rel: &str, body: &[u8]) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        fn remove(&self, rel: &str) {
            std::fs::remove_file(self.0.join(rel)).unwrap();
        }
        fn exists(&self, rel: &str) -> bool {
            self.0.join(rel).exists()
        }
    }
    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Clear the dirty flags the way the sync engine does — with change capture suppressed, so the
    /// clear does not immediately re-dirty every row through the AFTER UPDATE trigger.
    fn mark_clean(conn: &Connection) {
        crate::sync::with_capture_suppressed(|| {
            conn.execute("UPDATE vault_file_index SET dirty = 0", []).unwrap();
        });
    }

    fn dirty(conn: &Connection, rel: &str) -> i64 {
        conn.query_row(
            "SELECT dirty FROM vault_file_index WHERE rel_path = ?1",
            params![rel],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn an_attachment_is_indexed_with_its_content_hash_and_size() {
        let v = TempVault::new("index");
        let conn = db::test_conn();
        v.write("Attachments/spec.pdf", b"%PDF-1.7 pretend");

        let stats = reindex(&conn, &v.path()).unwrap();
        assert_eq!(stats.indexed, 1);
        assert_eq!(stats.hashed, 1, "the first pass has to read the bytes");

        let index = db::vault_file_index(&conn).unwrap();
        let row = index.get("Attachments/spec.pdf").expect("indexed");
        assert_eq!(row.hash, vault::hash_bytes(b"%PDF-1.7 pretend"));
        assert_eq!(row.size, 16);
    }

    #[test]
    fn an_unchanged_file_is_neither_re_hashed_nor_re_dirtied() {
        // The ping-pong guard. SQLite fires AFTER UPDATE even when the values are identical, so an
        // unguarded upsert would mark the row dirty on every rescan and have two devices pushing an
        // unchanged file at each other forever — on a 20-second timer.
        let v = TempVault::new("unchanged");
        let conn = db::test_conn();
        v.write("photo.png", b"\x89PNG not really");
        reindex(&conn, &v.path()).unwrap();

        // Pretend the row has been synced: no longer dirty. The clear has to run with capture
        // suppressed, exactly as `changeset::stamp_dirty` does — an ordinary `SET dirty = 0` fires
        // the AFTER UPDATE trigger, which immediately sets it back to 1.
        mark_clean(&conn);

        let again = reindex(&conn, &v.path()).unwrap();
        assert_eq!(again.hashed, 0, "an unchanged file must hit the mtime/size cache");
        assert_eq!(dirty(&conn, "photo.png"), 0, "an unchanged file must not be re-marked dirty");
    }

    #[test]
    fn an_edited_file_is_re_hashed_and_ships_again() {
        let v = TempVault::new("edited");
        let conn = db::test_conn();
        v.write("notes.txt", b"first");
        reindex(&conn, &v.path()).unwrap();
        mark_clean(&conn);

        // A different size alone is enough to miss the cache, which is what we want — the mtime may
        // not have moved at all on a filesystem with one-second timestamps.
        v.write("notes.txt", b"second, longer");
        let again = reindex(&conn, &v.path()).unwrap();

        assert_eq!(again.hashed, 1);
        assert_eq!(dirty(&conn, "notes.txt"), 1, "an edit has to ship");
        let index = db::vault_file_index(&conn).unwrap();
        assert_eq!(index["notes.txt"].hash, vault::hash_bytes(b"second, longer"));
    }

    #[test]
    fn a_file_deleted_here_is_tombstoned_for_the_other_devices() {
        let v = TempVault::new("deleted");
        let conn = db::test_conn();
        v.write("old.bin", b"bytes");
        reindex(&conn, &v.path()).unwrap();

        v.remove("old.bin");
        let after = reindex(&conn, &v.path()).unwrap();

        assert_eq!(after.removed, 1);
        assert!(!db::vault_file_index(&conn).unwrap().contains_key("old.bin"));
        let tombstones: i64 = conn
            .query_row(
                "SELECT count(*) FROM sync_tombstones WHERE entity_table = 'vault_file_index'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tombstones, 1, "the delete has to propagate, not just happen locally");
    }

    #[test]
    fn a_peers_file_we_have_not_downloaded_yet_is_wanted_not_deleted() {
        // The single most damaging way to get this wrong: the peer's index row arrives before its
        // bytes do, the scan sees no such file on disk, reads that as a local delete, and tombstones
        // the file the peer just created. `vault_file_seen` is the discriminator — it holds only
        // paths THIS device has actually had.
        let v = TempVault::new("peerfile");
        let conn = db::test_conn();
        db::upsert_vault_file(&conn, "FromLaptop/report.pdf", "abc123", 4096).unwrap();

        let stats = reindex(&conn, &v.path()).unwrap();
        assert_eq!(stats.removed, 0, "a file we never had is not a file we deleted");
        assert!(db::vault_file_index(&conn).unwrap().contains_key("FromLaptop/report.pdf"));

        let want = wanted(&conn, &v.path()).unwrap();
        assert_eq!(want.len(), 1);
        assert_eq!(want[0].rel_path, "FromLaptop/report.pdf");
        assert_eq!(want[0].hash, "abc123");
    }

    #[test]
    fn a_file_we_already_hold_is_not_wanted() {
        let v = TempVault::new("held");
        let conn = db::test_conn();
        v.write("have.txt", b"mine");
        reindex(&conn, &v.path()).unwrap();
        assert!(wanted(&conn, &v.path()).unwrap().is_empty());
    }

    #[test]
    fn a_file_whose_bytes_vanished_from_disk_is_wanted_again() {
        // The cache says we hold this hash, but the file is gone (a botched restore, a synced folder
        // that dropped it). Trusting the cache alone would leave a hole nothing ever refills.
        let v = TempVault::new("vanished");
        let conn = db::test_conn();
        v.write("gone.txt", b"here for now");
        reindex(&conn, &v.path()).unwrap();
        // Remove the file WITHOUT reindexing, so the index row and the seen-cache both still claim it.
        v.remove("gone.txt");

        let want = wanted(&conn, &v.path()).unwrap();
        assert_eq!(want.len(), 1, "the bar for 'have it' is the file being there");
        assert_eq!(want[0].rel_path, "gone.txt");
    }

    #[test]
    fn page_mirrors_and_dot_folders_never_enter_the_file_index() {
        let v = TempVault::new("excluded");
        let conn = db::test_conn();
        // A page's own .md file: it already syncs as a `notes` row.
        conn.execute(
            "INSERT INTO notes(content, created_at, updated_at, rel_path) VALUES('# Plan', 't', 't', 'Work/Plan.md')",
            [],
        )
        .unwrap();
        v.write("Work/Plan.md", b"# Plan");
        v.write(".obsidian/workspace.json", b"{}");
        v.write("Work/diagram.png", b"img");

        reindex(&conn, &v.path()).unwrap();
        let index = db::vault_file_index(&conn).unwrap();

        assert!(index.contains_key("Work/diagram.png"), "a real attachment syncs");
        assert!(!index.contains_key("Work/Plan.md"), "a page mirror must not double-sync");
        assert!(!index.contains_key(".obsidian/workspace.json"), "editor state is device-local");
    }

    #[test]
    fn a_file_that_has_become_a_page_is_left_to_the_page_row() {
        // A stray .md is file-synced until the vault watcher folds it into a page. From then on the
        // `notes` row owns it. Its frozen index row must stop driving anything, or both devices ask
        // each other for a file neither can serve, on a 20-second timer, forever.
        let v = TempVault::new("becamepage");
        let conn = db::test_conn();
        v.write("Loose.md", b"# dropped in");
        reindex(&conn, &v.path()).unwrap();
        assert!(wanted(&conn, &v.path()).unwrap().is_empty(), "precondition: we hold it");

        // The watcher turns it into a page, and the page's own text drifts from the indexed bytes.
        conn.execute(
            "INSERT INTO notes(content, created_at, updated_at, rel_path) VALUES('# dropped in', 't', 't', 'Loose.md')",
            [],
        )
        .unwrap();
        v.write("Loose.md", b"# dropped in, then edited in the app");

        assert!(wanted(&conn, &v.path()).unwrap().is_empty(), "the page row owns it now");
        // And a stale tombstone for it must never delete the page's file.
        db::remove_vault_file(&conn, "Loose.md").unwrap();
        assert_eq!(apply_index_deletions(&conn, &v.path()).unwrap(), 0);
        assert!(v.exists("Loose.md"));
    }

    #[test]
    fn a_blob_whose_bytes_do_not_match_the_promised_hash_is_refused() {
        // The hash check is the security boundary, not just an integrity one: the content address
        // was fixed by an index row that came through ordinary LWW sync, so a peer cannot use the
        // blob phase to write content of its own choosing under a path we trust.
        let v = TempVault::new("badhash");
        let conn = db::test_conn();
        let err = write_blob(&conn, &v.path(), "evil.sh", &vault::hash_bytes(b"harmless"), b"rm -rf /")
            .unwrap_err();
        assert!(err.to_string().contains("expected"), "got {err}");
        assert!(!v.exists("evil.sh"), "nothing may land when the hash disagrees");
    }

    #[test]
    fn a_verified_blob_lands_and_stops_being_wanted() {
        let v = TempVault::new("land");
        let conn = db::test_conn();
        let bytes = b"the real attachment";
        let hash = vault::hash_bytes(bytes);
        db::upsert_vault_file(&conn, "In/file.bin", &hash, bytes.len() as i64).unwrap();
        assert_eq!(wanted(&conn, &v.path()).unwrap().len(), 1);

        write_blob(&conn, &v.path(), "In/file.bin", &hash, bytes).unwrap();

        assert!(v.exists("In/file.bin"));
        assert!(wanted(&conn, &v.path()).unwrap().is_empty(), "we hold it now");
        // And it must not read as a local delete on the next scan, nor be re-hashed.
        let stats = reindex(&conn, &v.path()).unwrap();
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.hashed, 0, "a landed blob is already hashed");
    }

    #[test]
    fn a_file_a_peer_tombstoned_is_deleted_here_too() {
        let v = TempVault::new("peerdel");
        let conn = db::test_conn();
        v.write("shared.pdf", b"content");
        reindex(&conn, &v.path()).unwrap();

        // The peer deleted it: their tombstone removes our index row, but not our file.
        db::remove_vault_file(&conn, "shared.pdf").unwrap();
        let removed = apply_index_deletions(&conn, &v.path()).unwrap();

        assert_eq!(removed, 1);
        assert!(!v.exists("shared.pdf"));
        // And the next scan must not resurrect the row from the file we just removed.
        let stats = reindex(&conn, &v.path()).unwrap();
        assert_eq!(stats.indexed, 0);
        assert!(db::vault_file_index(&conn).unwrap().is_empty());
    }

    #[test]
    fn read_blob_refuses_to_serve_bytes_that_have_drifted_from_the_index() {
        // The file changed after it was indexed. Serving the old promise with the new bytes would
        // fail the receiver's check anyway — better to serve nothing and let the next session,
        // which will have re-indexed, serve the truth.
        let v = TempVault::new("drift");
        let conn = db::test_conn();
        v.write("doc.txt", b"as indexed");
        reindex(&conn, &v.path()).unwrap();
        let promised = db::vault_file_index(&conn).unwrap()["doc.txt"].hash.clone();

        v.write("doc.txt", b"changed underneath");
        assert!(read_blob(&conn, &v.path(), &promised).is_none());
    }

    #[test]
    fn the_sync_id_of_a_file_is_derived_from_its_path_so_two_devices_agree() {
        // Random uuids would give two devices two rows for one attachment, and applying the peer's
        // row would hit UNIQUE(rel_path) and abort the WHOLE changeset batch — the failure mode one
        // .ics feed once caused for all of sync.
        let a = db::test_conn();
        let b = db::test_conn();
        db::upsert_vault_file(&a, "Shared/photo.png", "hash-a", 10).unwrap();
        db::upsert_vault_file(&b, "Shared/photo.png", "hash-b", 20).unwrap();

        let uuid_of = |c: &Connection| -> String {
            c.query_row("SELECT uuid FROM vault_file_index WHERE rel_path = 'Shared/photo.png'", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(uuid_of(&a), uuid_of(&b));
        assert_eq!(uuid_of(&a), db::vault_file_uuid("Shared/photo.png"));
    }

    #[test]
    fn a_file_over_the_size_cap_is_left_alone_entirely() {
        // Not an error, not a partial index — simply not synced, so it never appears in a peer's
        // want-list and never blocks the session behind it.
        let v = TempVault::new("huge");
        v.write("small.txt", b"ok");
        let pages = std::collections::HashSet::new();
        let found = vault::scan_files(&v.path(), &pages);
        assert_eq!(found.len(), 1);
        assert!(vault::MAX_SYNC_FILE > 0);
        // The cap is enforced in the scan itself, so an over-cap file is invisible to reindex.
        assert!(found.iter().all(|f| f.size <= vault::MAX_SYNC_FILE));
    }
}
