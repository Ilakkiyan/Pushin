# Mobile → desktop inference bridge

Route AI (chat + embeddings) from a device that can't run a local model (mobile) to **any reachable
desktop peer** over the existing private Iroh mesh. No single "home base"; data replication is
untouched, so every device still works fully offline; only *mobile AI features* degrade when no
desktop is reachable.

## Invariants (don't relitigate)
- **Desktops always use local inference.** "PC-side inference" is a mobile-only concept.
- **Data replication unchanged.** Each device keeps a full offline SQLite replica; notes/tasks/calendar
  always work even when every other machine is off.
- **No fixed home base.** The phone tries whichever desktop peer answers first (first-answer-wins).
- Mesh auth + E2E encryption reused from the sync layer (same mesh secret, same QUIC node keys).

## Decisions (locked with the user)
- **Borrow embeddings too** (not keyword-only): the bridge carries both `Chat` and `Embed` requests, so
  mobile semantic recall works via a desktop's embed server.
- **Requester timeout**: each peer attempt is bounded (default 60s, matches the local first-run wait);
  on timeout/error, move to the next peer.
- **First-answer-wins**: iterate known peers, first successful reply is used.

## Architecture: a second channel mirroring the sync layer
The sync code already provides an authenticated, E2E-encrypted, NAT-traversing mesh with a
transport-agnostic, unit-tested protocol. The bridge is an **independent channel** so it can't disturb
the sync choreography.

### 1. `sync/frame.rs` (new): shared length-prefixed JSON framing
Extract `read_msg`/`write_msg` (currently private in `protocol.rs`) into generic
`read_frame<T: DeserializeOwned>` / `write_frame<T: Serialize>` with the existing `MAX_FRAME` cap.
`protocol.rs` and `infer.rs` both use it.

### 2. `sync/infer.rs` (new): the inference protocol (transport-agnostic, unit-tested)
- `enum InferRequest { Chat { model, messages, schema }, Embed { model, input } }`
- `enum InferReply { Chat(serde_json::Value), Embed(Vec<f32>) }`
- Wire messages: `Hello { node, mesh, name }`, `Request(InferRequest)`, `Reply { ok, reply?, error? }`.
- `serve_infer(mesh, name, node, r, w, handle)`: responder: Hello-auth → `handle(req).await` →
  `Reply`. `handle` is an **async closure** (`Fn(InferRequest) -> Future<Output = Result<InferReply>>`),
  so there's no async-trait dependency and it's trivially fakeable in tests.
- `request_infer(mesh, name, node, req, r, w) -> Result<InferReply>`: initiator: Hello-auth → send
  `Request` → read `Reply`.
- Auth: verify the peer's mesh secret in `Hello`, identical to `protocol::handshake`.
- Tests over `tokio::io::duplex`: fake handler returns a canned Chat/Embed reply; wrong-mesh rejected.

### 3. `sync/transport.rs`
- `pub const INFER_ALPN: &[u8] = b"pushin-infer/0";`; bind both ALPNs.
- `dial_infer(ep, addr)` (connect with `INFER_ALPN`, `open_bi`).

### 4. `sync/engine.rs`
- `handle_incoming`: dispatch on `conn.alpn()`: sync ALPN → `run_session`; `INFER_ALPN` →
  `infer::serve_infer(...)` with a handler that runs local chat/embed.
- `SyncEngine` gains a `reqwest::Client` (passed from `AppState.http` at `start`) to call the local
  `llama-server` (`llm::chat_json`) and embed server (`hermes::embed_text`).
- Local handler: `Chat` → read `llm_base_url`/`model_id` from settings, `chat_json`; `Embed` →
  `embed_base_url()` + `embed_model`, `embed_text`.
- `can_infer()` = local `llm::health` up (true desktop / false mobile).
- `request_chat_from_peers(...)` / `request_embed_from_peers(...)`: walk `state::list_peers`,
  `dial_infer` each under a per-attempt `tokio::time::timeout`, **first Ok wins**; none → `Err`.
- Responder concurrency guard: a semaphore caps simultaneous inbound inference streams so a desktop
  can't be overwhelmed.

### 5. Routing seam: transparent fallback (BUILT this way)
Rather than thread a router param through the eval-battery-guarded `parser` (5+ call sites), the seam is
a **process-wide fallback registered on the existing functions**, with zero churn to `parser`/`commands`:
- `llm.rs`: `PEER_CHAT: Mutex<Option<PeerChat>>` + `register_peer_chat`. `chat_json` checks
  `health(local)` first; if unreachable and a peer is registered, routes `(messages, schema)` to it.
- `hermes.rs`: `PEER_EMBED` + `register_peer_embed`. `embed_text` (and `embed_batch`, per-input) fall
  back the same way when the local embed server is down.
- `engine.rs::register_peer_fallbacks` (called on `start`) registers both with a `Weak<SyncEngine>` that
  calls `request_chat` / `request_embed` (first-answer-wins over peers).

Desktops keep a reachable local server, so `health()` passes and they **never** hit the fallback
(PC-side inference stays mobile-only). Mobile has no local server → every `chat_json`/`embed_text`
transparently routes to a paired desktop. No caller signatures changed.

### 6. UX (frontend, follow-up)
Mobile shows an AI-source chip: "AI via ‹Desktop›" when routed, "AI unavailable, no desktop online"
when none. Data UI never blocks on this.

## Testing
- `sync/infer.rs`: unit tests over an in-memory duplex (canned chat + embed replies; wrong-mesh
  rejected): deterministic, no network/model, mirrors `protocol.rs`.
- Live phone↔desktop path is only provable on two real devices (same caveat as the rest of the mesh).

## Non-goals (v1)
- No token streaming (full completion returned).
- No cross-peer load-balancing / model preference (first-answer-wins).
- Desktops never route out: local only.
