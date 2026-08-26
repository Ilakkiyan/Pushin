//! The running sync engine: owns the Iroh endpoint, serves inbound sessions, periodically pulls
//! from known peers, and implements [`SyncStore`] against the live SQLite DB. One per app process,
//! started once a mesh secret exists (the device has created or joined a network).
//!
//! DB access stays behind the app's `Mutex<Connection>` and is never held across an `.await`
//! (gotcha #8): each [`SyncStore`] method takes a short lock, does its work, and releases.

use super::changeset::{self, Change};
use super::infer::{self, InferReply, InferRequest};
use super::protocol::{self, SessionStats, SyncStore};
use super::{identity, state, transport};
use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::{Connection, Incoming};
use iroh::{Endpoint, NodeAddr, NodeId};
use rusqlite::Connection as SqlConnection;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};
use tokio::sync::Semaphore;

/// How often the engine proactively pulls from every known peer.
const SYNC_INTERVAL: Duration = Duration::from_secs(20);
/// Per-peer timeout for a borrowed-inference attempt (a big model can be slow; matches the local
/// first-run wait). On timeout we move to the next peer.
const INFER_TIMEOUT: Duration = Duration::from_secs(60);
/// Cap on simultaneous *inbound* inference requests we serve, so a phone can't overwhelm a desktop.
const MAX_INFLIGHT_INFER: usize = 2;
/// How long a responder waits for the initiator to close before dropping the connection itself.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

pub struct SyncEngine {
    endpoint: Endpoint,
    node_id: String,
    mesh: String,
    db: Arc<Mutex<SqlConnection>>,
    app: AppHandle,
    /// HTTP client for calling this device's OWN local chat/embed servers when serving a peer.
    http: reqwest::Client,
    /// Limits concurrent inbound inference we serve (see [`MAX_INFLIGHT_INFER`]).
    infer_sem: Arc<Semaphore>,
    /// Whether this endpoint was bound with relays enabled (invite minting needs to know).
    use_relay: bool,
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
        db: Arc<Mutex<SqlConnection>>,
        app: AppHandle,
        http: reqwest::Client,
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
        let infer_sem = Arc::new(Semaphore::new(MAX_INFLIGHT_INFER));
        let engine =
            Arc::new(SyncEngine { endpoint, node_id, mesh, db, app, http, infer_sem, use_relay });
        engine.clone().spawn_accept_loop();
        engine.clone().spawn_periodic();
        engine.register_peer_fallbacks();
        Ok(engine)
    }

    /// Register this engine as the process-wide peer-inference fallback: when the local chat/embed
    /// server is unreachable (a phone with no model), `llm::chat_json` / `hermes::embed_text` route
    /// through here to a paired desktop. Uses a `Weak` so a later leave/rejoin can replace it cleanly.
    fn register_peer_fallbacks(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        crate::llm::register_peer_chat(Box::new(move |messages, schema| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(e) => e.request_chat(messages, schema).await,
                    None => bail!("sync engine no longer running"),
                }
            })
        }));
        let weak = Arc::downgrade(self);
        crate::hermes::register_peer_embed(Box::new(move |input| {
            let weak = weak.clone();
            Box::pin(async move {
                match weak.upgrade() {
                    Some(e) => e.request_embed(input).await,
                    None => bail!("sync engine no longer running"),
                }
            })
        }));
    }

    /// The mesh secret this engine bound with. Compared against the keychain to detect that the
    /// device has since joined a *different* network and the engine must be rebuilt.
    pub fn mesh(&self) -> &str {
        &self.mesh
    }

    /// Mint an invite ticket for another device to join this network.
    pub async fn create_invite(&self) -> Result<String> {
        transport::make_ticket(&self.endpoint, &self.mesh, self.use_relay).await
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
        // Dispatch by ALPN: the inference bridge is a separate channel from changeset sync.
        if conn.alpn().as_deref() == Some(transport::INFER_ALPN) {
            return self.serve_inference(&conn).await;
        }
        let (send, recv) = transport::accept_stream(&conn).await?;
        let stats = protocol::run_session(self, false, recv, send).await?;
        self.note_peer(&stats);
        // We sent the closing `Bye`; the initiator closes as soon as it reads that. Wait for the
        // close rather than returning, because dropping `conn` abandons unacknowledged stream data
        // and would race the `Bye` off the wire.
        let _ = tokio::time::timeout(CLOSE_WAIT, conn.closed()).await;
        Ok(())
    }

    /// Serve one inbound inference request by running it on THIS device's local model (only a device
    /// with a running local server can usefully answer — a mobile peer will simply error out).
    async fn serve_inference(&self, conn: &Connection) -> Result<()> {
        let _permit = self.infer_sem.clone().acquire_owned().await.context("inference permit")?;
        let (send, recv) = transport::accept_stream(conn).await?;
        let (mesh, node, name) = (self.mesh.clone(), self.node_id.clone(), self.device_name());
        infer::serve_infer(&mesh, &node, &name, recv, send, |req| self.run_local(req)).await
    }

    /// Run a request against this device's own local chat/embed server (used when serving a peer).
    async fn run_local(&self, req: InferRequest) -> Result<InferReply> {
        let (llm_base, model, embed_model) = {
            let conn = self.db.lock().map_err(|_| anyhow!("db poisoned"))?;
            let s = crate::db::get_settings(&conn)?;
            (s.llm_base_url, s.model_id, s.embed_model)
        };
        match req {
            // Ignore the requester's model name — use whatever THIS device has configured/running.
            InferRequest::Chat { messages, schema, .. } => {
                let v = crate::llm::chat_json(&self.http, &llm_base, &model, messages, schema).await?;
                Ok(InferReply::Chat(v))
            }
            InferRequest::Embed { input, .. } => {
                let base = crate::model_manager::embed_base_url();
                let v = crate::hermes::embed_text(&self.http, &base, &embed_model, &input).await?;
                Ok(InferReply::Embed(v))
            }
        }
    }

    /// Borrow a peer's model for a chat completion: try each known peer, **first success wins**.
    /// Errors if no reachable peer can run it (the "no desktop online" fallback for mobile).
    pub async fn request_chat(&self, messages: Value, schema: Value) -> Result<Value> {
        match self.request_from_peers(InferRequest::Chat { model: String::new(), messages, schema }).await? {
            InferReply::Chat(v) => Ok(v),
            _ => bail!("peer returned the wrong reply kind for a chat request"),
        }
    }

    /// Borrow a peer's embed server for one vector (first success wins).
    pub async fn request_embed(&self, input: String) -> Result<Vec<f32>> {
        match self.request_from_peers(InferRequest::Embed { model: String::new(), input }).await? {
            InferReply::Embed(v) => Ok(v),
            _ => bail!("peer returned the wrong reply kind for an embed request"),
        }
    }

    async fn request_from_peers(&self, req: InferRequest) -> Result<InferReply> {
        let peers = match self.db.lock() {
            Ok(conn) => state::list_peers(&conn).unwrap_or_default(),
            Err(_) => bail!("db poisoned"),
        };
        if peers.is_empty() {
            bail!("no paired devices to borrow inference from");
        }
        let (mesh, node, name) = (self.mesh.clone(), self.node_id.clone(), self.device_name());
        for p in peers {
            let id = match p.node_id.parse::<NodeId>() {
                Ok(i) => i,
                Err(_) => continue,
            };
            let attempt = async {
                let (conn, send, recv) = transport::dial_infer(&self.endpoint, id).await?;
                let reply = infer::request_infer(&mesh, &node, &name, req.clone(), recv, send).await;
                conn.close(0u32.into(), b"done");
                reply
            };
            match tokio::time::timeout(INFER_TIMEOUT, attempt).await {
                Ok(Ok(reply)) => return Ok(reply), // first answer wins
                Ok(Err(e)) => eprintln!("infer: peer {} failed: {e:#}", p.node_id),
                Err(_) => eprintln!("infer: peer {} timed out", p.node_id),
            }
        }
        bail!("no reachable device could run inference")
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
            // A peer may have just handed us the shared Google link (or retracted it). Project it
            // onto this device's local Google state so pairing auto-applies the connection.
            // Idempotent and cheap, so it runs unconditionally rather than sniffing the batch.
            if let Err(e) = crate::db::adopt_google_link(&conn) {
                eprintln!("sync: couldn't adopt the shared Google link: {e}");
            }
            (stats.applied, stats.max_hlc)
        };
        // Tell the UI to refresh after a real change landed (lock released first).
        if applied > 0 {
            let _ = self.app.emit("sync-applied", applied);
        }
        Ok(max_hlc)
    }
}
