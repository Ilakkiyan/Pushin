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

use super::changeset::Change;
use super::frame::{read_frame, write_frame};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

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

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    pub peer: String,
    pub peer_name: String,
    pub received: usize,
    pub sent: usize,
}

/// Read one framed `Msg` (thin alias so the sync choreography below reads unchanged).
async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
    read_frame(r).await
}
/// Write one framed `Msg`.
async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, msg: &Msg) -> Result<()> {
    write_frame(w, msg).await
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
        recv_bye(&mut r).await?; // they applied it — safe to close
    } else {
        let since = recv_pull(&mut r).await?; // their Pull
        serve_pull(store, &mut w, &since, &mut stats).await?; // our Push
        write_msg(&mut w, &do_pull(store)).await?; // Pull
        recv_push(store, &mut r, &peer, &mut stats).await?; // Push
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
}
