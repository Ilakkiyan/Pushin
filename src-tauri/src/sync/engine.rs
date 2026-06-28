//! The running sync engine: owns the Iroh endpoint, serves inbound sessions, periodically pulls
//! from known peers, and implements [`SyncStore`] against the live SQLite DB. One per app process,
//! started once a mesh secret exists (the device has created or joined a network).
//!
//! DB access stays behind the app's `Mutex<Connection>` and is never held across an `.await`
//! (gotcha #8): each [`SyncStore`] method takes a short lock, does its work, and releases.

use super::changeset::{self, Change};
use super::protocol::{self, SessionStats, SyncStore};
use super::{identity, state, transport};
use anyhow::{anyhow, Context, Result};
use iroh::endpoint::Incoming;
use iroh::{Endpoint, NodeAddr, NodeId};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// How often the engine proactively pulls from every known peer.
const SYNC_INTERVAL: Duration = Duration::from_secs(20);

pub struct SyncEngine {
    endpoint: Endpoint,
    node_id: String,
    mesh: String,
    db: Arc<Mutex<Connection>>,
    app: AppHandle,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

impl SyncEngine {
    /// Bind the endpoint and start the background loops. Errors if no mesh secret is set yet.
    pub async fn start(
        db: Arc<Mutex<Connection>>,
        app: AppHandle,
        use_relay: bool,
    ) -> Result<Arc<SyncEngine>> {
        let mesh = identity::mesh_secret().context("device has not joined a sync network")?;
        let secret = transport::secret_key(identity::load_or_create_node_key());
        let endpoint = transport::bind(secret, use_relay).await?;
        let node_id = endpoint.node_id().to_string();
        {
            let conn = db.lock().map_err(|_| anyhow!("db poisoned"))?;
            state::set_node_id(&conn, &node_id)?;
        }
        let engine = Arc::new(SyncEngine { endpoint, node_id, mesh, db, app });
        engine.clone().spawn_accept_loop();
        engine.clone().spawn_periodic();
        Ok(engine)
    }

    /// Mint an invite ticket for another device to join this network.
    pub async fn create_invite(&self) -> Result<String> {
        transport::make_ticket(&self.endpoint, &self.mesh).await
    }

    /// Dial one peer and run a full sync session.
    pub async fn sync_with(&self, addr: impl Into<NodeAddr>) -> Result<SessionStats> {
        let (conn, send, recv) = transport::dial(&self.endpoint, addr).await?;
        let stats = protocol::run_session(self, true, recv, send).await?;
        self.note_peer(&stats);
        conn.close(0u32.into(), b"done");
        Ok(stats)
    }

    /// Pull from every known peer (best-effort). Returns how many succeeded.
    pub async fn sync_all_peers(&self) -> usize {
        let peers = {
            match self.db.lock() {
                Ok(conn) => state::list_peers(&conn).unwrap_or_default(),
                Err(_) => return 0,
            }
        };
        let mut ok = 0;
        for p in peers {
            match p.node_id.parse::<NodeId>() {
                Ok(id) => match self.sync_with(id).await {
                    Ok(_) => ok += 1,
                    Err(e) => eprintln!("sync: peer {} failed: {e:#}", p.node_id),
                },
                Err(_) => eprintln!("sync: bad node id {}", p.node_id),
            }
        }
        ok
    }

    /// Close the endpoint on shutdown.
    pub async fn shutdown(&self) {
        self.endpoint.close().await;
    }

    // ---- internals ----

    fn spawn_accept_loop(self: Arc<Self>) {
        let ep = self.endpoint.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let engine = self.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = engine.handle_incoming(incoming).await {
                        eprintln!("sync: inbound session failed: {e:#}");
                    }
                });
            }
        });
    }

    fn spawn_periodic(self: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(SYNC_INTERVAL).await;
                self.sync_all_peers().await;
            }
        });
    }

    async fn handle_incoming(&self, incoming: Incoming) -> Result<()> {
        let conn = incoming.await.context("accepting inbound connection")?;
        let (send, recv) = transport::accept_stream(&conn).await?;
        let stats = protocol::run_session(self, false, recv, send).await?;
        self.note_peer(&stats);
        Ok(())
    }

    /// Record/refresh a peer after a session: store its node id + name and bump last-seen.
    fn note_peer(&self, stats: &SessionStats) {
        if stats.peer.is_empty() {
            return;
        }
        if let Ok(conn) = self.db.lock() {
            let _ = state::upsert_peer(&conn, &stats.peer, &stats.peer_name);
            let _ = state::touch_peer(&conn, &stats.peer, &now_iso());
        }
    }
}

impl SyncStore for SyncEngine {
    fn mesh_secret(&self) -> String {
        self.mesh.clone()
    }
    fn node_id(&self) -> String {
        self.node_id.clone()
    }
    fn device_name(&self) -> String {
        self.db
            .lock()
            .ok()
            .and_then(|c| state::device_name(&c).ok())
            .unwrap_or_else(|| "Pushin device".into())
    }
    fn watermark(&self, peer: &str) -> String {
        self.db.lock().ok().and_then(|c| state::watermark(&c, peer).ok()).unwrap_or_default()
    }
    fn set_watermark(&self, peer: &str, hlc: &str) {
        if let Ok(c) = self.db.lock() {
            let _ = state::set_watermark(&c, peer, hlc);
        }
    }
    fn changes_since(&self, since: &str) -> Result<Vec<Change>> {
        let conn = self.db.lock().map_err(|_| anyhow!("db poisoned"))?;
        // Stamp local edits (advance + persist the clock), then collect the delta.
        let mut clock = state::load_clock(&conn)?;
        changeset::stamp_dirty(&conn, &self.node_id, &mut clock, now_ms())?;
        state::save_clock(&conn, &clock)?;
        changeset::changes_since(&conn, since)
    }
    fn apply(&self, changes: &[Change]) -> Result<String> {
        let (applied, max_hlc) = {
            let conn = self.db.lock().map_err(|_| anyhow!("db poisoned"))?;
            let mut clock = state::load_clock(&conn)?;
            let stats = changeset::apply_changes(&conn, &mut clock, now_ms(), changes)?;
            state::save_clock(&conn, &clock)?;
            (stats.applied, stats.max_hlc)
        };
        // Tell the UI to refresh after a real change landed (lock released first).
        if applied > 0 {
            let _ = self.app.emit("sync-applied", applied);
        }
        Ok(max_hlc)
    }
}
