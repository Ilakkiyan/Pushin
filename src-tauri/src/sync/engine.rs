//! The running sync engine: owns the Iroh endpoint, serves inbound sessions, periodically pulls
//! from known peers, and implements [`SyncStore`] against the live SQLite DB. One per app process,
//! started once a mesh secret exists (the device has created or joined a network).
//!
//! DB access stays behind the app's `Mutex<Connection>` and is never held across an `.await`
//! (gotcha #8): each [`SyncStore`] method takes a short lock, does its work, and releases.

use super::blobs;
use super::log;
use super::changeset::{self, Change};
use super::infer::{self, InferReply, InferRequest};
use super::protocol::{self, SessionStats, SyncStore};
use super::{identity, state, transport};
use crate::progress;
use anyhow::{anyhow, bail, Context, Result};
use iroh::endpoint::{Connection, Incoming};
use iroh::{Endpoint, EndpointAddr, EndpointId};
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

/// One line summarising a finished session — the thing you actually want to read when a device
/// "isn't syncing": whether rows moved, whether files moved, and how much.
fn describe(s: &SessionStats) -> String {
    let who = if s.peer_name.is_empty() { s.peer.chars().take(8).collect() } else { s.peer_name.clone() };
    format!(
        "session with {who}: rows {} in / {} out, files {} in ({} bytes) / {} out",
        s.received, s.sent, s.files_received, s.bytes_received, s.files_sent
    )
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
        let node_id = endpoint.id().to_string();
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
    ///
    /// The progress bookends are unconditional: the closing `done` is emitted on the error path too,
    /// or a peer that goes unreachable mid-session would leave the sidebar bar stuck at whatever
    /// fraction it had reached, with nothing left running to ever finish it.
    pub async fn sync_with(&self, addr: impl Into<EndpointAddr>) -> Result<SessionStats> {
        progress::emit(&self.app, progress::SyncProgress::new("device", "rows", ""));
        let out = self.sync_with_inner(addr).await;
        progress::emit(&self.app, progress::SyncProgress::finished("device", ""));
        out
    }

    async fn sync_with_inner(&self, addr: impl Into<EndpointAddr>) -> Result<SessionStats> {
        let (conn, send, recv) = transport::dial(&self.endpoint, addr).await?;
        let blob_store = VaultBlobs::new(self);
        let stats = protocol::run_session_with_blobs(self, &blob_store, true, recv, send).await?;
        log::info(describe(&stats));
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
            match p.node_id.parse::<EndpointId>() {
                Ok(id) => match self.sync_with(id).await {
                    Ok(_) => ok += 1,
                    Err(e) => log::error(format!("peer {} failed: {e:#}", p.node_id)),
                },
                Err(_) => log::error(format!("bad node id {}", p.node_id)),
            }
        }
        ok
    }

    /// Reconcile the vault folder with the shared file index (a no-op when no vault folder is set).
    ///
    /// Errors are logged and swallowed on purpose: a vault folder that has been renamed, unmounted,
    /// or is momentarily unreadable must not take row sync down with it. The files simply do not
    /// move this round.
    fn reindex_vault(&self) {
        let Ok(conn) = self.db.lock() else { return };
        let Some(dir) = crate::db::get_settings(&conn).ok().and_then(|s| s.vault_dir) else { return };
        match blobs::reindex(&conn, &dir) {
            // Only worth a line when it did something — a quiet scan every 20 seconds would bury
            // the entries that actually explain a failure.
            Ok(s) if s.hashed > 0 || s.removed > 0 => log::info(format!(
                "vault scan: {} file(s) indexed, {} newly hashed, {} removed",
                s.indexed, s.hashed, s.removed
            )),
            Ok(_) => {}
            Err(e) => log::error(format!("vault reindex failed: {e:#}")),
        }
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
                        log::error(format!("inbound session failed: {e:#}"));
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
        if conn.alpn() == transport::INFER_ALPN {
            return self.serve_inference(&conn).await;
        }
        let (send, recv) = transport::accept_stream(&conn).await?;
        progress::emit(&self.app, progress::SyncProgress::new("device", "rows", ""));
        let blob_store = VaultBlobs::new(self);
        let stats = protocol::run_session_with_blobs(self, &blob_store, false, recv, send).await;
        progress::emit(&self.app, progress::SyncProgress::finished("device", ""));
        let stats = stats?;
        log::info(describe(&stats));
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
            let id = match p.node_id.parse::<EndpointId>() {
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

/// The engine's [`BlobStore`]: the vault folder on this device, if one is configured.
///
/// Constructed per session and handed to [`protocol::run_session_with_blobs`]. When no vault folder
/// is set, `enabled()` is false and the session never enters the file phase — a device that keeps
/// its vault in SQLite only still syncs rows exactly as before.
pub struct VaultBlobs<'a> {
    engine: &'a SyncEngine,
    vault_dir: Option<String>,
}

impl<'a> VaultBlobs<'a> {
    fn new(engine: &'a SyncEngine) -> Self {
        let vault_dir = engine
            .db
            .lock()
            .ok()
            .and_then(|c| crate::db::get_settings(&c).ok())
            .and_then(|s| s.vault_dir);
        VaultBlobs { engine, vault_dir }
    }
}

impl protocol::BlobStore for VaultBlobs<'_> {
    fn enabled(&self) -> bool {
        self.vault_dir.is_some()
    }

    fn wanted(&self) -> Vec<blobs::WantedFile> {
        let Some(dir) = &self.vault_dir else { return Vec::new() };
        let Ok(conn) = self.engine.db.lock() else { return Vec::new() };
        // The peer's changeset has just landed, so this is the moment their deletions are real.
        // Settling them BEFORE computing the want-list is what stops us asking for the bytes of a
        // file they told us in the same breath to delete.
        if let Err(e) = blobs::apply_index_deletions(&conn, dir) {
            log::error(format!("couldn't apply vault file deletions: {e:#}"));
        }
        blobs::wanted(&conn, dir).unwrap_or_default()
    }

    fn read(&self, hash: &str) -> Option<Vec<u8>> {
        let dir = self.vault_dir.as_ref()?;
        let conn = self.engine.db.lock().ok()?;
        blobs::read_blob(&conn, dir, hash)
    }

    fn store(&self, rel_path: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.vault_dir.as_ref().ok_or_else(|| anyhow!("no vault folder"))?;
        let conn = self.engine.db.lock().map_err(|_| anyhow!("db poisoned"))?;
        // Writing the file fires the vault watcher, which would otherwise read our own write as an
        // external edit. `write_blob` records it in `vault_file_seen` with the landed mtime, so the
        // next reindex recognises the bytes as ours and does not re-hash or re-ship them.
        blobs::write_blob(&conn, dir, rel_path, hash, bytes)
    }

    fn progress(&self, done: u64, total: u64) {
        progress::emit(
            &self.engine.app,
            progress::SyncProgress::new("device", "files", "").at(done, total),
        );
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
        // Bring the vault file index in line with the folder BEFORE stamping, so a file added or
        // deleted on this device ships in this session rather than the next one. Hashing is skipped
        // for anything whose (mtime, size) is unchanged, so the steady-state cost is a directory
        // walk — the first scan of a large vault is the slow one.
        self.reindex_vault();
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
                log::error(format!("couldn't adopt the shared Google link: {e}"));
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
