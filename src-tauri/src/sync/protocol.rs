//! The peer-to-peer sync wire protocol — deliberately transport-agnostic so it runs over anything
//! that's `AsyncRead + AsyncWrite` (an Iroh QUIC bi-stream in production; an in-memory duplex in
//! tests). Messages are length-prefixed JSON.
//!
//! One session is a fixed, deadlock-free choreography between an *initiator* (the dialer) and a
//! *responder* (the accepter). Both authenticate with the shared mesh secret, then each pulls the
//! other's changes since its last watermark:
//!
//! ```text
//! initiator → responder:  Hello, Pull, Push
//! responder → initiator:  Hello, Push, Pull, Bye
//! ```
//!
//! The trailing `Bye` is what makes the session safe to tear down. The responder's last act would
//! otherwise be a *read*, and the initiator closes the QUIC connection the moment `run_session`
//! returns — a close that discards unacknowledged stream data, killing the responder's final read.
//! Ending on a responder→initiator message means the initiator only closes once the responder has
//! applied everything.
//!
//! The [`SyncStore`] trait is the seam to the database; production wires it to the SQLite changeset
//! functions, tests wire it to two in-memory DBs to prove end-to-end convergence over a real stream.

use super::blobs::{self, WantedFile};
use super::changeset::Change;
use super::frame::{read_frame, write_frame};
use base64::Engine as _;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

#[derive(Serialize, Deserialize, Debug)]
enum Msg {
    /// Identify + prove mesh membership. `mesh` is the shared secret (the connection is already
    /// E2E-encrypted by QUIC node keys; the secret is the app-level "you belong to my network").
    /// `name` is a human label shown in the peer's device list.
    ///
    /// `blobs` advertises support for the file-transfer phase. It is `#[serde(default)]` and the
    /// phase runs only when BOTH sides set it, which is what makes a mixed-version pair keep
    /// working: an older peer's Hello simply has no such field, so it deserializes to `false` and
    /// neither side enters a phase the other would not answer. (The reverse direction is free —
    /// serde ignores unknown fields, so an old build reading this Hello just drops it.) Without the
    /// negotiation, one un-updated device would desynchronise the choreography and break sync
    /// entirely rather than merely skipping files.
    Hello { node: String, mesh: String, name: String, #[serde(default)] blobs: bool },
    /// "Send me everything with HLC strictly greater than `since`."
    Pull { since: String },
    /// A batch of changes, with the highest HLC contained (the receiver's new watermark).
    Push { changes: Vec<Change>, max_hlc: String },
    /// "The file index says these exist and I don't have their bytes — send them."
    Want { files: Vec<WantedFile> },
    /// One chunk of one file. `seq` is the chunk index (for ordering assertions), `last` closes the
    /// file, `data` is base64 of at most [`blobs::CHUNK`] raw bytes.
    Blob { rel_path: String, hash: String, seq: u32, last: bool, data: String },
    /// End of the blob stream answering a [`Msg::Want`]. Always sent, even when nothing was served
    /// — it is what lets the receiver stop reading.
    BlobsDone,
    /// Responder → initiator, last: "I have applied your Push; you may close the connection."
    Bye,
}

/// The database seam the protocol drives. Implementations do short, synchronous, locked DB ops —
/// never held across an `.await` (gotcha #8).
pub trait SyncStore {
    fn mesh_secret(&self) -> String;
    fn node_id(&self) -> String;
    /// A human label for this device, shown in the peer's device list.
    fn device_name(&self) -> String;
    /// Highest HLC we've already pulled from `peer` (our delta watermark for them). `""` = never.
    fn watermark(&self, peer: &str) -> String;
    fn set_watermark(&self, peer: &str, hlc: &str);
    /// All local changes with HLC > `since`.
    fn changes_since(&self, since: &str) -> Result<Vec<Change>>;
    /// Apply remote changes; return the highest HLC applied.
    fn apply(&self, changes: &[Change]) -> Result<String>;
}

/// The file-transfer seam, kept separate from [`SyncStore`] so the row protocol can be exercised —
/// and shipped — without a vault. [`NoBlobs`] is the "this device has no vault folder" implementation
/// and is what the plain [`run_session`] uses.
///
/// Every method does short synchronous work (a DB read, a file read/write) and is called between
/// awaits, never across one — same rule as [`SyncStore`] (gotcha #8).
pub trait BlobStore {
    /// Whether to advertise the blob phase at all. `false` skips it even against a capable peer —
    /// a device with no vault folder configured has nothing to send and nowhere to put what it gets.
    fn enabled(&self) -> bool;
    /// The files this device is missing bytes for, computed AFTER the peer's changeset has been
    /// applied so it includes whatever they just told us exists. Production implementations also
    /// settle peer-tombstoned deletions here.
    fn wanted(&self) -> Vec<WantedFile>;
    /// The bytes behind a content hash, or `None` if this device does not (or no longer) holds them.
    fn read(&self, hash: &str) -> Option<Vec<u8>>;
    /// A fully-received, hash-verified file. `rel_path` is OUR path for that hash from OUR want-list,
    /// never a path the peer chose — see the receive loop.
    fn store(&self, rel_path: &str, hash: &str, bytes: &[u8]) -> Result<()>;
    /// Receive progress: bytes landed so far out of the want-list total. Drives the sync bar.
    fn progress(&self, _done: u64, _total: u64) {}
}

/// A [`BlobStore`] that carries nothing — used by [`run_session`] and by every row-only test.
#[allow(dead_code)] // the row-only path: used by `run_session` and the protocol tests
pub struct NoBlobs;
impl BlobStore for NoBlobs {
    fn enabled(&self) -> bool { false }
    fn wanted(&self) -> Vec<WantedFile> { Vec::new() }
    fn read(&self, _hash: &str) -> Option<Vec<u8>> { None }
    fn store(&self, _rel_path: &str, _hash: &str, _bytes: &[u8]) -> Result<()> { Ok(()) }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub peer: String,
    pub peer_name: String,
    pub received: usize,
    pub sent: usize,
    /// Vault files whose bytes landed on this device during the session.
    pub files_received: usize,
    /// Vault files this device served to the peer.
    pub files_sent: usize,
    /// Bytes of file content received (the number the progress bar counts up to).
    pub bytes_received: u64,
}

/// Read one framed `Msg` (thin alias so the sync choreography below reads unchanged).
async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
    read_frame(r).await
}
/// Write one framed `Msg`.
async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
    write_frame(w, msg).await
}

/// Exchange Hellos and verify mesh membership; returns the peer's (node id, device name) and
/// whether the peer can take part in the blob phase.
async fn handshake<S, R, W>(
    store: &S,
    blobs_ok: bool,
    initiator: bool,
    r: &mut R,
    w: &mut W,
) -> Result<(String, String, bool)>
where
    S: SyncStore,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let me = Msg::Hello {
        node: store.node_id(),
        mesh: store.mesh_secret(),
        name: store.device_name(),
        blobs: blobs_ok,
    };
    let peer = if initiator {
        write_msg(w, &me).await?;
        read_msg(r).await?
    } else {
        let p = read_msg(r).await?;
        write_msg(w, &me).await?;
        p
    };
    match peer {
        Msg::Hello { node, mesh, name, blobs } if mesh == store.mesh_secret() && !mesh.is_empty() => {
            Ok((node, name, blobs))
        }
        Msg::Hello { .. } => bail!("peer failed mesh authentication"),
        _ => bail!("expected Hello, got something else"),
    }
}

/// Serve a peer's want-list: stream each file we hold as chunked [`Msg::Blob`] frames, then
/// [`Msg::BlobsDone`].
///
/// A file we cannot produce is silently skipped rather than erroring — the peer treats an
/// unanswered want as "ask again next session", which is exactly right when the file was deleted or
/// edited between the index row being written and the request arriving.
async fn serve_blobs<B, W>(store: &B, w: &mut W, files: &[WantedFile], stats: &mut SessionStats) -> Result<()>
where
    B: BlobStore,
    W: AsyncWrite + Unpin,
{
    for f in files {
        let Some(bytes) = store.read(&f.hash) else { continue };
        let chunks = bytes.chunks(blobs::CHUNK);
        let total = chunks.len().max(1);
        for (i, chunk) in bytes.chunks(blobs::CHUNK).enumerate() {
            write_msg(
                w,
                &Msg::Blob {
                    rel_path: f.rel_path.clone(),
                    hash: f.hash.clone(),
                    seq: i as u32,
                    last: i + 1 == total,
                    data: base64::engine::general_purpose::STANDARD.encode(chunk),
                },
            )
            .await?;
        }
        // A zero-byte file has no chunks at all, so it needs one explicit empty frame — otherwise an
        // empty attachment would never arrive and would be re-requested every single session.
        if bytes.is_empty() {
            write_msg(
                w,
                &Msg::Blob {
                    rel_path: f.rel_path.clone(),
                    hash: f.hash.clone(),
                    seq: 0,
                    last: true,
                    data: String::new(),
                },
            )
            .await?;
        }
        stats.files_sent += 1;
    }
    write_msg(w, &Msg::BlobsDone).await
}

/// Read a blob stream until [`Msg::BlobsDone`], landing each verified file.
///
/// `wants` is OUR want-list, and it is the whitelist: a chunk whose hash we did not ask for is
/// dropped, and a file is written to the path WE recorded for that hash, never the one the sender
/// put on the wire. Both matter — the blob phase is the only place a peer hands us raw bytes and a
/// path, and a peer that could choose the path could write anywhere the vault can reach.
async fn recv_blobs<B, R>(store: &B, r: &mut R, wants: &[WantedFile], stats: &mut SessionStats) -> Result<()>
where
    B: BlobStore,
    R: AsyncRead + Unpin,
{
    use std::collections::HashMap;
    let by_hash: HashMap<&str, &WantedFile> = wants.iter().map(|f| (f.hash.as_str(), f)).collect();
    let total: u64 = wants.iter().map(|f| f.size.max(0) as u64).sum();
    let mut done: u64 = 0;
    // At most one file is in flight at a time (the sender streams them one after another), so a
    // single buffer is enough — and its cap is what keeps a hostile peer from growing it forever.
    let mut buf: Vec<u8> = Vec::new();
    let mut current: Option<String> = None;

    loop {
        match read_msg(r).await? {
            Msg::BlobsDone => break,
            Msg::Blob { hash, seq, last, data, .. } => {
                let Some(want) = by_hash.get(hash.as_str()).copied() else { continue };
                if current.as_deref() != Some(hash.as_str()) {
                    // A new file started; anything half-read before it is abandoned.
                    buf.clear();
                    current = Some(hash.clone());
                }
                let chunk = base64::engine::general_purpose::STANDARD
                    .decode(data.as_bytes())
                    .map_err(|e| anyhow::anyhow!("blob chunk {seq} for {hash}: {e}"))?;
                if buf.len() + chunk.len() > crate::vault::MAX_SYNC_FILE as usize {
                    bail!("blob for {hash} exceeds the {} byte cap", crate::vault::MAX_SYNC_FILE);
                }
                buf.extend_from_slice(&chunk);
                if last {
                    // `store` re-verifies the hash; a mismatch means a corrupt or lying peer, and we
                    // drop the file rather than the session — the next sync asks for it again.
                    if store.store(&want.rel_path, &want.hash, &buf).is_ok() {
                        stats.files_received += 1;
                        stats.bytes_received += buf.len() as u64;
                    }
                    done += want.size.max(0) as u64;
                    store.progress(done.min(total), total);
                    buf.clear();
                    current = None;
                }
            }
            _ => bail!("expected Blob or BlobsDone"),
        }
    }
    store.progress(total, total);
    Ok(())
}

/// Run one full sync session over a stream, rows only. See [`run_session_with_blobs`].
#[allow(dead_code)] // the engine always carries blobs; this is the seam tests and callers without a vault use
pub async fn run_session<S, R, W>(store: &S, initiator: bool, r: R, w: W) -> Result<SessionStats>
where
    S: SyncStore,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    run_session_with_blobs(store, &NoBlobs, initiator, r, w).await
}

/// Run one full sync session over a stream. Both peers end up converged (subject to LWW).
///
/// The choreography stays a fixed, deadlock-free alternation. Rows first, then — only when both
/// Hellos advertised it — the file phase, in the same strict turn-taking order:
///
/// ```text
/// initiator → responder:  Hello, Pull, Push, Want, Blobs…
/// responder → initiator:  Hello, Push, Pull, Blobs…, Want, Bye
/// ```
///
/// The file phase deliberately sits AFTER the row exchange: a want-list is only correct once the
/// peer's `vault_file_index` rows have been applied, because those rows are how this device learns
/// which files exist at all.
pub async fn run_session_with_blobs<S, B, R, W>(
    store: &S,
    blob_store: &B,
    initiator: bool,
    mut r: R,
    mut w: W,
) -> Result<SessionStats>
where
    S: SyncStore,
    B: BlobStore,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (peer, peer_name, peer_blobs) =
        handshake(store, blob_store.enabled(), initiator, &mut r, &mut w).await?;
    let do_blobs = peer_blobs && blob_store.enabled();
    // The two ways file sync silently does nothing. Neither is an error, and that is exactly why
    // they need saying out loud — otherwise "my files aren't syncing" has no visible cause at all.
    if !blob_store.enabled() {
        super::log::info("file sync off on this device (no vault folder set) — rows only");
    } else if !peer_blobs {
        super::log::warn(format!(
            "peer {peer_name} does not support file sync (older build) — rows synced, files skipped"
        ));
    }
    let mut stats = SessionStats { peer: peer.clone(), peer_name, ..Default::default() };

    // Pull our side: ask the peer for everything past our watermark, apply it, advance the watermark.
    let do_pull = |store: &S| -> Msg { Msg::Pull { since: store.watermark(&peer) } };
    // Serve the peer's pull: hand them everything past the `since` they asked for.
    async fn serve_pull<S: SyncStore, W: AsyncWrite + Unpin>(
        store: &S, w: &mut W, since: &str, stats: &mut SessionStats,
    ) -> Result<()> {
        let changes = store.changes_since(since)?;
        let max_hlc = changes.iter().map(|c| c.hlc.clone()).max().unwrap_or_default();
        stats.sent += changes.len();
        write_msg(w, &Msg::Push { changes, max_hlc }).await
    }
    async fn recv_push<S: SyncStore, R: AsyncRead + Unpin>(
        store: &S, r: &mut R, peer: &str, stats: &mut SessionStats,
    ) -> Result<()> {
        match read_msg(r).await? {
            Msg::Push { changes, max_hlc } => {
                stats.received += changes.len();
                let applied = store.apply(&changes)?;
                let hi = applied.max(max_hlc);
                if hi > store.watermark(peer) {
                    store.set_watermark(peer, &hi);
                }
                Ok(())
            }
            _ => bail!("expected Push"),
        }
    }
    async fn recv_pull<R: AsyncRead + Unpin>(r: &mut R) -> Result<String> {
        match read_msg(r).await? {
            Msg::Pull { since } => Ok(since),
            _ => bail!("expected Pull"),
        }
    }
    async fn recv_want<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<WantedFile>> {
        match read_msg(r).await? {
            Msg::Want { files } => Ok(files),
            _ => bail!("expected Want"),
        }
    }
    async fn recv_bye<R: AsyncRead + Unpin>(r: &mut R) -> Result<()> {
        match read_msg(r).await? {
            Msg::Bye => Ok(()),
            _ => bail!("expected Bye"),
        }
    }

    if initiator {
        write_msg(&mut w, &do_pull(store)).await?; // Pull
        recv_push(store, &mut r, &peer, &mut stats).await?; // Push
        let since = recv_pull(&mut r).await?; // their Pull
        serve_pull(store, &mut w, &since, &mut stats).await?; // our Push
        if do_blobs {
            let mine = blob_store.wanted();
            super::log::info(format!("asking for {} file(s)", mine.len()));
            write_msg(&mut w, &Msg::Want { files: mine.clone() }).await?; // our Want
            recv_blobs(blob_store, &mut r, &mine, &mut stats).await?; // their Blobs
            let theirs = recv_want(&mut r).await?; // their Want
            serve_blobs(blob_store, &mut w, &theirs, &mut stats).await?; // our Blobs
        }
        recv_bye(&mut r).await?; // they applied it — safe to close
    } else {
        let since = recv_pull(&mut r).await?; // their Pull
        serve_pull(store, &mut w, &since, &mut stats).await?; // our Push
        write_msg(&mut w, &do_pull(store)).await?; // Pull
        recv_push(store, &mut r, &peer, &mut stats).await?; // Push
        if do_blobs {
            let theirs = recv_want(&mut r).await?; // their Want
            serve_blobs(blob_store, &mut w, &theirs, &mut stats).await?; // our Blobs
            let mine = blob_store.wanted();
            super::log::info(format!("asking for {} file(s)", mine.len()));
            write_msg(&mut w, &Msg::Want { files: mine.clone() }).await?; // our Want
            recv_blobs(blob_store, &mut r, &mine, &mut stats).await?; // their Blobs
        }
        write_msg(&mut w, &Msg::Bye).await?; // release the initiator
    }

    // Gracefully finish the write side so the peer's final read sees clean EOF, not a reset
    // (important on a real QUIC stream — harmless on an in-memory duplex).
    let _ = w.shutdown().await;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::sync::changeset;
    use crate::sync::hlc::HlcState;
    use rusqlite::{params, Connection};
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A `SyncStore` backed by a real (in-memory) Pushin DB + a clock + watermark map.
    struct TestStore {
        node: String,
        mesh: String,
        conn: Connection,
        clock: RefCell<HlcState>,
        watermarks: RefCell<HashMap<String, String>>,
    }
    impl TestStore {
        fn new(node: &str, mesh: &str) -> Self {
            TestStore {
                node: node.into(),
                mesh: mesh.into(),
                conn: db::test_conn(),
                clock: RefCell::new(HlcState::default()),
                watermarks: RefCell::new(HashMap::new()),
            }
        }
    }
    impl SyncStore for TestStore {
        fn mesh_secret(&self) -> String { self.mesh.clone() }
        fn node_id(&self) -> String { self.node.clone() }
        fn device_name(&self) -> String { format!("{}-device", self.node) }
        fn watermark(&self, peer: &str) -> String {
            self.watermarks.borrow().get(peer).cloned().unwrap_or_default()
        }
        fn set_watermark(&self, peer: &str, hlc: &str) {
            self.watermarks.borrow_mut().insert(peer.into(), hlc.into());
        }
        fn changes_since(&self, since: &str) -> Result<Vec<Change>> {
            // Stamp local edits, then collect the delta — exactly the production sequence.
            changeset::stamp_dirty(&self.conn, &self.node, &mut self.clock.borrow_mut(), 1000)?;
            changeset::changes_since(&self.conn, since)
        }
        fn apply(&self, changes: &[Change]) -> Result<String> {
            let stats = changeset::apply_changes(&self.conn, &mut self.clock.borrow_mut(), 1000, changes)?;
            Ok(stats.max_hlc)
        }
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn session_converges_two_real_dbs_over_a_stream() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");

        // A has a project + task; B has a separate event. After a session both have all three.
        a.conn.execute("INSERT INTO projects(name, color, created_at) VALUES('P','#fff','t')", []).unwrap();
        let pid = a.conn.last_insert_rowid();
        a.conn.execute("INSERT INTO tasks(title, project_id, created_at) VALUES('T', ?1, 't')", params![pid]).unwrap();
        b.conn.execute("INSERT INTO events(title, start, end, created_at) VALUES('E','s','e','t')", []).unwrap();

        let (c1, c2) = tokio::io::duplex(1 << 20);
        let (ar, aw) = tokio::io::split(c1);
        let (br, bw) = tokio::io::split(c2);

        let ta = async { run_session(&a, true, ar, aw).await };
        let tb = async { run_session(&b, false, br, bw).await };
        let (ra, rb) = tokio::join!(ta, tb);
        ra.unwrap();
        rb.unwrap();

        // Both DBs now hold the task and the event.
        assert_eq!(count(&a.conn, "SELECT count(*) FROM events"), 1, "A pulled B's event");
        assert_eq!(count(&b.conn, "SELECT count(*) FROM tasks"), 1, "B pulled A's task");
        assert_eq!(count(&b.conn, "SELECT count(*) FROM projects"), 1, "FK target came along");

        // Re-syncing quiesces: within a couple of rounds nothing new flows, and crucially no
        // duplicate rows are ever created (a foreign-authored row may echo back once under the
        // scalar-HLC watermark — idempotent — but it must converge to zero).
        let mut rounds_to_quiet = 0;
        for i in 0..4 {
            let (c1, c2) = tokio::io::duplex(1 << 20);
            let (ar, aw) = tokio::io::split(c1);
            let (br, bw) = tokio::io::split(c2);
            let (ra, _) = tokio::join!(run_session(&a, true, ar, aw), run_session(&b, false, br, bw));
            if ra.unwrap().received == 0 {
                rounds_to_quiet = i;
                break;
            }
        }
        assert!(rounds_to_quiet <= 2, "sync must quiesce quickly, took {rounds_to_quiet} rounds");
        // No duplication from the echoes.
        assert_eq!(count(&a.conn, "SELECT count(*) FROM tasks"), 1);
        assert_eq!(count(&a.conn, "SELECT count(*) FROM events"), 1);
        assert_eq!(count(&b.conn, "SELECT count(*) FROM tasks"), 1);
        assert_eq!(count(&b.conn, "SELECT count(*) FROM events"), 1);
    }

    #[tokio::test]
    async fn wrong_mesh_secret_is_rejected() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "DIFFERENT");
        let (c1, c2) = tokio::io::duplex(1 << 16);
        let (ar, aw) = tokio::io::split(c1);
        let (br, bw) = tokio::io::split(c2);
        let (ra, rb) = tokio::join!(run_session(&a, true, ar, aw), run_session(&b, false, br, bw));
        assert!(ra.is_err() || rb.is_err(), "mismatched mesh secret must fail the session");
    }

    /// The transport-level counterpart to the duplex test above: two REAL Iroh endpoints in one
    /// process, paired through a real `make_ticket`/`parse_ticket`/`dial` round-trip. This is the
    /// only test that exercises `transport.rs` end-to-end, and it is what caught the teardown race:
    /// the initiator used to close the QUIC connection the instant its own session returned, which
    /// discarded the responder's final read — so the *inviting* device silently failed every pair
    /// (no peer recorded, no changes applied) while the joiner reported success. Loopback-only:
    /// relays disabled, dialed via the ticket's direct addresses.
    #[tokio::test]
    async fn two_real_iroh_endpoints_pair_and_converge() {
        use crate::sync::transport;
        use std::time::Duration;

        let ep_a = transport::bind(transport::secret_key([7u8; 32]), false).await.unwrap();
        let ep_b = transport::bind(transport::secret_key([9u8; 32]), false).await.unwrap();

        let a = TestStore::new(&ep_a.node_id().to_string(), "shared-mesh");
        let b = TestStore::new(&ep_b.node_id().to_string(), "shared-mesh");
        a.conn.execute("INSERT INTO tasks(title, created_at) VALUES('from A','t')", []).unwrap();
        b.conn.execute("INSERT INTO events(title, start, end, created_at) VALUES('from B','s','e','t')", []).unwrap();

        let ticket = transport::make_ticket(&ep_a, "shared-mesh", false).await.unwrap();
        let (addr, mesh) = transport::parse_ticket(&ticket).unwrap();
        assert_eq!(mesh, "shared-mesh", "the mesh secret survives the ticket round-trip");

        // A accepts one inbound session (the inviting device).
        let accept = async {
            let incoming = ep_a.accept().await.expect("A got an inbound connection");
            let conn = incoming.await.expect("A completed the handshake");
            let (send, recv) = transport::accept_stream(&conn).await.unwrap();
            let stats = run_session(&a, false, recv, send).await;
            let _ = tokio::time::timeout(Duration::from_secs(10), conn.closed()).await;
            stats
        };
        // B dials the ticket and drives the session (the joining device).
        let dial = async {
            let (conn, send, recv) = transport::dial(&ep_b, addr).await?;
            let stats = run_session(&b, true, recv, send).await;
            conn.close(0u32.into(), b"done");
            stats
        };

        let (ra, rb) = tokio::time::timeout(Duration::from_secs(30), async {
            tokio::join!(accept, dial)
        })
        .await
        .expect("pairing must not hang — a stuck join is the bug this test guards");

        // BOTH sides must succeed. The joiner alone succeeding is the silent-failure mode.
        ra.expect("the inviting device's session");
        rb.expect("the joining device's session");

        assert_eq!(count(&a.conn, "SELECT count(*) FROM events"), 1, "A pulled B's event over QUIC");
        assert_eq!(count(&b.conn, "SELECT count(*) FROM tasks"), 1, "B pulled A's task over QUIC");
    }

    /// An invite minted the instant the endpoint binds carries only local interface addresses —
    /// direct addresses resolve in milliseconds, the home-relay handshake takes seconds. That
    /// leaves a joiner with no path at all when the direct one is blocked (a dismissed Windows
    /// Firewall prompt does it), so `make_ticket` waits for the relay before minting.
    #[tokio::test]
    async fn a_minted_ticket_carries_a_relay_url_not_just_the_lan_address() {
        use crate::sync::transport;
        use iroh::Watcher;
        use std::time::Duration;

        let ep = transport::bind(transport::secret_key([11u8; 32]), true).await.unwrap();
        let ticket = transport::make_ticket(&ep, "m", true).await.unwrap();
        let (addr, _) = transport::parse_ticket(&ticket).unwrap();

        // No reachable relay (offline / CI without egress) is a legitimate outcome: the invite still
        // carries direct addresses. Only assert the relay landed when one actually came up.
        let have_relay = tokio::time::timeout(Duration::from_millis(50), ep.home_relay().initialized())
            .await
            .is_ok();
        if !have_relay {
            eprintln!("no home relay reachable — skipping the relay assertion");
            return;
        }
        assert!(
            addr.relay_url.is_some(),
            "an invite must carry the relay URL as a fallback path, not just the LAN address",
        );
    }

    // ---------- file sync (the blob phase) ----------

    /// A real vault folder for one side of a session, wired to that side's DB.
    struct TestBlobs<'a> {
        conn: &'a Connection,
        dir: std::path::PathBuf,
        on: bool,
        /// Bytes reported through `progress`, so a test can assert the bar would actually move.
        seen_progress: RefCell<Vec<(u64, u64)>>,
    }

    impl<'a> TestBlobs<'a> {
        fn new(conn: &'a Connection, tag: &str, on: bool) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "pushin_proto_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TestBlobs { conn, dir, on, seen_progress: RefCell::new(Vec::new()) }
        }
        fn path(&self) -> String {
            self.dir.to_string_lossy().to_string()
        }
        fn put(&self, rel: &str, body: &[u8]) {
            let p = self.dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        fn read(&self, rel: &str) -> Option<Vec<u8>> {
            std::fs::read(self.dir.join(rel)).ok()
        }
        /// What the engine does inside `changes_since`: fold the folder into the shared index.
        fn reindex(&self) {
            crate::sync::blobs::reindex(self.conn, &self.path()).unwrap();
        }
    }
    impl Drop for TestBlobs<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl BlobStore for TestBlobs<'_> {
        fn enabled(&self) -> bool {
            self.on
        }
        fn wanted(&self) -> Vec<WantedFile> {
            // Mirrors production: the peer's changeset has just landed, so this is the moment their
            // deletions become real — settle them before deciding what is still missing.
            crate::sync::blobs::apply_index_deletions(self.conn, &self.path()).unwrap();
            crate::sync::blobs::wanted(self.conn, &self.path()).unwrap_or_default()
        }
        fn read(&self, hash: &str) -> Option<Vec<u8>> {
            crate::sync::blobs::read_blob(self.conn, &self.path(), hash)
        }
        fn store(&self, rel_path: &str, hash: &str, bytes: &[u8]) -> Result<()> {
            crate::sync::blobs::write_blob(self.conn, &self.path(), rel_path, hash, bytes)
        }
        fn progress(&self, done: u64, total: u64) {
            self.seen_progress.borrow_mut().push((done, total));
        }
    }

    /// Run one full session between two stores over an in-memory duplex.
    async fn session(a: &TestStore, ab: &TestBlobs<'_>, b: &TestStore, bb: &TestBlobs<'_>) {
        let (c1, c2) = tokio::io::duplex(1 << 20);
        let (ar, aw) = tokio::io::split(c1);
        let (br, bw) = tokio::io::split(c2);
        let (ra, rb) = tokio::join!(
            run_session_with_blobs(a, ab, true, ar, aw),
            run_session_with_blobs(b, bb, false, br, bw),
        );
        ra.unwrap();
        rb.unwrap();
    }

    #[tokio::test]
    async fn an_attachment_travels_from_one_device_to_the_other() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "a", true);
        let bb = TestBlobs::new(&b.conn, "b", true);

        // A has a PDF in its vault that B has never seen. B has nothing.
        let body = b"%PDF-1.7 the quarterly report";
        ab.put("Attachments/report.pdf", body);
        ab.reindex();

        session(&a, &ab, &b, &bb).await;

        // The index row travelled as an ordinary LWW row, and the BYTES followed in the blob phase.
        assert_eq!(
            bb.read("Attachments/report.pdf").as_deref(),
            Some(&body[..]),
            "the file itself has to land, not just its index row"
        );
        let index = crate::db::vault_file_index(&b.conn).unwrap();
        assert_eq!(index["Attachments/report.pdf"].hash, crate::vault::hash_bytes(body));
        // And B must not then ask for it again.
        assert!(crate::sync::blobs::wanted(&b.conn, &bb.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_file_larger_than_one_chunk_arrives_whole() {
        // The chunking seam: anything over `blobs::CHUNK` is several frames that have to reassemble
        // in order and hash to the original, or the receiver rejects the whole file.
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "bigA", true);
        let bb = TestBlobs::new(&b.conn, "bigB", true);

        // Deterministic, non-uniform bytes: a run of identical bytes would still pass with a
        // chunk-ordering bug, since every arrangement hashes the same.
        let big: Vec<u8> = (0..(crate::sync::blobs::CHUNK * 2 + 1234))
            .map(|i| (i.wrapping_mul(31) % 251) as u8)
            .collect();
        ab.put("Media/clip.bin", &big);
        ab.reindex();

        session(&a, &ab, &b, &bb).await;

        assert_eq!(bb.read("Media/clip.bin"), Some(big), "a multi-chunk file must reassemble exactly");
    }

    #[tokio::test]
    async fn files_move_in_both_directions_in_one_session() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "bothA", true);
        let bb = TestBlobs::new(&b.conn, "bothB", true);

        ab.put("from-a.txt", b"written on the desktop");
        bb.put("from-b.txt", b"written on the laptop");
        ab.reindex();
        bb.reindex();

        session(&a, &ab, &b, &bb).await;

        assert_eq!(bb.read("from-a.txt").as_deref(), Some(&b"written on the desktop"[..]));
        assert_eq!(ab.read("from-b.txt").as_deref(), Some(&b"written on the laptop"[..]));
    }

    #[tokio::test]
    async fn an_empty_file_still_arrives() {
        // A zero-byte file has no chunks to iterate, so without an explicit empty frame it would be
        // silently skipped and then re-requested on every single session, forever.
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "emptyA", true);
        let bb = TestBlobs::new(&b.conn, "emptyB", true);
        ab.put("placeholder.txt", b"");
        ab.reindex();

        session(&a, &ab, &b, &bb).await;

        assert_eq!(bb.read("placeholder.txt"), Some(Vec::new()));
        assert!(crate::sync::blobs::wanted(&b.conn, &bb.path()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn receiving_a_file_reports_progress_that_ends_at_the_total() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "progA", true);
        let bb = TestBlobs::new(&b.conn, "progB", true);
        ab.put("one.bin", b"0123456789");
        ab.put("two.bin", b"abcdefghij");
        ab.reindex();

        session(&a, &ab, &b, &bb).await;

        let ticks = bb.seen_progress.borrow().clone();
        assert!(!ticks.is_empty(), "the bar needs something to render");
        let (done, total) = *ticks.last().unwrap();
        assert_eq!(total, 20, "both files' bytes are the denominator");
        assert_eq!(done, total, "progress has to finish full, or the bar never completes");
        assert!(ticks.iter().all(|(d, t)| d <= t), "progress must never exceed its total");
    }

    #[tokio::test]
    async fn a_peer_that_predates_file_sync_still_syncs_rows() {
        // Version skew is the failure this negotiation exists to prevent: an older build never
        // answers a Want, so entering the phase against one would desynchronise the choreography and
        // break sync ENTIRELY rather than merely skipping files.
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "skewA", true);
        ab.put("attachment.bin", b"only A can see this");
        ab.reindex();
        a.conn
            .execute("INSERT INTO events(title, start, end, created_at) VALUES('E','s','e','t')", [])
            .unwrap();

        let (c1, c2) = tokio::io::duplex(1 << 20);
        let (ar, aw) = tokio::io::split(c1);
        let (br, bw) = tokio::io::split(c2);
        let (ra, rb) = tokio::join!(
            run_session_with_blobs(&a, &ab, true, ar, aw),
            run_session_with_blobs(&b, &NoBlobs, false, br, bw), // the old build
        );
        ra.unwrap();
        rb.unwrap();

        assert_eq!(
            count(&b.conn, "SELECT count(*) FROM events"),
            1,
            "rows must still cross to a peer that cannot take files"
        );
    }

    #[tokio::test]
    async fn a_device_with_no_vault_folder_skips_the_file_phase() {
        // Same shape as version skew, but the reason is local: nothing configured to sync into.
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "novaultA", true);
        let bb = TestBlobs::new(&b.conn, "novaultB", false); // no vault folder set
        ab.put("doc.txt", b"stays home");
        ab.reindex();

        session(&a, &ab, &b, &bb).await;

        assert!(bb.read("doc.txt").is_none(), "nowhere to put it, so it must not be written");
        assert!(bb.seen_progress.borrow().is_empty(), "and no bar should have appeared");
    }

    #[tokio::test]
    async fn a_blob_we_never_asked_for_is_ignored() {
        // The want-list is the whitelist. This is the only place a peer hands us raw bytes AND a
        // path, so a peer that could choose the path could write anywhere the vault reaches.
        use base64::Engine as _;
        let b = TestStore::new("B", "secret");
        let bb = TestBlobs::new(&b.conn, "unwanted", true);

        let (mut writer, mut reader) = tokio::io::duplex(1 << 16);
        let junk = b"#!/bin/sh\nrm -rf /";
        let sender = async {
            write_msg(
                &mut writer,
                &Msg::Blob {
                    rel_path: "../../evil.sh".into(),
                    hash: crate::vault::hash_bytes(junk),
                    seq: 0,
                    last: true,
                    data: base64::engine::general_purpose::STANDARD.encode(junk),
                },
            )
            .await
            .unwrap();
            write_msg(&mut writer, &Msg::BlobsDone).await.unwrap();
        };
        let mut stats = SessionStats::default();
        let receiver = recv_blobs(&bb, &mut reader, &[], &mut stats);
        let (_, r) = tokio::join!(sender, receiver);
        r.unwrap();

        assert_eq!(stats.files_received, 0, "an unrequested blob must not land");
        assert!(bb.read("evil.sh").is_none());
        assert!(!bb.dir.parent().unwrap().join("evil.sh").exists(), "and nothing escaped the vault");
    }

    #[tokio::test]
    async fn a_deleted_attachment_is_removed_on_the_other_device_too() {
        let a = TestStore::new("A", "secret");
        let b = TestStore::new("B", "secret");
        let ab = TestBlobs::new(&a.conn, "delA", true);
        let bb = TestBlobs::new(&b.conn, "delB", true);
        ab.put("temporary.txt", b"here for now");
        ab.reindex();
        session(&a, &ab, &b, &bb).await;
        assert!(bb.read("temporary.txt").is_some(), "precondition: B received it");

        // A deletes it, and re-indexes — which tombstones the row.
        std::fs::remove_file(ab.dir.join("temporary.txt")).unwrap();
        ab.reindex();
        session(&a, &ab, &b, &bb).await;

        assert!(bb.read("temporary.txt").is_none(), "the delete has to reach the other device");
    }
}
