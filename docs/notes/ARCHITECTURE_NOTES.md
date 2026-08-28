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
- `db.rs` — all SQL. Migrations are `user_version`-gated (`0001_init` … `0017`). `0002` calendar accounts; `0008` evolves `notes` into vault pages + `page_links`; `0009` adds `daily_date`, `entity_links`, an `inbox` flag; `0010` adds `labels` + polymorphic `entity_labels`; `0011` makes booking public (slug/share_token/enabled + booking `event_id`); `0012` adds `entity_index` (Context Engine); `0013` adds `people` (CRM); `0014` adds `focus_sessions`; `0015_sync` adds `uuid`/`updated_hlc`/`dirty` + change-capture triggers to every synced table, plus `sync_tombstones`/`sync_peers`/`sync_self` (generated from `sync::schema::TABLES`); `0016` adds `notes.rel_path` (two-way file vault); `0017` adds `habits.preferred_minute` (learned habit time, sync-safe since 0015 reads columns dynamically); `0018` adds `notes.origin`; `0019` adds `ics_subscriptions`; `0020` adds the synced `google_link` (shared Google setup) — a table added *after* 0015, so it applies 0015's columns/triggers itself via `sync::schema::table_sync_sql` (`TableSpec::added_in` keeps the frozen 0015 generator from touching it). `0021` adds `tasks.missed_count`/`last_missed_on` (day-rollover audit trail). `0022` brings `ics_subscriptions` into the sync chain — same after-0015 treatment as `google_link`, plus a uuid backfill derived from the feed URL so two devices that both subscribed pre-upgrade converge on one row instead of doubling the calendar list. Page/graph/daily/inbox/entity-link/label CRUD, `resolve_task_prefs`, people CRUD, focus sessions + `estimation_samples`, `entity_index` CRUD + `entities_for_index`/`entity_neighbors`, keyword `suggest_labels` all live here. `open`/`test_conn` call `sync::register_functions` before migrating (the 0015 triggers call `sync_capturing()`).
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

## Missed-task rollover (`schedule_service::sweep_missed`, migration `0021_task_missed`)
Work whose planned time came and went, deterministically kicked to the next available slot. No model
involved — the LLM never touches this path.

- **Staleness is per-day, not per-moment.** A block is stale once it ends before **midnight of today**.
  A block you blew past at 9am is therefore *kept* until the day turns over — the calendar never
  rearranges itself under you mid-day. `reschedule_inner`'s "sticky" cutoff was moved from `now` to
  `today_start` to match, so today's already-passed blocks still hold their slot AND still count
  against their task's estimate (via `locked_min`), which is what stops the task being re-placed twice.
- **What the sweep does:** delete the stale blocks, `db::mark_task_missed` (once per local date — the
  `last_missed_on < ?` guard is what keeps the count honest across the dozens of reschedules a normal
  day fires), and let the scheduler pass that follows re-place the freed minutes from `now` forward.
- **Pinned blocks roll too, and lose the pin.** A pin means "do it at *this* time"; a day later there
  is no such time left to hold. This is the one caller allowed to delete a locked block, hence
  `db::delete_blocks` alongside `replace_unlocked_blocks` (which deliberately can't).
- **Scope is deliberately narrow.** Only *active* tasks. A missed meeting is history, not work to
  re-plan; habits have their own occurrence/streak logic that rolling a miss forward would corrupt;
  and a done/archived task keeps its past blocks as the record of when the work happened.
- **Triggering.** The sweep runs inside `reschedule_inner`, so every existing reschedule path gets it
  for free. But nothing calls reschedule when time merely *passes* — so `App.tsx` holds a day-rollover
  watcher: once on open (the app may have been shut for days), then a 60s tick comparing
  `new Date().toDateString()`, which resolves sleep/wake, timezone changes and DST correctly without
  timer arithmetic.
- **`missed_count` is surfaced**, not just stored: a "↻ N×" chip on the task row (`TaskListPane`) and
  on the Today pane's due chips. Cleared when the task is marked done, so the nag can't outlive the work.

### The passed-deadline trap (`scheduler::schedule_one`)
The bug that made rollover moot, and worth remembering: placement was capped at the task's deadline
(`place(..., latest = deadline)`). Once that deadline was **behind** `now`, the cap left a zero-width
window, `place` returned nothing, and the task got **no block at all** — overdue work silently fell off
the calendar precisely when it most needed to be seen. Now the cap is `deadline.filter(|d| *d > earliest)`:
a blown deadline can't be honoured by any placement, so it stops acting as a constraint and the task is
planned into the next free slot. The `DeadlineMiss` conflict still fires (there's a second arm for the
"it all fit, but past a deadline that had already blown" case), so it reads as late rather than lost.

---

## Google Calendar sync (`calendar/google.rs`)
- **OAuth2 + PKCE via system browser + loopback `TcpListener`** (Desktop client → no redirect URI to register). Token refresh implemented.
- **Pull** incremental via `syncToken` (full-window fallback on 410). **Push** local `source='manual'` events (insert/patch by `external_id`) + task blocks (reconciled, see below).
- **Echo/dup prevention:** only push `source='manual'`; block events are tagged `extendedProperties.private.pushinKind=block`; pull **skips** tagged events.
- **Tokens live in the OS keychain** (`secrets.rs`); `calendar_accounts` (migration 0002) keeps only non-secret metadata, with the token columns as a fallback for an unavailable keychain.
- **Requires the user's own Google OAuth Desktop client** (Client ID/secret pasted in Settings; Calendar API enabled; self added as test user). Steps in `README.md`.

### Multi-device: every paired device syncs the same calendar (`0020_google_link`)
Connecting Google once applies it to **every** paired device, and all of them talk to Google directly.
That means concurrent writers on one calendar, so two things had to become order-independent:
- **The shared link.** `google_link` is a synced, single-row table holding the account (email, calendar id) + the user's OAuth client. It projects onto a joining device via `db::adopt_google_link`, called after every sync session in `engine.rs`: it copies the client into `settings` and creates a `calendar_accounts` row with **no** access token and **no** `sync_token`, so the new device mints its own token from the shared refresh token and does its own first full pull. Deleting the link (Disconnect) propagates the teardown.
- **The refresh token** has no SQLite column. It rides the changeset as a **keychain-backed secret field** (`TableSpec::secrets`) — read from `secrets` when building a payload, written straight back to the peer's keychain on apply, and cleared there when the link's tombstone lands.
- **`events.provider`/`external_id` now sync.** They are the event's Google identity; without them a peer would see a synced event as "never pushed" and insert a **duplicate**. `account_id` (a local FK) and `etag` stay device-local.
- **`adopt_existing`** closes the remaining race: if a peer's event reached us over the mesh before its `external_id` did, an exact title+start+end match in the fetched window is adopted instead of inserted.
- **`plan_block_mirror`** replaced the old delete-every-block-event-and-recreate mirror, which two devices could not run concurrently without deleting each other's work. Blocks are keyed by their sync `uuid` (in `extendedProperties.private.pushinRef`), so the mirror is a diff: insert missing, patch changed, delete orphans — idempotent, and a raced double-insert self-heals on the next run. Both reconcilers are pure functions and unit-tested. **Built but NOT tested live** — first connect is the real test; likely first snags are missing test-user or un-enabled Calendar API (readable errors surface in Settings).

---

## Device sync — private peer-to-peer mesh (`sync/`, migration `0015_sync`)
Run Pushin on multiple devices and keep them in sync **without a cloud**: data flows device→device over a private **Iroh** QUIC mesh (E2E-encrypted), joined by a shared **mesh secret**. SQLite stays the source of truth; a **custom changeset log** reconciles state with last-writer-wins.
- **Two-layer split.** Everything above `transport.rs` is transport-agnostic and unit-tested over an in-memory duplex; `transport.rs` is the only Iroh-touching file. Protocol/changeset have full unit coverage, and `two_real_iroh_endpoints_pair_and_converge` pairs **two real Iroh endpoints in one process** over loopback (real ticket → real dial → real session), so only NAT-traversal across machines is left unprovable in tests.
- **Identity & the "key" (`identity.rs`, OS keychain via `secrets`).** Each device has a persisted Iroh node key (its NodeId is its address). A per-network **mesh secret** is the shared key. Pairing ticket = base32(NodeAddr + mesh secret), shown as an invite code; the joiner adopts the secret and does an initial sync.
- **Global ids, not local rowids.** Integer PKs collide across devices, so `0015` adds a `uuid` to every synced table; FK columns ship as the referenced row's uuid and resolve back on apply (`changeset.rs`). Polymorphic refs (`entity_links`/`entity_labels`) resolve via `entity_kind`→table. Out-of-order FKs use a deferred-fixup loop.
- **Change capture = triggers, not instrumented CRUD.** `0015` adds `AFTER INSERT/UPDATE/DELETE` triggers that stamp uuids, mark rows `dirty`, and write `sync_tombstones` — so the ~50 `db.rs` mutation functions are untouched. Triggers consult `sync_capturing()`; our own build/apply writes run inside `with_capture_suppressed`. **Suppression is thread-local.**
- **HLC + LWW (`hlc.rs`).** A Hybrid Logical Clock gives a total order robust to clock skew; higher HLC wins per row. `stamp_dirty` assigns HLCs, `changes_since(hlc)` is the delta a peer pulls, per-peer **watermarks** in `sync_peers`. **Granularity is row-level** (field-level is a follow-up).
- **What syncs:** all user content (tasks/events/projects/habits/notes+links/labels+joins/people/focus/bookings/event_types + all blocks), plus the shared `google_link` (see Google sync above). **Excluded** (device-local / re-derived): `settings`, `calendar_accounts` + the per-device Google tokens/cursor, Iroh keys, `entity_index`, and `embedding` columns (each device re-embeds; writing an embedding is wrapped in `with_capture_suppressed`).
- **Engine (`engine.rs`).** Started at boot if paired (best-effort, never blocks startup); owns the endpoint, an accept loop + a **20s periodic pull**; impls `SyncStore` (short locked ops, never across `.await`). On applying remote changes it emits a Tauri `sync-applied` event; `App.tsx` re-`load()`s. Endpoint closes on app exit.
- **Commands/UI.** `sync_status`/`sync_create_invite`/`sync_join`/`sync_now`/`sync_remove_peer`/`sync_set_device_name`/`sync_set_relay`/`sync_leave` → `ipc.ts` → **Settings ▸ Devices & sync** (`components/DevicesSync.tsx`).
- **Relay/privacy.** Default uses n0 relays for NAT traversal (encrypted QUIC only); a Settings toggle switches to LAN/direct-only.
- ⚠️ **Pairing teardown is why the session ends on a `Bye`.** The responder's last act would otherwise be a *read*, while the initiator closes the QUIC connection the instant `run_session` returns — and a QUIC close discards unacknowledged stream data. Result: the **inviting** device's session died every single time (`connection lost: closed by peer`), so it never applied the joiner's changes and never recorded the peer, while the joiner reported success. The responder now sends a closing `Bye` the initiator waits for, and `handle_incoming` awaits `conn.closed()` (10s cap) before dropping the connection so the `Bye` can't be raced off the wire.
- ⚠️ **Invites wait for the home relay.** `node_addr().initialized()` resolves as soon as *any* address exists, and local interface addresses land in milliseconds while the relay handshake takes seconds — so minting immediately produced a ticket carrying **only the LAN address**, with no relay URL and no public reflexive address. A joiner then had no path at all if the direct one was blocked (a dismissed Windows Firewall prompt suffices). `make_ticket` now waits up to 10s for the home relay when relays are enabled, falling back to direct-only rather than failing.
- ⚠️ **`ensure_engine` rebuilds when the mesh secret changes.** An engine caches the secret it bound with; joining another device's network replaces that secret, so a previously-started engine (this device had made its own network) kept presenting the OLD one and failed the peer's mesh authentication. `sync_join` is also bounded by a **45s timeout** with an actionable error — it used to be able to spin indefinitely with no feedback.
- ⚠️ **Iroh is pinned to `0.90`, not `1.0`.** iroh 1.0's `netwatch` forces `windows-core 0.62`, whose `wmi 0.18.4` won't compile against the `windows-core 0.61` Tauri 2.11 uses (a real Windows build break). iroh 0.90's chain is self-consistent. Needs rusqlite `functions` + `data-encoding`. See memory `device-sync`.
- **Known follow-ups:** scalar-HLC watermark can re-ship a foreign-authored row once (idempotent); field-level LWW; smarter block handling; persisting full NodeAddr per peer.
- ⚠️ **`events.ics_sub_id` was an unsynced FK that broke sync outright** (fixed `9ae15ce`; shipped v0.8.0, live through v0.8.1). `events` syncs but `ics_subscriptions` does not, and the column was neither declared in the events `TableSpec::fks` (so never rewritten to a uuid) nor in `skip` (so never dropped) — it went on the wire as a raw *local* rowid. With `foreign_keys = ON` (set by `db::open`), a peer holding no subscription at that id failed the apply INSERT with `FOREIGN KEY constraint failed (787)`, and that error propagates out of the batch: **the entire apply was rejected, so nothing landed on that device at all.** One .ics feed silently broke all sync to any device not sharing that rowid. The milder hazard the column also carried — `ON DELETE CASCADE`, so deleting an unrelated local feed at a colliding rowid could cascade-delete a synced event — needed the collision, and was the *lucky* case. Fix: `ics_sub_id` joins the events `skip` list, dropped on the wire rather than rewritten (the far side has no equivalent row to point at); the event still replicates, just without a feed id that would be meaningless there. Regression test: `ics_backed_event_syncs_without_dragging_its_local_feed_id_across`.
  - **Still open (product call, not a bug):** whether .ics events should replicate between devices *at all*. They're re-derivable from the feed, so a peer subscribed to the same calendar ends up holding both its own copy and the replicated one.

---

## Vault file sync — the documents, not just their rows (`sync/blobs.rs`, migration `0023_vault_file_sync`)
Vault **pages** already replicated as `notes` rows from `0015`. What did not was everything else in the vault folder — attachments, PDFs, images, any non-page file — because a changeset log carries metadata and a 40 MB PDF is not metadata. `0023` adds the missing half: the index syncs as rows, the bytes travel in their own phase of the same session.
- **Two tables, one synced.** `vault_file_index` (rel_path, hash, size) is the shared truth and joins the `TABLES` registry, so it merges, tombstones, and survives a half-finished session like any other table. `vault_file_seen` (rel_path → mtime, size, hash) is **device-local** scan bookkeeping so a rescan can skip re-hashing an unchanged 80 MB attachment; syncing it would be nonsense (mtime differs on every device by construction) and would have two devices dirtying each other's rows forever.
- **Deterministic uuids.** `vault_file_index.uuid` is derived from the path (`db::vault_file_uuid` = `'vf-' || hex(rel_path)`), not random — the same lesson as `ICS_UUID_BACKFILL`. Two already-paired devices holding the same attachment would otherwise each mint a random id, and applying the peer's row would hit `UNIQUE(rel_path)` and abort the **entire** changeset batch (the `9ae15ce` failure mode).
- **The blob phase (`protocol.rs`).** After the row exchange — never before, because the want-list is only correct once the peer's index rows have landed — each side sends a `Want` and answers the other's with a stream of chunked `Blob` frames (512 KB raw per frame, base64) terminated by `BlobsDone`. Still a fixed, deadlock-free alternation: `initiator → Hello, Pull, Push, Want, Blobs…` / `responder → Hello, Push, Pull, Blobs…, Want, Bye`.
- **Capability negotiation.** `Hello.blobs` is `#[serde(default)]` and the phase runs only when **both** sides advertise it. Without that, one un-updated device would desynchronise the choreography and break sync *entirely* rather than merely skipping files. Serde ignores unknown fields, so the old build reading the new Hello is free. A device with no vault folder configured also declines — it has nothing to send and nowhere to put what it gets.
- **The want-list is the whitelist.** A received chunk whose hash we did not ask for is dropped, and a file is written to the path **we** recorded for that hash, never the one the sender put on the wire. The blob phase is the only place a peer hands us raw bytes *and* a path; a peer that could choose the path could write anywhere the vault reaches. `write_blob` then re-verifies the SHA-256 before anything lands, so the content address fixed by ordinary LWW sync is what actually arrives.
- **Convergence rests on `vault_file_seen`.** It holds only paths *this* device has actually had on disk, which is what separates "the peer's file, not fetched yet" from "a file I deleted". Get that wrong and the index row a peer just created gets tombstoned by the receiver's next scan — the file deletes itself moments after arriving.
- **Not indexed:** page mirrors (a page syncs as a `notes` row and each device rewrites its own `.md`; indexing the file too would put two writers on one path), dot-files and dot-folders (`.obsidian/`, `.git/`, `.DS_Store` — device-local editor state), and anything over `vault::MAX_SYNC_FILE` (100 MB), which stays local rather than stalling the session behind it.
- ⚠️ **A stray `.md` can change owner mid-life.** Drop a loose markdown file in the vault and it is file-synced — until the watcher folds it into a page, at which point the `notes` row owns it and its index row is frozen at the pre-page hash. Both `wanted` and `apply_index_deletions` skip page-mirror paths for exactly this reason: without the guard, two devices ask each other for a file neither can serve (the bytes no longer hash to what the row promised) on a 20-second timer, forever.
- **When it runs.** `engine::changes_since` reindexes the folder before stamping, so a file added or deleted here ships in *this* session. The vault watcher only watches `.md`, so a newly-dropped PDF is picked up by the 20s periodic sync rather than instantly. Hashing is skipped for anything whose `(mtime, size)` is unchanged, so steady-state cost is a directory walk; the first scan of a large vault is the slow one.
- **Tested:** `sync::blobs` unit tests (index/cache/tombstone/whitelist/hash-refusal) plus end-to-end sessions in `sync::protocol` — a real attachment crossing, a multi-chunk file reassembling, both directions in one session, an empty file, a delete propagating, progress reaching its total, and both "peer predates file sync" and "no vault folder here" falling back to rows only.

---

## Sync progress → the sidebar bar (`progress.rs`, `components/SyncBar.tsx`)
One event, `sync-progress`, carries `{source, phase, label, done, total, active}` from **both** engines — the device mesh (rows, then file bytes) and Google Calendar (pull/push/mirror) — so the sidebar bar renders whichever is running without knowing which.
- **`total == 0` means indeterminate**, not zero. The Google pull cannot know its size until Google answers, so the bar sweeps a segment and prints no number. A fake percentage that jumps to 90% and parks is worse than none: it teaches the user to stop believing the bar.
- **The closing `done` is emitted on error paths too** (`sync_with` bookends the whole attempt; `sync_google` emits before the `?`). A peer that goes unreachable mid-session would otherwise leave the bar stuck at whatever fraction it reached with nothing left running to finish it. `App.tsx` adds a 30s watchdog for the case nothing can emit at all — a hard backend crash.
- Lives above the AI status tag in the sidebar footer, renders nothing when idle (the footer keeps its height), and collapses to a bare bar with the detail in the tooltip on the narrow rail.

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
- **Engine auto-download** spans macOS/Linux/Windows; Windows **CUDA** GPU build now auto-downloads (memory `gpu-inference`). Still TODO: bundle `llama-server` as a per-OS sidecar (offline installs); Linux CUDA; Vulkan/HIP.
- **Public booking page** is served by a hardened local HTTP server exposed via a user-run tunnel (ngrok/cloudflared). A managed hosted relay is a follow-up.
- No **drag-to-resize** on the calendar (only drag-to-move).
- **Rollover vs. the Google block mirror (pre-existing, slightly widened).** The mirror's window starts
  at `now - 1 day`, so when the rollover sweep deletes a block that ended *earlier* than that, the
  matching Google event is outside the window and `plan_block_mirror` never sees it to retire it — it
  lingers as a stale "Focus: X" entry. This already happened for unlocked blocks (wiped by
  `replace_unlocked_blocks` every reschedule); the sweep merely adds pinned blocks to the set. Fixing it
  means widening `time_min`, which costs a bigger pull on every sync — deliberately not done yet.
- **Test gap:** the full Google `sync()` orchestrator end-to-end. PageEditor real-editing is Playwright-only (jsdom can't drive ProseMirror).
- **Labeling system (core SHIPPED):** cross-cutting label taxonomy over tasks/events/habits/pages/projects (`0010_labels`). Built: CRUD/merge, a shared `LabelPicker`, sidebar `LabelPane`, Cmd-K label jumps, actionable scheduling (`db::resolve_task_prefs` → `scheduler::schedule_with_prefs`, a soft preference), calendar color-by-label + filter chips, AI auto-labeling (keyword → "Suggested" chips), event labeling UI. **Still TODO:** read-only "system labels"; a `#`-trigger inline label chip; scheduler batching. See memory `labeling-system-plan`.
