# Pushin — architecture deep-dives

Reference detail moved out of `CLAUDE.md` to keep that file lean (it's auto-loaded into the AI's context
every turn). Read the relevant section here when working on that subsystem. The `docs/developer-guide/*`
pages (architecture, backend, frontend, ipc, parser-scheduler, testing-releases) cover much of the same
ground for humans; this file is the terse, hard-won engineering detail.

---

## File map

**Rust (`src-tauri/src/`)**
- `lib.rs` — Tauri setup, command registration, app-exit kills the llama-server child.
- `commands.rs` — the IPC surface. **Never hold the DB `Mutex<Connection>` across an `.await`** (async commands must stay `Send`); use short scoped lock blocks.
- `model.rs` — `Settings`, `Event`, `Block`, `Task`, `GoogleAccount`, plus `Person`, `FocusSession`, `EntityKind`/`ContextItem` (Context Engine), `Briefing`, `MeetingBrief`/`AttendeeBrief`, etc.
- `db.rs` — all SQL. Migrations are `user_version`-gated (`0001_init` … `0017`). `0002` calendar accounts; `0008` evolves `notes` into vault pages + `page_links`; `0009` adds `daily_date`, `entity_links`, an `inbox` flag; `0010` adds `labels` + polymorphic `entity_labels`; `0011` makes booking public (slug/share_token/enabled + booking `event_id`); `0012` adds `entity_index` (Context Engine); `0013` adds `people` (CRM); `0014` adds `focus_sessions`; `0015_sync` adds `uuid`/`updated_hlc`/`dirty` + change-capture triggers to every synced table, plus `sync_tombstones`/`sync_peers`/`sync_self` (generated from `sync::schema::TABLES`); `0016` adds `notes.rel_path` (two-way file vault); `0017` adds `habits.preferred_minute` (learned habit time, sync-safe since 0015 reads columns dynamically). Page/graph/daily/inbox/entity-link/label CRUD, `resolve_task_prefs`, people CRUD, focus sessions + `estimation_samples`, `entity_index` CRUD + `entities_for_index`/`entity_neighbors`, keyword `suggest_labels` all live here. `open`/`test_conn` call `sync::register_functions` before migrating (the 0015 triggers call `sync_capturing()`).
- `model_manager.rs` — model + engine auto-download, `llama-server` spawn/health, `MODELS` list. Cross-platform: picks the llama.cpp release asset per OS/arch (asset substrings are extension-less since llama.cpp churns extensions). macOS/Linux `.tar.gz` unpack via system `tar`; Windows `.zip` via the in-process `zip` crate. Extracts to a staging dir then **flattens binary + co-located libs into `bin/`**. Spawn sets `LD_LIBRARY_PATH` (Linux) and `CREATE_NO_WINDOW` (Windows). **CUDA auto-download shipped** (see memory `gpu-inference`): `prefer_cuda()` + `cuda_asset_candidates()` pull a GPU build + cudart when an NVIDIA GPU is present, with a CPU fallback; `-ngl 99` is added when a `ggml-cuda`/`ggml-metal`/`ggml-hip` lib is in `bin/`.
- `llm.rs` — `chat_json(messages, schema)`; sampling tuned to stop runaway (see gotchas in CLAUDE.md). Holds the process-wide `PEER_CHAT` fallback (`register_peer_chat`): if the local server is unreachable and a peer is registered, `chat_json` transparently routes to a desktop peer (mobile inference bridge — see that section).
- `parser.rs` — **the trickiest file.** NL→plan, day-word→date, dedupe, edit-merge. See gotchas in CLAUDE.md.
- `scheduler.rs` — auto-scheduler + `parse_dt`/`fmt_dt` + `estimation_factor` (adaptive: learn real durations from focus actuals; soft, fallback 1.0; applied in `schedule_service::reschedule_inner`). Has unit tests.
- `calendar/google.rs` — Google two-way sync (OAuth/API/sync engine). `calendar/mod.rs` just declares `google` (SQLite is the source of truth; the old `CalendarProvider`/LocalProvider indirection was removed).
- `booking.rs` — booking-page availability (reuses scheduler free-slots) + invitee→`people` upsert on confirm.
- `booking_server.rs` — local `TcpListener` HTTP server for the public booking page (slots/book/cancel), hardened (body cap, thread-per-connection + in-flight cap, global rate limit, XSS escaping — see `SECURITY_TEST_PLAN.md`).
- `context.rs` — Context Engine: deterministic `*_text` projections, FNV-1a `text_hash`, `needs_index_work`, `merge_and_trim`, `ContextBundle`. (Ranking is `hermes::rank_items`; `reindex_all` + `recall_context` live in `commands.rs`.)
- `briefing.rs` — pure Daily Briefing assembler (today's events + due/overdue tasks + scheduled focus minutes).
- `meeting.rs` — pure Meeting Companion brief: event → booked attendees (→ `people`) + history + linked notes.
- `sync/` — device-to-device sync (private Iroh mesh + changeset log) **and** the mobile→desktop inference bridge. `schema.rs` = synced-table registry + generated `0015_sync` migration; `changeset.rs` = build/apply row deltas (FK→uuid, polymorphic, LWW, tombstones, deferred FK fixup); `hlc.rs` = Hybrid Logical Clock; `state.rs` = `sync_self`/`sync_peers`; `identity.rs` = keychain node key + mesh secret; `frame.rs` = length-prefixed JSON framing (`MAX_FRAME` cap) shared by both mesh protocols; `protocol.rs` = transport-agnostic sync wire protocol; `infer.rs` = transport-agnostic inference-bridge protocol (`InferRequest`/`InferReply`, `serve_infer`/`request_infer`); `transport.rs` = the **only** Iroh-touching file (binds the sync + `INFER_ALPN` channels; `dial_infer`); `engine.rs` = the running engine (accept loop dispatched by ALPN + periodic pull + `SyncStore` impl + peer-inference fallbacks). `sync_capturing()` (a registered SQL fn) gates the change-capture triggers; suppression is **thread-local** (`with_capture_suppressed`).

**Frontend (`src/`)**
- `lib/ipc.ts` — typed wrappers over every Tauri command + shared types (incl. `Page`, `PageGraph`).
- `lib/blocks.ts` — BlockNote helpers: blocks→plaintext, block JSON↔page, `[[link]]` title extraction.
- `lib/editorSchema.tsx` — the BlockNote schema + the custom `pageLink` inline-content (the `[[wikilink]]` chip).
- `state/store.ts` — Zustand store; `mutate()` runs a change → stores conflicts → refresh → `maybeSync()` (debounced Google sync). Holds `pages`/`currentPageId` + page CRUD and `view`/`sidebarCollapsed`.
- `panes/` — `ChatPane` (+ memory chips), `CalendarPane` (24h grid, drag-to-move/pin, day-header daily-note, `BriefingCard`, event-detail popover), `MonthPane`, `TaskListPane` (+ Focus timer), `ProjectsPane`, `HabitsPane`, `SettingsPane`, `BookingPane`, `PeoplePane`, `VaultPane`, `GraphPane`, `InboxPane`, `LabelPane`.
- `components/` — `Sidebar`, `VaultTree`, `PageEditor`, `CommandPalette`, `LabelPicker`, `BriefingCard`, `QuickCapture`, `TitleBar`, `DevicesSync`, `InferenceSetup`, `ConflictBanner`, `OnboardingModal`, `UpdateBanner`, `MobileShell`.
- `lib/import.ts` — vault importer (folder picker → headless BlockNote markdown→blocks → pages).
- **Editor stack:** `@blocknote/{core,react,mantine}` + `react-force-graph-2d`. Both client-side/offline.

---

## Google Calendar sync (`calendar/google.rs`)
- **OAuth2 + PKCE via system browser + loopback `TcpListener`** (Desktop client → no redirect URI to register). Token refresh implemented.
- **Pull** incremental via `syncToken` (full-window fallback on 410). **Push** local `source='manual'` events (insert/patch by `external_id`) + task blocks (full mirror).
- **Echo/dup prevention:** only push `source='manual'`; block events are tagged `extendedProperties.private.pushinKind=block`; pull **skips** tagged events; blocks are **delete+recreate each sync** (correct but churny — smarter diffing is a TODO).
- **Tokens stored in SQLite** (`calendar_accounts`, migration 0002) — **moving to OS keychain is a TODO.**
- **Requires the user's own Google OAuth Desktop client** (Client ID/secret pasted in Settings; Calendar API enabled; self added as test user). Steps in `README.md`. **Built but NOT tested live** — first connect is the real test; likely first snags are missing test-user or un-enabled Calendar API (readable errors surface in Settings).

---

## Device sync — private peer-to-peer mesh (`sync/`, migration `0015_sync`)
Run Pushin on multiple devices and keep them in sync **without a cloud**: data flows device→device over a private **Iroh** QUIC mesh (E2E-encrypted), joined by a shared **mesh secret**. SQLite stays the source of truth; a **custom changeset log** reconciles state with last-writer-wins.
- **Two-layer split.** Everything above `transport.rs` is transport-agnostic and unit-tested over an in-memory duplex; `transport.rs` is the only Iroh-touching file. Protocol/changeset have full unit coverage; the actual mesh handshake/NAT-traversal is **only provable on two real machines**.
- **Identity & the "key" (`identity.rs`, OS keychain via `secrets`).** Each device has a persisted Iroh node key (its NodeId is its address). A per-network **mesh secret** is the shared key. Pairing ticket = base32(NodeAddr + mesh secret), shown as an invite code; the joiner adopts the secret and does an initial sync.
- **Global ids, not local rowids.** Integer PKs collide across devices, so `0015` adds a `uuid` to every synced table; FK columns ship as the referenced row's uuid and resolve back on apply (`changeset.rs`). Polymorphic refs (`entity_links`/`entity_labels`) resolve via `entity_kind`→table. Out-of-order FKs use a deferred-fixup loop.
- **Change capture = triggers, not instrumented CRUD.** `0015` adds `AFTER INSERT/UPDATE/DELETE` triggers that stamp uuids, mark rows `dirty`, and write `sync_tombstones` — so the ~50 `db.rs` mutation functions are untouched. Triggers consult `sync_capturing()`; our own build/apply writes run inside `with_capture_suppressed`. **Suppression is thread-local.**
- **HLC + LWW (`hlc.rs`).** A Hybrid Logical Clock gives a total order robust to clock skew; higher HLC wins per row. `stamp_dirty` assigns HLCs, `changes_since(hlc)` is the delta a peer pulls, per-peer **watermarks** in `sync_peers`. **Granularity is row-level** (field-level is a follow-up).
- **What syncs:** all user content (tasks/events/projects/habits/notes+links/labels+joins/people/focus/bookings/event_types + all blocks). **Excluded** (device-local / re-derived): `settings`, `calendar_accounts`+Google tokens, Iroh keys, `entity_index`, and `embedding` columns (each device re-embeds; writing an embedding is wrapped in `with_capture_suppressed`).
- **Engine (`engine.rs`).** Started at boot if paired (best-effort, never blocks startup); owns the endpoint, an accept loop + a **20s periodic pull**; impls `SyncStore` (short locked ops, never across `.await`). On applying remote changes it emits a Tauri `sync-applied` event; `App.tsx` re-`load()`s. Endpoint closes on app exit.
- **Commands/UI.** `sync_status`/`sync_create_invite`/`sync_join`/`sync_now`/`sync_remove_peer`/`sync_set_device_name`/`sync_set_relay`/`sync_leave` → `ipc.ts` → **Settings ▸ Devices & sync** (`components/DevicesSync.tsx`).
- **Relay/privacy.** Default uses n0 relays for NAT traversal (encrypted QUIC only); a Settings toggle switches to LAN/direct-only.
- ⚠️ **Iroh is pinned to `0.90`, not `1.0`.** iroh 1.0's `netwatch` forces `windows-core 0.62`, whose `wmi 0.18.4` won't compile against the `windows-core 0.61` Tauri 2.11 uses (a real Windows build break). iroh 0.90's chain is self-consistent. Needs rusqlite `functions` + `data-encoding`. See memory `device-sync`.
- **Known follow-ups:** scalar-HLC watermark can re-ship a foreign-authored row once (idempotent); field-level LWW; smarter block handling; persisting full NodeAddr per peer.

---

## Mobile → desktop inference bridge (`sync/infer.rs`, `sync/frame.rs`)
Lets a device that **can't run a local model** (mobile) borrow AI from **any reachable desktop peer** over the *same* private Iroh mesh. Plan + locked decisions: `MOBILE_INFERENCE_PLAN.md`. Full offline data replication is untouched — only *mobile AI features* degrade when no desktop is reachable.
- **A second, independent channel.** A separate ALPN (`INFER_ALPN = pushin-infer/0`, bound alongside the sync ALPN in `transport.rs`; `dial_infer` opens it) so the inference choreography can't disturb sync. `engine.rs::handle_incoming` dispatches on `conn.alpn()`: sync → `run_session`, infer → `infer::serve_infer`.
- **Protocol (`infer.rs`, transport-agnostic + unit-tested over an in-memory duplex).** `InferRequest::{Chat { model, messages, schema }, Embed { model, input }}` → `InferReply::{Chat(Value), Embed(Vec<f32>)}`. Fixed choreography: mesh-authenticated `Hello` (both sides, same mesh secret as sync) → `Request` → `Reply`. `serve_infer`'s `handle` is an **async closure** (no async-trait dep, trivially fakeable).
- **Framing (`frame.rs`).** `read_frame`/`write_frame`: 4-byte big-endian length + JSON, `MAX_FRAME` (128 MiB) cap; shared by `protocol.rs` (sync) and `infer.rs`.
- **Responder = a desktop.** `engine.rs` holds a `reqwest::Client`; the infer handler runs `Chat` via `llm::chat_json` (settings' `llm_base_url`/`model_id`) and `Embed` via `hermes::embed_text` (`embed_base_url()`). A semaphore caps concurrent inbound streams so a desktop can't be swamped. `can_infer()` = local `llm::health` up (true on desktop, false on mobile).
- **Transparent routing seam (no `parser`/`commands` churn).** Rather than thread a router through the eval-guarded parser, fallback is registered on the existing functions: `llm.rs` `PEER_CHAT` + `hermes.rs` `PEER_EMBED` (`register_peer_chat`/`register_peer_embed`). `chat_json`/`embed_text` check local `health()` first; if down **and** a peer fallback is registered, they route out. `engine.rs::register_peer_fallbacks` (called on `start`, holds a `Weak<SyncEngine>`) wires both to `request_chat`/`request_embed`, which walk `state::list_peers`, `dial_infer` each under a per-attempt `tokio::time::timeout`, **first Ok wins**. Desktops keep a reachable local server so `health()` passes and they **never** route out (PC-side inference stays mobile-only); no caller signatures changed.
- **Live phone↔desktop path is only provable on two real devices** (same caveat as the rest of the mesh). Non-goals (v1): no token streaming, no load-balancing/model preference, desktops never route out.

---

## Auto-update from GitHub Releases (`tauri-plugin-updater`)
Official Tauri 2 updater — desktop-only (`updater`+`process` deps gated to `cfg(not(any(target_os="android", target_os="ios")))`; registered under `#[cfg(desktop)]`).
- **Data preserved by construction.** The updater swaps only the app bundle; `pushin.db`, `models/*.gguf`, `bin/` engine live in app-data and are never touched. Migrations are additive + `user_version`-gated. No backup/restore code needed.
- **Config (`tauri.conf.json`):** `bundle.createUpdaterArtifacts: true` + `plugins.updater` with inline **pubkey** + `endpoints` → `.../releases/latest/download/latest.json` (Windows `installMode: passive`). Capabilities add `updater:default` + `process:allow-restart`.
- **Frontend:** `lib/updates.ts` (`checkForUpdate` swallows errors → `null`; `installUpdate` = `downloadAndInstall` + `relaunch()`). `components/UpdateBanner.tsx` (desktop branch only). Settings → Updates adds version chip + check/install. Tested in `UpdateBanner.test.tsx`.
- **Releases:** `release.yml` passes `TAURI_SIGNING_PRIVATE_KEY` to `tauri-action`. `releaseDraft: false` so `releases/latest/...` resolves. ⚠️ **Bump the version** (`package.json` + `tauri.conf.json` + `Cargo.toml`) before each tag.
- **Release-pipeline gotchas (hard-won; releases run unattended — a red ❌ is invisible unless you look):**
  - **Lockfile npm-major drift.** `npm ci` fails `EUSAGE — Missing: react@…` when `package-lock.json` was generated by a different npm major than CI's. **Fixed** by pinning CI to the dev Node via `.nvmrc` (24) + `node-version-file` in the workflows.
  - **macOS-only Rust break.** `netwatch` (via iroh) uses socket2's `all`-gated `Type::RAW` in its macOS/BSD netmon — only macOS compiles it. **Fixed** with `socket2 = { version = "0.5", features = ["all"] }`. Verify without a Mac: `rustup target add aarch64-apple-darwin` then `cargo check --target aarch64-apple-darwin`.
  - **Universal macOS binary.** Ship one `--target universal-apple-darwin`. Matrix is 3 jobs (universal-mac / windows / linux).
  - Reading CI logs needs auth, but **job/step metadata is public**: `GET /repos/{o}/{r}/actions/runs/{id}/jobs` reveals which step failed.

---

## The vault — Notion docs + Obsidian links/graph (`hermes.rs`, `db.rs` pages, `notes` table)
The flat Hermes notes grew into full vault pages. The `notes` table is kept (preserves embeddings) and extended by `0008_pages` with `title`, `icon`, `parent_id` (page tree), `content_json` (BlockNote blocks), `sort_order`, `archived`, plus a `page_links` table (one row per `[[wikilink]]`). `content` stays the derived plaintext backing recall/search. Frontend `Page`, Rust `model::Page`.
- **Pages API:** `db.rs` (`list_pages`/`get_page`/`insert_page`/`update_page`/`move_page`/`set_page_links`/`page_backlinks`/`search_pages`/`page_graph`, `derive_title`) + matching `commands.rs`. Unit-tested in `db.rs` `mod tests`. Legacy notes (NULL title/content_json) open as a plain paragraph doc.
- **Wikilinks (`[[`):** the editor's `pageLink` carries `{pageId, title}`. On save the frontend extracts titles and `update_page` rebuilds `page_links` (`set_page_links` resolves title→id; unresolved = a "ghost" with NULL `target_id`). `page_graph`/`page_backlinks` re-resolve ghosts by title at read time, so a link lights up the moment its page exists.
- **Editor save loop:** `PageEditor` debounces (~600ms) + flushes on unmount; sends `content`, `content_json`, link titles. Embedding reuses the Hermes lock dance (drop lock before `.await`).
- **Embeddings = all-in-one, zero setup:** a **second `llama-server` in `--embeddings` mode** on `EMBED_PORT` (8181), serving bge-small-en-v1.5 Q8 (~37 MB, 384-dim). `ensure_embeddings` (idempotent, best-effort) downloads + spawns it; triggered from `store.load()` and after "Start the AI". Second child in `AppState.embed_server`, killed on exit. Hermes embeds via `model_manager::embed_base_url()`.
- **Recall = graceful degradation:** `hermes_recall` embeds the query + ranks by cosine; falls back to keyword overlap if embeddings unavailable. Result carries `mode: "semantic" | "keyword"`.
- **Pure + tested:** `cosine`, `keyword_score`, f32↔BLOB codec (`cargo test --lib hermes`).
- **Calendar ↔ vault bridge:** Daily Notes (`get_or_create_daily`) + entity links (`entity_links`, a task/event ↔ its notes page, via `openEntityNote`).
- **AI over the vault:** `hermes::recall` powers auto-recall (top notes into the planner prompt, semantic-only + score-gated), chat→memory (`parser::extract_memories`), semantic Cmd-K, and ask-your-vault (`vault_ask` — local RAG with citations).
- **Frictionless layer:** quick capture (`Cmd/Ctrl+Shift+N` → `capture_note` → Inbox), Inbox triage, Markdown/Obsidian import (`read_markdown_dir` + `lib/import.ts`), editor `/` templates.
- **Two-way markdown file vault (built, live-unverified):** the SQLite vault mirrors to real `.md` files in a user-picked folder. Export (`lib/vaultExport.ts` → `vault_write` → `vault.rs::write_file`), Watch (`notify` → `vault-changed` → `lib/vaultImport.ts`). `notes.rel_path` (0016) = page↔file map. Echo guard: `vault_write` records a content hash per path; the watcher skips matching events. See memory `vault-two-way-files`.

---

## The Context Engine — cross-entity recall (`context.rs`, `entity_index`, migration 0012)
The shared retrieval spine: every feature pulls context through one path. Full plan: `CONTEXT_ENGINE_PLAN.md`; build log: `DEVLOG.md`.
- **`entity_index`** (0012): one polymorphic row per entity (`entity_kind` ∈ task/event/page/person/goal) with projected `text`, a stable FNV-1a `text_hash`, and an LE-f32 `embedding` (NULL until indexed).
- **Ranking generalized:** `hermes::rank_items` over `ContextItem`; `rank_notes` is a thin adapter.
- **Reindex** (`commands::reindex_all`, best-effort): projects all entities (`db::entities_for_index`), embeds changed rows in batches (skip-unchanged via `text_hash`), prunes deleted. Background sweep from `ensure_embeddings`. *Deferred:* per-mutation single-row hooks.
- **Assembler** (`commands::recall_context` + `context::merge_and_trim`): cross-kind semantic recall → 1-hop graph neighbors (`db::entity_neighbors`) → recency tail → token-budgeted `ContextBundle`.
- **Wired:** planner auto-recall (pages-only, semantic-only, `RECALL_FLOOR` **0.65** — bge-small's unrelated short-text baseline is ~0.59) and `vault_ask` (tasks/events/pages/people). Cmd-K is notes-only by choice.

---

## Test suite (`.github/workflows/test.yml` runs it on push/PR)
- **Rust unit + integration** (`cargo test --lib`, ~188 tests): pure logic across scheduler/parser/habits/db/hermes/booking/model_manager/commands, plus **httpmock** integration for `llm::chat_json`, `hermes::embed_text`, and `google.rs` (via a `#[cfg(test)]` `api_base()`/`token_url()` seam). `secrets.rs` uses a `#[cfg(test)]` in-memory store seam. In-memory DB via `db::test_conn()`. **Deferred:** the full Google `sync()` orchestrator end-to-end.
- **Frontend unit + component** (`npm test` → Vitest + jsdom, ~71 tests): pure utils, the Zustand store (mocked ipc), an **IPC contract test** (`ipcContract.test.ts` parses `lib.rs` `generate_handler![]` vs `ipc.ts` `invoke<>` names), and components. Test files are colocated `*.test.ts(x)` and excluded from the app `tsconfig`.
- **Mocked-IPC E2E** (`npm run test:e2e` → Playwright): drives the real React app with a faked `window.__TAURI_INTERNALS__.invoke` (`tests/e2e/_mockBridge.ts`). **CI-only** (no browser build in the WSL sandbox).
- **Live model eval** (`tests/llm_eval.rs`, `--ignored`): parser-quality battery; needs a running `:8080`, out of CI. Baseline ~90%, judge per-category.
- **Model battery with UI projection** (`tests/model_battery.rs`, `--ignored`) — the **pre-push model gate**. Same live path as `llm_eval` + runs `reschedule_inner` and prints a readable text "screen" per case (chat reply, calendar, task/habit lists, conflicts). ~45 hard cases. Output → stdout + `target/model-battery/report.md`. Run: `cargo test --test model_battery -- --ignored --nocapture` (app open), or against Ollama via `PUSHIN_LLM_URL`/`PUSHIN_LLM_MODEL`.

---

## Known limitations / follow-ups
- **Mobile (Android builds + runs; iOS untested — needs a Mac):** plan in `~/.claude/plans/virtual-noodling-hoare.md`; recipe + gotchas in memory `mobile-cross-compile`. Full Rust core (incl. Iroh sync) cross-compiles to Android; a debug APK runs on an emulator (`gen/android`, gitignored). Desktop-safe build changes: `keyring` scoped to `cfg(not(target_os="android"))` + in-memory Android stub in `secrets.rs`; `reqwest` → rustls. Mobile UI: `lib/useIsMobile.ts` → `components/MobileShell.tsx` (bottom tabs, no sidebar), `CalendarPane` `days` prop (1=mobile, 7=desktop), quick-capture FAB. **Inference on mobile** is handled by the **mobile→desktop inference bridge** (see that section): a phone with no local `llama-server` borrows a paired desktop peer's chat + embed servers over the mesh, so AI features work whenever a desktop is reachable (data always works offline regardless). **Still:** in-process llama.cpp via FFI + a smaller default model for standalone on-device mobile inference (so mobile AI works with no desktop online) is a later follow-up.
- Google **tokens → OS keychain**; smarter **block-mirror diffing** (avoid delete+recreate churn).
- **Engine auto-download** spans macOS/Linux/Windows; Windows **CUDA** GPU build now auto-downloads (memory `gpu-inference`). Still TODO: bundle `llama-server` as a per-OS sidecar (offline installs); Linux CUDA; Vulkan/HIP.
- **Public booking page** is served by a hardened local HTTP server exposed via a user-run tunnel (ngrok/cloudflared). A managed hosted relay is a follow-up.
- No **drag-to-resize** on the calendar (only drag-to-move).
- **Test gap:** the full Google `sync()` orchestrator end-to-end. PageEditor real-editing is Playwright-only (jsdom can't drive ProseMirror).
- **Labeling system (core SHIPPED):** cross-cutting label taxonomy over tasks/events/habits/pages/projects (`0010_labels`). Built: CRUD/merge, a shared `LabelPicker`, sidebar `LabelPane`, Cmd-K label jumps, actionable scheduling (`db::resolve_task_prefs` → `scheduler::schedule_with_prefs`, a soft preference), calendar color-by-label + filter chips, AI auto-labeling (keyword → "Suggested" chips), event labeling UI. **Still TODO:** read-only "system labels"; a `#`-trigger inline label chip; scheduler batching. See memory `labeling-system-plan`.
