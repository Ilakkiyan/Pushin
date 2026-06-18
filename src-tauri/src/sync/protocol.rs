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
//! responder → initiator:  Hello, Push, Pull
//! ```
//!
//! The [`SyncStore`] trait is the seam to the database; production wires it to the SQLite changeset
//! functions, tests wire it to two in-memory DBs to prove end-to-end convergence over a real stream.

use super::changeset::Change;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Hard cap on a single framed message (guards against a hostile/buggy peer forcing a huge alloc).
const MAX_FRAME: u32 = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug)]
enum Msg {
    /// Identify + prove mesh membership. `mesh` is the shared secret (the connection is already
    /// E2E-encrypted by QUIC node keys; the secret is the app-level "you belong to my network").
    /// `name` is a human label shown in the peer's device list.
    Hello { node: String, mesh: String, name: String },
    /// "Send me everything with HLC strictly greater than `since`."
    Pull { since: String },
    /// A batch of changes, with the highest HLC contained (the receiver's new watermark).
    Push { changes: Vec<Change>, max_hlc: String },
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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub peer: String,
    pub peer_name: String,
    pub received: usize,
    pub sent: usize,
}

async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    if bytes.len() as u64 > MAX_FRAME as u64 {
        bail!("outgoing sync frame too large: {} bytes", bytes.len());
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        bail!("incoming sync frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

/// Exchange Hellos and verify mesh membership; returns the peer's (node id, device name).
async fn handshake<S, R, W>(store: &S, initiator: bool, r: &mut R, w: &mut W) -> Result<(String, String)>
where
    S: SyncStore,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let me = Msg::Hello { node: store.node_id(), mesh: store.mesh_secret(), name: store.device_name() };
    let peer = if initiator {
        write_msg(w, &me).await?;
        read_msg(r).await?
    } else {
        let p = read_msg(r).await?;
        write_msg(w, &me).await?;
        p
    };
    match peer {
        Msg::Hello { node, mesh, name } if mesh == store.mesh_secret() && !mesh.is_empty() => Ok((node, name)),
        Msg::Hello { .. } => bail!("peer failed mesh authentication"),
        _ => bail!("expected Hello, got something else"),
    }
}

/// Run one full sync session over a stream. Both peers end up converged (subject to LWW).
pub async fn run_session<S, R, W>(
    store: &S,
    initiator: bool,
    mut r: R,
    mut w: W,
) -> Result<SessionStats>
where
    S: SyncStore,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (peer, peer_name) = handshake(store, initiator, &mut r, &mut w).await?;
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

    if initiator {
        write_msg(&mut w, &do_pull(store)).await?; // Pull
        recv_push(store, &mut r, &peer, &mut stats).await?; // Push
        let since = recv_pull(&mut r).await?; // their Pull
        serve_pull(store, &mut w, &since, &mut stats).await?; // our Push
    } else {
        let since = recv_pull(&mut r).await?; // their Pull
        serve_pull(store, &mut w, &since, &mut stats).await?; // our Push
        write_msg(&mut w, &do_pull(store)).await?; // Pull
        recv_push(store, &mut r, &peer, &mut stats).await?; // Push
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
}
