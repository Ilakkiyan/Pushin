# Dev log

Running, reverse-chronological record of notable changes — what changed, why, and how it was
verified. Newest first. Keep entries short; link to the deeper doc when there is one.

Conventions: one `###` entry per change-set; always note verification (tests/build). Companion docs:
[ROADMAP.md](ROADMAP.md) (vision), [CONTEXT_ENGINE_PLAN.md](CONTEXT_ENGINE_PLAN.md) (Phase 1),
[SECURITY_TEST_PLAN.md](SECURITY_TEST_PLAN.md) (booking-page audit).

---

## 2026-06-17

### Device-to-device sync — private Iroh mesh + changeset log ✅ (built, live-unverified)
Run Pushin on multiple devices, synced **without a cloud** — a private peer-to-peer mesh (Iroh QUIC,
E2E-encrypted) joined by a shared key, carrying a custom changeset log over SQLite. New `sync/` module +
migration `0015_sync`. See CLAUDE.md ▸ **Device sync** for the full design.
- **Data layer (the hard part, fully tested):** every synced table gets `uuid`/`updated_hlc`/`dirty`
  columns + change-capture **triggers** (so `db.rs` CRUD is untouched), generated from one registry
  (`sync::schema::TABLES`). FKs ship as referenced uuids and resolve to local ids on apply (polymorphic
  refs + deferred fixup for out-of-order/self-refs). Hybrid Logical Clock + **row-level LWW**; tombstones
  for deletes; per-peer watermarks. Capture suppression is **thread-local** (correct for inline triggers;
  isolates parallel tests).
- **Transport (`transport.rs` = only Iroh-touching file) + protocol (`protocol.rs`, transport-agnostic,
  tested over an in-memory duplex) + engine (`engine.rs`: accept loop + 20s periodic pull + `SyncStore`).**
  Identity/mesh key in the OS keychain; pairing by base32 invite ticket. Emits `sync-applied` → frontend
  re-`load()`s.
- **Commands + UI:** `sync_*` commands → `ipc.ts` → **Settings ▸ Devices & sync** (`DevicesSync.tsx`):
  name device, create/paste invite, peer list, relay vs LAN-only toggle, sync-now, leave.
- **⚠️ Iroh pinned to 0.90, not 1.0:** 1.0's `netwatch` forces `windows-core 0.62`, whose `wmi 0.18.4`
  won't compile against the `0.61` Tauri 2.11 uses — a real Windows build break. 0.90's windows chain is
  self-consistent. Revisit when upstream aligns.
- **Verified:** `cargo test --lib` **188** (+14: hlc, changeset convergence/LWW/tombstones, protocol
  over a real stream, state, identity), full `cargo build` ok, `tsc` clean, Vitest **71** (IPC contract
  picks up the 8 new commands). **Not** verified: the live two-machine mesh (NAT traversal/relays) — like
  Google sync, only provable on real devices.
- **Follow-ups:** field-level LWW; per-device change-seq to kill the once-per-round echo; persist full
  peer NodeAddr (today relies on n0 discovery by NodeId); managed/self-hosted relay.

## 2026-06-15

### Phase 4.3b — Meeting Companion: action-item extraction ✅ (Phase 4 complete)
The model-dependent step, made safe by confirm-chips on top of the deterministic brief.
- `extract_action_items` command (`chat_json`, strict schema: ≤10 items, ≤120 chars each — gotcha #4)
  + pure `clean_action_items` (trim/dedupe/cap), unit-tested.
- UI: in the event popover, paste meeting notes → "Extract action items" → suggested **confirm-chips**;
  clicking one creates a task (`createTask`) and removes the chip. Nothing is created without a click,
  so a model miss is just an unchecked suggestion — never a wrong task.
- Verified: `cargo test --lib` (174, +1) + `npm run build` + Vitest (71).
- **Phase 4 done** (focus tracking → adaptive scheduler → meeting companion). Execution loop wired:
  capture → plan → focus → learn → meet.

### Phase 4.3a — Meeting Companion: deterministic brief ✅
The reliable foundation (no LLM) before the model-dependent extraction step.
- `meeting.rs` `assemble` (pure, +2 tests): an event → its booked attendees (invitees matched to
  people by email, deduped; transient fallback when no person record) with relationship history
  (total meetings + last met), plus notes linked to the event. `history_summary` pure helper.
- `model::{MeetingBrief, AttendeeBrief}`; `meeting_brief` command + IPC.
- UI: a Brief section in the calendar event popover (`EventDetailModal`) — attendees + their history
  + linked notes; defensive load, renders nothing when there's nothing to show.
- Verified: `cargo test --lib` (173, +2) + `npm run build` + Vitest (71).
- Next (4.3b): LLM action-item extraction from meeting notes → **confirm-chips** → tasks (the model
  part, made safe by the confirm step on top of this deterministic core).

### Phase 4.2 — Adaptive scheduler (learned durations) ✅
Closes the focus-tracking loop: the scheduler now biases task durations by what completed tasks
ACTUALLY took vs their estimate. Deliberately conservative.
- `scheduler::estimation_factor(samples)` (pure, +tests): clamped median of `actual/estimate`, **1.0
  until ≥4 focus-tracked completed tasks** — so the pure scheduler and its tests are untouched without
  data. Clamp [0.6, 1.8] keeps it gentle.
- `db::estimation_samples` — `(estimate, actual)` for completed, focus-tracked tasks.
- Applied in `schedule_service::reschedule_inner`: not-done task estimates are rescaled *for this
  scheduling pass only* (stored estimates unchanged); the pure `scheduler::schedule*` is never altered.
- Verified: `cargo test --lib` (171, +2). No IPC/frontend change.
- Follow-up: a transparency surface ("Pushin learned you take ~1.3× your estimates") before this is
  very active — it only kicks in after real usage, so it's dormant today.

### llm_eval — battery run + new de-dup cases ✅
Re-ran the live battery after the session's work and expanded it for the new parser behavior.
- Added a **`dedup` category** (3 cases / 6 checks): "work on X from <time>" → a single timed block,
  no duplicate task (`dedup_lab_report`, `dedup_thesis`) + an over-fire guard that an unrelated task
  survives alongside a timed event (`dedup_does_not_overfire`).
- Result: **TOTAL 152/169 (90%)** — baseline held, so the session's (non-parser) work didn't regress
  the planner. `dedup` scored **5/6**; the one miss is a model labeling whiff (gotcha #1), and the
  deterministic guarantee itself is unit-tested in `parser`/`db`.
- Note: only de-dup was added to llm_eval because it's the only *model-driven* new behavior; briefing/
  focus/people/auto-labeling are deterministic and covered by `cargo test --lib`.

### Phase 4.1 — Focus Mode / time-tracking ✅
Records *actual* time per task — the actuals foundation for the adaptive scheduler (Phase 4.2).
- Migration `0014_focus_sessions` + `model::FocusSession` (`end` NULL while running; `minutes` derived).
- `db`: `start_focus` (enforces a single active session), `stop_focus`, `active_focus`,
  `focus_minutes_for_task`; 4 commands + IPC.
- UI: a per-task Play/Stop button in `TaskListPane` with a live mm:ss elapsed timer; the active
  session is restored on mount. Defensive against a missing api method (older mocks).
- Verified: `cargo test --lib` (169, +1) + `npm run build` + Vitest (71). Added `activeFocus` to the
  integration mock.
- **Phase 4 plan:** 4.1 focus tracking (done) → 4.2 adaptive scheduler (learn real durations, feeds
  estimates — touches the scheduler IP, its own slice) → 4.3 Meeting Companion (brief over People +
  action-item extraction). Scheduler untouched this slice.

### Phase 3 — Planning rituals + NL action bar ✅
ROADMAP Phase 3 (ask-your-life was already cross-entity from Phase 1/2).
- **Daily Briefing** (`briefing.rs`, pure + 3 tests): assembles today's events, due/overdue tasks, and
  scheduled focus minutes — deterministic, no LLM. `daily_briefing` command + `BriefingCard`, a slim
  dismissible banner above the calendar (renders nothing on a clear day).
- **NL action bar**: ⌘K palette gains a "Run: …" action that runs the text through the planner
  (`store.plan`) and shows a one-line outcome summary — so you can create/move/cancel from anywhere.
- Verified: `cargo test --lib` (168, +3) + `npm run build` + Vitest (71). Updated the integration
  mock (`dailyBriefing`/`suggestLabels`) + CommandPalette placeholder test; `BriefingCard` is defensive
  against a missing api method so older mocks can't crash the calendar.

### Phase 2.3 — Auto-labeling (keyword) ✅
Deterministic keyword auto-labeling, surfaced as confirm-chips in the shared `LabelPicker` (so it
covers tasks/events/pages/people/habits/projects at once).
- `db::suggest_labels_from` — existing labels whose name appears as a **whole word** in the entity's
  text (word-boundary match: "work" hits "more work", not "homework") and isn't already applied.
- `db::entity_text(kind, id)` — pulls the free text per label-kind. `suggest_labels` command.
- `LabelPicker` shows a "Suggested" row of one-tap add chips when the dropdown opens.
- Verified: `cargo test --lib` (165, +1) + `npm run build` + Vitest (71). LabelKind/Person gained
  "person". **Phase 2 complete** (People layer + auto-labeling).

### Phase 2.2 — People UI ✅
- 5 IPC commands (`list/get/create/update/delete_person`) + `ipc.ts` wrappers + `Person` type.
- `PeoplePane` (list + detail: editable name/email/notes, `LabelPicker kind="person"`, meeting
  history from bookings) + sidebar "People" nav + `App`/`View` wiring.
- Verified: `cargo test --lib` (164) + `npm run build` + Vitest (71, incl. IPC contract).

### Phase 2.1 — People layer foundation (backend) ✅
First slice of ROADMAP Phase 2 (the relationship layer / private CRM). Backend-only — no new IPC
command yet (contract tests untouched), so people are auto-captured + recalled but not yet surfaced
in a pane.
- Migration `0013_people` (`people` table, unique email index for dedupe) + `model::Person`.
- `db` people CRUD + `upsert_person_by_email` (dedupe key; backfills a blank name).
- **Booking → People:** `booking::confirm_booking` upserts a person from the invitee (best-effort) —
  the booking flow now feeds the rest of the app.
- **Context Engine:** people flow into `entity_index` (`entities_for_index` + `context::person_text`),
  and `vault_ask` recall now spans `Person` too ("who did I meet about X").
- Verified: `cargo test --lib` — **164 passed** (2 new). `EntityKind::Person` was already in place.
- Next: 2.2 People UI (commands + pane + sidebar) and 2.3 auto-labeling (post-pass + confirm chips).

### Event labeling — click-to-open detail popover ✅
Closes the gap behind the original label observation: events had no UI to add labels (only display).
Clicking a calendar event now opens a small `EventDetailModal` (title, time, `LabelPicker kind="event"`,
delete) — rendered outside the block, since the block is `overflow-hidden` and would clip a dropdown.
Habits keep their HabitsPane labeling (popover is events-only). Calendar refreshes event labels on
close so color-by-label + filters reflect edits. `npm run build` + Vitest (71) green.

## 2026-06-14

### Memory-engine status badge + parser dedup validation ✅
Two loose ends.
- **Badge:** Sidebar now shows `AI ready · Memory ✓` / `· Memory…` (store `embedReady`, set from
  `ensure_embeddings`' result — it cheap-early-returns when healthy, so it doubles as a status probe).
  No new IPC command. `npm run build` + Vitest (71) green; Sidebar test loosened to `/AI ready/`.
- **`llm_eval` validation of the task/event dedup:** ran the live battery (7B). First pass exposed a
  real over-reach — the dedup dropped a wanted task when the model emitted a *duration-only* event
  ("study, ~3h"). Fixed: dedup now fires **only when the event has an explicit user start time**
  (`plan.events` with a parseable `start_time`), so duration-only events don't drop same-named tasks.
  Added a regression unit test. Re-ran: **TOTAL 90%** (147/163), unchanged from baseline; `single-task`
  0/2 is a pre-existing model routing whiff (routes "study, ~3h" as an event, creates no task — not the
  dedup). Net: no regression.
- Verified: `cargo test --lib` + live `llm_eval --ignored`.

### Context Engine — recall tuning + task/event de-dup (live feedback) ✅
Live testing surfaced (1) irrelevant planner recall and (2) one "work on X from 12–2" message
creating BOTH a task and an event. Investigated by dumping the live DB + reproducing the math.
**Empirical finding:** the two *unrelated* junk notes in the corpus had cosine **0.587** — bge-small's
similarity floor for short text is ~0.59, so the old 0.35/0.45 thresholds were meaningless. Fixes:
- Planner auto-recall is **pages-only** (`recall_context(&[Page])`); the planner already sees events.
- `RECALL_FLOOR` 0.35 → **0.65** — must clear bge-small's ~0.59 unrelated baseline with margin.
- `db::entities_for_index` now **skips empty-body pages** (a blank daily note was being indexed on its
  date title alone → recalled for everything). Pruned from the index on the next sweep.
- `parser::store_plan` now **drops a task that fuzzy-matches an event created/updated the same turn**
  (the explicit calendar block is the intent) — kills the double-booking. Deterministic; 2 unit tests.
- Verified: `cargo test --lib` — **161 passed** (2 new). ⚠️ The parser change should still be run
  through `llm_eval` against a live server to confirm no regression.
- Caveat: recall corpus was 2 junk notes; thresholds need real-corpus tuning as the vault grows.

### Context Engine — Steps 3 & 4: assembler + wire-in ✅ (Phase 1 complete)
The shared retrieval surface every feature can now call, plus the first consumers.
- `context::ContextBundle` / `Budget` / `merge_and_trim` — pure dedupe-by-(kind,id) + budget trim.
- `db::entity_neighbors` (page↔task/event via `entity_links`, page→page via `page_links`, both ways)
  + `db::recent_entities` (recency tail).
- `commands::recall_context` — embed → `rank_items` over `entity_index` → 1-hop neighbor expansion →
  recency → budgeted bundle; gotcha-#8 lock dance.
- **Wired:** planner auto-recall → `recall_context` + `gate_recalled_context` (semantic-only, ≥0.35,
  ≤2; unscored neighbors/recency excluded → parser stays conservative, gotchas #1/#9). `vault_ask` now
  reasons over tasks/events/pages but cites **pages only** (non-page slots → 0, dropped).
- *Scoped out:* Cmd-K (`hermes_recall`) stays notes/pages-only (broadening it is a UI change), so the
  notes-only path (`rank_notes`/`notes_for_recall`) is kept, not removed.
- Verified: `cargo test --lib` — **159 passed** (5 new), no warnings, no IPC surface change.
- ⚠️ Not yet validated live: the planner auto-recall behavior needs a running chat+embed server (no
  llama-server in this WSL env). Pure parts (ranking, neighbors, gate, budget) are unit-covered; the
  end-to-end recall quality should be checked with the app open (and re-run `llm_eval`).

### Context Engine — Step 2: reindex pipeline ✅
Keeps `entity_index` current so cross-entity recall reflects the real data. Backend-only (no IPC
surface change → contract test untouched).
- `context::needs_index_work` + `IndexState` — pure skip/re-embed decision (new · text changed ·
  missing vector · model changed; no-backend → text-only tracking).
- `db::entities_for_index` (projects tasks/events/pages; pages read from `notes` since `list_pages`
  strips bodies) + `db::entity_index_meta`.
- `commands::reindex_all(db, http)` — batched async embed (32/req) + upsert + prune of deleted
  entities; gotcha-#8 lock dance; spawnable. Wired into `ensure_embeddings` via `spawn_reindex` so a
  sweep runs in the background once the embed engine is ready.
- `EntityKind` gained `Hash` (used as a map/set key).
- *Deferred:* per-mutation single-row hooks — new tasks/events/pages currently index on the next
  sweep (startup / "Start the AI"), not instantly. Add inline upserts when live freshness matters.
- Verified: `cargo test --lib` — **156 passed** (2 new). Async sweep itself needs a live embed
  server (mirrors other embed code that isn't unit-tested offline); its pure parts are covered.
- Next: Step 3 (assembler — `assemble_context` + graph-neighbor expansion + token budgeting).

### Context Engine — Step 1: schema + ranking core ✅
First slice of [CONTEXT_ENGINE_PLAN.md](CONTEXT_ENGINE_PLAN.md). Adds the cross-entity recall
substrate without touching the planner yet (protects parser stability, gotchas #1/#9).
- Migration `0012_context_index` — polymorphic `entity_index` table (mirrors `entity_labels`).
- `model::EntityKind` + `model::ContextItem` (the common recall currency).
- `hermes::rank_items` (generalized ranking); `rank_notes` refactored to delegate (tests preserved).
- `context` module (`mod context` in `lib.rs`) — deterministic `*_text` projections + stable
  `text_hash` (FNV-1a, persisted-safe unlike `DefaultHasher`).
- `db` — `upsert_entity_index` / `delete_entity_index` / `entity_index_for_recall` /
  `entity_index_hashes` CRUD.
- Verified: `cargo test --lib` — **154 passed** (5 new). No command/IPC surface change.
- Next: Step 2 (reindex pipeline: async embed + upsert + create/update hooks + startup sweep).

### Booking-page security audit + hardening
Tunnel-based public booking page reviewed against [SECURITY_TEST_PLAN.md](SECURITY_TEST_PLAN.md).
Fixed in `booking_server.rs`: unbounded request body (64 KB cap), single-thread Slowloris
(thread-per-connection + in-flight cap + whole-request deadline), booking spam / Google-sync
amplification (global rate limit → 429), `</script>` breakout XSS (JSON `js_embed` escaping).
Verified safe: off-grid bookings, double-book race, SQLi, stored XSS (React-escaped), disabled/
regenerated tokens. Accepted-risk (documented): Host/DNS-rebind, CSRF, token-in-URL, TLS-at-tunnel.
- Verified: `cargo test --lib` (149 passed), 7 new security tests.

### Roadmap + Context Engine plan
Added [ROADMAP.md](ROADMAP.md) (Context Engine keystone + 13 features as one Capture→Organize→Plan→
Execute→Reflect loop) and [CONTEXT_ENGINE_PLAN.md](CONTEXT_ENGINE_PLAN.md) (Phase 1, grounded in code).
