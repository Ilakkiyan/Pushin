# Pushin — project guide for Claude Code

Pushin is a **local-AI, Motion-style calendar** evolving into a **"second brain"**. You describe your
tasks/events in plain language; a **small LLM running 100% on-device** turns that into structured tasks
+ fixed events, and a **deterministic Rust auto-scheduler** packs the tasks into your calendar around
fixed events. It also has **two-way Google Calendar sync**, and a **Notion-style document vault with
Obsidian-style `[[wikilinks]]` + a connection graph** (the Hermes memory layer, grown up).

The whole app lives behind a **collapsible left sidebar** (`Sidebar.tsx`) — not top tabs — with a
**Cmd/Ctrl-K command palette** (`CommandPalette.tsx`) to jump to any page or view.

This file is the knowledge handoff. Read it fully before changing things — much of it is non-obvious
and was learned the hard way.

---

## Locked product decisions (don't relitigate without the user)
- **Desktop-first** (Tauri 2), stack chosen so the React frontend + Rust core can later extend to PWA/mobile.
- **On-device only** inference (no cloud fallback). Privacy + offline are the point.
- **LLM parses, deterministic solver schedules.** Tiny models are good at extraction, bad at constraint-solving — keep the scheduler in Rust.
- **Google sync = full mirror** (events **and** task blocks) to the user's **primary** calendar.

## Stack
- **Shell:** Tauri 2 — Rust backend (`src-tauri/`), web frontend (`src/`).
- **Frontend:** React 19 + TypeScript + Vite + Tailwind v4; state via **Zustand** (`src/state/store.ts`);
  SQLite is source of truth. Editor: `@blocknote/*`; graph: `react-force-graph-2d`. Tests: Vitest + Playwright.
- **Inference:** llama.cpp **`llama-server`** run as a child process, OpenAI-compatible API at `http://127.0.0.1:8080`, using **`response_format: json_schema`** for constrained JSON.
- **Models** (`model_manager::MODELS`): Qwen2.5 **3B** ("lite", default download, ~2GB), **7B** ("recommended", ~4.7GB), **14B** ("most powerful", ~9GB). 4-bit GGUF from bartowski on HuggingFace, auto-downloaded on first run. Default `Settings.model_id` stays the lite 3B (fast first run); the card flags 7B as recommended.
- **DB:** SQLite via `rusqlite` (Rust) + `@tauri-apps/plugin-sql` (frontend). Lives at `~/Library/Application Support/com.pushin.app/pushin.db`.
- Target: **macOS arm64**, **Linux x64/arm64**, **Windows x64/arm64**. The engine
  auto-download/unpack/spawn is cross-platform (`model_manager.rs`); macOS is the most-tested.

## Architecture
```
React UI (sidebar shell | chat | week/month calendar | tasks | habits | vault editor | graph | ⌘K palette)
  │  Tauri invoke (commands.rs)
  ▼
Rust core
  ├─ model_manager : first-run download of model + llama.cpp engine; spawn/kill llama-server
  ├─ llm           : HTTP client → llama-server; json_schema requests; retry; anti-runaway sampling
  ├─ parser        : NL → events/tasks/edits; day-word→date resolution; dedupe; merge
  ├─ scheduler     : the IP — dependency DAG + EDF/priority greedy + chunking + conflicts; parse_dt/fmt_dt
  ├─ calendar/google : OAuth(PKCE loopback) + token refresh + two-way sync
  ├─ booking       : availability via scheduler free-slots (booking-page seam)
  ├─ hermes        : memory layer ("second brain") — embeddings + cosine/keyword recall; backs the vault
  ├─ context       : Context Engine — cross-entity recall over `entity_index` (projections, reindex, assembler)
  ├─ briefing      : deterministic Daily Briefing (today's events + due tasks + focus minutes)
  ├─ meeting       : Meeting Companion — deterministic pre-meeting brief + (LLM) action-item extraction
  └─ db            : projects, tasks, task_deps, events, blocks, settings, calendar_accounts, event_types, bookings, notes(=vault pages), page_links, labels, entity_labels, entity_index, people, focus_sessions
       │ spawns child process              │ OAuth + HTTPS
       ▼                                    ▼
  llama-server (GGUF, Metal)          Google Calendar API v3 (optional)
```

## File map
**Rust (`src-tauri/src/`)**
- `lib.rs` — Tauri setup, command registration, app-exit kills the llama-server child.
- `commands.rs` — the IPC surface. **Never hold the DB `Mutex<Connection>` across an `.await`** (async commands must stay `Send`); use short scoped lock blocks.
- `model.rs` — `Settings`, `Event`, `Block`, `Task`, `GoogleAccount`, plus `Person`, `FocusSession`,
  `EntityKind`/`ContextItem` (Context Engine), `Briefing`, `MeetingBrief`/`AttendeeBrief`, etc.
- `db.rs` — all SQL. Migrations are `user_version`-gated (`0001_init` … `0014_focus_sessions`). `0008`
  evolves `notes` into vault pages + adds `page_links`; `0009` adds `daily_date`, `entity_links`
  (page ↔ task/event), an `inbox` flag; `0010` adds `labels` + polymorphic `entity_labels`; `0011`
  makes booking public (slug/share_token/enabled + booking `event_id`); `0012` adds **`entity_index`**
  (Context Engine cross-entity recall); `0013` adds **`people`** (CRM); `0014` adds **`focus_sessions`**
  (time-tracking). Page/graph/daily/inbox/entity-link/label CRUD, `resolve_task_prefs`, people CRUD,
  focus sessions + `estimation_samples`, `entity_index` CRUD + `entities_for_index`/`entity_neighbors`,
  and keyword `suggest_labels` all live here.
- `model_manager.rs` — model + engine auto-download, `llama-server` spawn/health, `MODELS` list.
  Cross-platform: picks the **CPU** llama.cpp release asset per OS/arch (asset substrings are
  extension-less since llama.cpp churns extensions — Linux moved `.zip`→`.tar.gz`). macOS/Linux
  `.tar.gz` unpack via system `tar`; Windows `.zip` via the in-process `zip` crate. Extracts to a
  staging dir then **flattens binary + co-located libs into `bin/`**. Spawn sets `LD_LIBRARY_PATH`
  (Linux) and `CREATE_NO_WINDOW` (Windows).
- `llm.rs` — `chat_json(messages, schema)`; sampling tuned to stop runaway (see Gotchas).
- `parser.rs` — **the trickiest file.** NL→plan, day-word→date, dedupe, edit-merge. See Gotchas.
- `scheduler.rs` — auto-scheduler + `parse_dt`/`fmt_dt` + **`estimation_factor`** (adaptive: learn real
  durations from focus actuals; soft, fallback 1.0; applied in `schedule_service::reschedule_inner`). Has unit tests.
- `calendar/google.rs` — Google two-way sync (OAuth/API/sync engine).
- `calendar/mod.rs` — just declares `google` (SQLite is the source of truth; the old `CalendarProvider`/LocalProvider indirection was removed).
- `booking.rs` — booking-page availability (reuses scheduler free-slots) + invitee→`people` upsert on confirm.
- `booking_server.rs` — local `TcpListener` HTTP server for the public booking page (slots/book/cancel),
  hardened (body cap, thread-per-connection + in-flight cap, global rate limit, XSS escaping — see `SECURITY_TEST_PLAN.md`).
- `context.rs` — **Context Engine**: deterministic `*_text` projections, FNV-1a `text_hash`,
  `needs_index_work`, `merge_and_trim`, `ContextBundle`. (Ranking is `hermes::rank_items`; `reindex_all`
  + `recall_context` live in `commands.rs`.) See the Context Engine section.
- `briefing.rs` — pure Daily Briefing assembler (today's events + due/overdue tasks + scheduled focus minutes).
- `meeting.rs` — pure Meeting Companion brief: event → booked attendees (→ `people`) + history + linked notes.

**Frontend (`src/`)**
- `lib/ipc.ts` — typed wrappers over every Tauri command + shared types (incl. `Page`, `PageGraph`).
- `lib/blocks.ts` — BlockNote helpers: blocks→plaintext (recall index), block JSON↔page, `[[link]]` title extraction.
- `lib/editorSchema.tsx` — the BlockNote schema + the custom `pageLink` inline-content (the `[[wikilink]]` chip).
- `state/store.ts` — Zustand store; `mutate()` runs a change → stores conflicts → refresh → `maybeSync()` (debounced Google sync). Also holds `pages`/`currentPageId` + page CRUD and `view`/`sidebarCollapsed`.
- `panes/` — `ChatPane` (+ "Remember this?" memory chips), `CalendarPane` (24h grid, drag-to-move/pin,
  day-header daily-note, **`BriefingCard`** banner, **event-detail popover** = labels + Meeting Companion
  brief + action-item confirm-chips), `MonthPane`, `TaskListPane` (task "Notes" action + **Focus timer**
  Play/Stop), `ProjectsPane`, `HabitsPane`, `SettingsPane`, `BookingPane`, **`PeoplePane`** (CRM),
  **`VaultPane`** (page editor), **`GraphPane`**, **`InboxPane`** (quick-capture triage), `LabelPane`.
- `components/` — **`Sidebar`** (left nav, **AI + Memory** status, vault tree, Inbox badge, Today's note,
  **People** nav), **`VaultTree`** (page tree, drag-to-reparent, import button), **`PageEditor`** (BlockNote
  + autosave + `[[` picker + `/` templates + backlinks/linked-entities/unlinked-mentions), **`CommandPalette`**
  (Cmd-K: semantic search + ask-your-vault + **"Run" NL action bar** → planner), **`LabelPicker`** (chips +
  **auto-label suggestions**), **`BriefingCard`**, **`QuickCapture`** (Cmd/Ctrl+Shift+N), **`TitleBar`**
  (frameless window controls), `InferenceSetup`, `ConflictBanner`, `OnboardingModal`.
- `lib/import.ts` — vault importer (folder picker → headless BlockNote markdown→blocks → pages).
  (The old top-tab `TopBar` and standalone `HermesPane` were removed when the sidebar + vault landed.)
- **Editor stack:** `@blocknote/{core,react,mantine}` (Notion-style block editor; CSS imported in
  `main.tsx`) + `react-force-graph-2d` (canvas force layout for the graph). Both are client-side/offline.

---

## ⚠️ Gotchas & hard-won lessons (read before editing parser/llm/scheduler)

1. **Small models are EXTREMELY prompt-sensitive and unreliable at reasoning.** The 3B is the
   reliability ceiling for multi-op/edit routing and relative dates. Tiny prompt wording changes flip
   results run-to-run. Recommend the **7B** to users who want consistency. Keep prompts **short** with
   one event + one task example — longer prompts *degrade* accuracy.

2. **Dates: the model emits a `day` WORD (today/tomorrow/weekday) + `startTime`/`endTime`; RUST
   computes the actual date** (`parser::resolve_day`/`resolve_event`). Letting the model output absolute
   dates failed badly — it was consistently **+7 days off**. Never trust model date math.

3. **PM-less end times.** The model often drops PM on the *end* of a range ("12-2" → end "02:00";
   "6pm-10pm" → end "10:00"). `parser::compute_end` does up to **two +12h bumps** to recover both
   dropped-PM (same day) and overnight ranges ("8pm→8am" = 12h). `parse_hm` also accepts
   "2pm"/"2:00 PM"/"14:00:00".

4. **Anti-runaway.** The model used to loop a `notes` string until it overran `max_tokens` and
   truncated the JSON. Fixes: **dropped `notes` from the schema**, added `maxLength`/`maxItems` caps
   (become grammar bounds in llama.cpp), `repeat_penalty`+`frequency_penalty`, and a retry in `chat_json`.

5. **Edit-routing safety net.** The 3B routes edits ("move the sleepover") as fresh *creates*. So
   `store_plan` reconciles: a "create" whose **title matches an existing event becomes a merge/update**,
   not a duplicate. Consequence: **one event per title** (can't have two identically-titled events) — a
   deliberate trade to kill the duplicate bug the user hit. `merge_event` keeps unspecified fields.

6. **Conversational CRUD.** Schema has `events` (create), `updateEvents` (fuzzy-title match + changed
   fields), `removeEvents` (fuzzy-title delete). The model SEES the current calendar in the system
   prompt. Order in `store_plan`: remove → update → create-with-reconcile.

7. **`parse_dt` tolerates** `Z`/offset/seconds-less ISO. All times stored as naive-local
   `YYYY-MM-DDTHH:MM:SS`.

8. **DB locking:** `Mutex<Connection>` guard must be dropped before any `.await`. The Google sync does
   a careful lock→read→unlock→http→lock→write dance for this reason.

9. **Conversation history** is passed to the planner for follow-ups ("this friday at 7pm" needs the
   prior turn for context, else it hallucinates a "Meeting").

10. **Task-field recovery (same philosophy as event fields).** The `deadline` field's format is
    never shown to the model, so its deadlines are usually dropped — and it defaults task lengths to
    60 min. `parser::backfill_task_fields` recovers both from the user's text: a single deadline
    ("due/by/before <day|M/D>", "in 3 weeks", or — in a task-only message — a lone day word like
    "exam **friday**") is applied to every task missing one; a lone task with an explicit length
    ("study **about 3 hours**") gets that estimate. Both are guarded (single/unambiguous, no
    competing event) so nothing is mis-assigned. See `find_deadline_dates`.

11. **Measuring the model: the eval harness.** `src-tauri/tests/llm_eval.rs` runs a battery of
    prompts (single/multi task, events, edits, removes, habits, dates/spans, mixed, conversational,
    restraint) through the **real** `parser::plan` → `store_plan` path against a live `:8080` server
    and prints a per-category scorecard. It's `#[ignore]`d + self-skips when no server is up, so
    `cargo test` stays green. Run it after prompt/guardrail changes to see if accuracy moved:
    `cargo test --test llm_eval -- --ignored --nocapture` (with Pushin open). This is the feedback
    loop — tune the prompt or add a deterministic track, then re-run. **Baseline: the 3B scores
    ~80–85% of checks, and the TOTAL bounces run-to-run (gotcha #1) — judge per-category, not the
    total.** Deterministic, unit-tested tracks (dates, multi-event, multi-task) stay green; the noisy
    categories (edit, habit) are model whiffs where it returns an empty plan or asks for a start time
    — recommend the 7B there. Tracks this harness drove: positional range assignment for multi-event
    messages (`find_time_ranges`), explicit-date trip collapse (create + self-updates → one all-day
    span), and a phantom-duplicate guard in `store_plan` (skip a create that fuzzy-matches an event
    just updated/removed — kills the "Dentist (original)" twin).
    - On WSL: the harness exe is a **Windows** binary, so it reaches the app's `:8080` even though
      `curl` from WSL can't. `cargo test --test llm_eval` fails to relink the locked `pushin.exe`
      while the app runs, **but the test exe is still built** — run it directly:
      `./target/debug/deps/llm_eval-*.exe --ignored --nocapture`. Has a `dedup` category (see #12).

12. **Task/event de-dup.** "I'll work on X from 12–2" makes the 3B emit BOTH a task and a fixed event
    (double-booking). `store_plan` drops a task that fuzzy-matches an event created/updated the same
    turn — but **only when that event has an explicit user start time** (a real block), so a
    duration-only "study ~3h" task survives. Gated on `plan.events[].start_time`; unit-tested in
    `parser`/`db` + the `dedup` llm_eval category.

## Google Calendar sync (`calendar/google.rs`)
- **OAuth2 + PKCE via system browser + loopback `TcpListener`** (Desktop client → no redirect URI to
  register). Token refresh implemented.
- **Pull** incremental via `syncToken` (full-window fallback on 410). **Push** local `source='manual'`
  events (insert/patch by `external_id`) + **task blocks** (full mirror).
- **Echo/dup prevention:** only push `source='manual'`; block events are tagged
  `extendedProperties.private.pushinKind=block`; pull **skips** tagged events; blocks are
  **delete+recreate each sync** (correct but churny — smarter diffing is a TODO).
- **Tokens stored in SQLite** (`calendar_accounts`, migration 0002) — **moving to OS keychain is a TODO.**
- **Requires the user's own Google OAuth Desktop client** (Client ID/secret pasted in Settings; Calendar
  API enabled; self added as test user). Steps are in `README.md`. **This was built but NOT tested
  live** (no credentials in dev) — first connect is the real test; likely first snags are missing
  test-user or un-enabled Calendar API (readable errors surface in Settings).

---

## The vault — Notion-style documents + Obsidian-style links/graph (`hermes.rs`, `db.rs` pages, `notes` table)
The "second brain" direction. **v2 (built): the flat Hermes notes grew into full vault pages.** The
`notes` table is kept (preserves embeddings) but extended by **migration `0008_pages.sql`** with
`title`, `icon`, `parent_id` (the page tree), `content_json` (BlockNote block array), `sort_order`,
`archived`, plus a **`page_links`** table (one row per `[[wikilink]]`). `content` stays the **derived
plaintext** that backs recall/search. Frontend type = `Page`; Rust = `model::Page`.
- **Pages API:** `db.rs` (`list_pages`/`get_page`/`insert_page`/`update_page`/`move_page`/
  `set_page_links`/`page_backlinks`/`search_pages`/`page_graph`, `derive_title`) + matching
  `commands.rs` (`list_pages`, `get_page`, `create_page`, `update_page`, `delete_page`, `move_page`,
  `page_backlinks`, `search_pages`, `page_graph`). Unit-tested in `db.rs` `mod tests` (title
  derivation, link/ghost resolution, self-link skip, delete cascade). Legacy notes (NULL
  title/content_json) open as a plain paragraph doc with a title derived from the first line.
- **Wikilinks (`[[`):** the editor's `pageLink` inline content carries `{pageId, title}`. On save the
  frontend extracts link titles and `update_page` rebuilds `page_links` (`set_page_links` resolves
  each title→page id; unresolved = a "ghost" with NULL `target_id`). `page_graph`/`page_backlinks`
  **re-resolve ghosts by title at read time**, so a link to a not-yet-created page lights up the
  moment that page exists — no rewrite needed.
- **Editor save loop:** `PageEditor` debounces (~600ms) and flushes on unmount; sends `content`
  (plaintext, re-embedded best-effort), `content_json`, and the link titles. Embedding reuses the
  Hermes `embed_best_effort` lock dance (gotcha #8).
- **Embeddings/recall (unchanged at the data layer):** little-endian f32 BLOBs in `embedding` (NULL
  until indexed); `hermes.rs` `cosine`/`keyword_score`/codec are pure + tested. `hermes_recall` still
  ranks the same rows (now pages). **The standalone Hermes capture/recall UI is gone**; keyword search
  is in the Cmd-K palette. (Semantic Cmd-K + auto-recall into the planner are **now built** — and
  generalized beyond pages to all entity kinds; see the **Context Engine** section.)
- **Embeddings = all-in-one, zero setup:** Pushin runs a **second `llama-server` in `--embeddings`
  mode** on `EMBED_PORT` (8181), serving a tiny auto-downloaded model (`EMBED_MODEL` =
  bge-small-en-v1.5 Q8, ~37 MB, 384-dim). `ensure_embeddings` (idempotent, best-effort) downloads it
  + spawns the server; it's triggered from `store.load()` and after "Start the AI". Second child
  lives in `AppState.embed_server`, killed on exit alongside the chat one.
  Hermes embeds via `model_manager::embed_base_url()` (NOT `llm_base_url`); `Settings.embed_model` is
  just the (cosmetic) request name, empty = semantic off.
- **Recall = graceful degradation:** `hermes_recall` embeds the query and ranks indexed notes by
  **cosine**; if embeddings are unavailable/none indexed it falls back to **keyword** overlap. The
  result carries `mode: "semantic" | "keyword"` so the UI shows which ran. Notes are always usable.
- **Async + DB lock:** `hermes_add_note`/`hermes_recall` are async (HTTP). Read settings in a scoped
  lock, drop it, `await` the embed, then re-lock to write (gotcha #8). Embedding is **best-effort** —
  a note always saves even if embedding fails (stored unindexed).
- **Pure + tested:** `cosine`, `keyword_score`, and the f32↔BLOB codec are pure (`cargo test --lib
  hermes`). The embed HTTP client mirrors `llm.rs` (can't be unit-tested offline).
- **UI:** the vault — `Sidebar` page tree (`VaultTree`, drag-to-reparent) → `VaultPane`/`PageEditor`
  (block editor + `[[` link picker + `/` slash templates + "Linked references" backlinks + "Linked
  tasks & events" + "Unlinked mentions"), `GraphPane` (connection graph), and `InboxPane` (quick
  capture). Embedding model is set in Settings → On-device AI.
- **Calendar ↔ vault bridge (built):** **Daily Notes** (`get_or_create_daily`; a page per date with
  `daily_date`, opened from the calendar day-header / "Today's note" / Journal sidebar section) and
  **entity links** (`entity_links` table; a task/event ↔ its notes page, via `openEntityNote` + the
  editor's "Linked tasks & events" panel).
- **AI over the vault (built):** `hermes::recall` (the recall ranking, refactored out of the
  `hermes_recall` command so it's shared) powers — **auto-recall** (top notes injected into the planner
  system prompt by `plan_tasks`, semantic-only + score-gated to protect parser reliability; surfaced as
  "📌 Recalled" in chat), **chat→memory** (`parser::extract_memories` proposes durable facts as a
  confirm chip), **semantic Cmd-K** (`CommandPalette` prefers recall over `search_pages`), and
  **ask-your-vault** (`vault_ask` — local RAG: recall → `chat_json` answer with page citations).
- **Frictionless layer (built):** **quick capture** (`Cmd/Ctrl+Shift+N` → `QuickCapture` → `capture_note`
  → Inbox), Inbox triage (Plan with AI / Keep as note / Delete), **Markdown/Obsidian import**
  (`read_markdown_dir` walks a folder via `tauri-plugin-dialog`; `lib/import.ts` converts each file with
  a headless BlockNote editor, `[[links]]` → `set_page_links`), and **editor templates** (custom `/`
  slash items).
- **Next steps (not built):** per-page icons; global (OS-level) quick-capture hotkey; Notion-export
  importer; progressive-disclosure onboarding. (Re-indexing pre-embed-server pages is now handled by the
  Context Engine reindex sweep.)

---

## The Context Engine — cross-entity recall (`context.rs`, `db.rs` `entity_index`, migration 0012)
The shared retrieval spine: every feature pulls relevant context through one path so the on-device LLM
always sees the right slice of the user's data. Full plan: `CONTEXT_ENGINE_PLAN.md`; build log: `DEVLOG.md`.
- **`entity_index`** (0012): one polymorphic row per entity (`entity_kind` ∈ task/event/page/person/goal)
  with projected `text`, a stable FNV-1a `text_hash`, and an LE-f32 `embedding` (NULL until indexed).
  Mirrors `entity_labels`/`entity_links`.
- **Ranking generalized:** `hermes::rank_items` over `ContextItem` (semantic cosine, keyword fallback);
  `rank_notes` is now a thin adapter.
- **Reindex** (`commands::reindex_all`, best-effort, gotcha-#8 lock dance): projects all entities
  (`db::entities_for_index`), embeds changed rows in batches (skip-unchanged via `text_hash`), prunes
  deleted ones. Background sweep from `ensure_embeddings` (startup). *Deferred:* per-mutation single-row
  hooks — new entities index on the next sweep.
- **Assembler** (`commands::recall_context` + `context::merge_and_trim`): cross-kind semantic recall →
  1-hop graph neighbors (`db::entity_neighbors`) → recency tail → token-budgeted `ContextBundle`.
- **Wired:** planner auto-recall (pages-only, semantic-only, `RECALL_FLOOR` **0.65** — bge-small's
  *unrelated* short-text baseline is ~0.59, so lower floors are noise) and `vault_ask` (spans tasks/
  events/pages/people). Cmd-K is still notes-only by choice.

## Test suite (layered; `.github/workflows/test.yml` runs it on push/PR)
- **Rust unit + integration** (`cargo test --lib`, 174 tests): pure logic across `scheduler`/`parser`/
  `habits`/`db`/`hermes`/`booking`/`model_manager`/`commands`, plus **httpmock** integration for
  `llm::chat_json` (retry/error), `hermes::embed_text`, and `google.rs` (PKCE + the Calendar leaf
  fns — token refresh, incremental pull + **410 fallback**, push verbs — via a `#[cfg(test)]`
  `api_base()`/`token_url()` override seam). `secrets.rs` uses a `#[cfg(test)]` in-memory store seam
  (`test_store`) for roundtrip tests. In-memory DB via `db::test_conn()`. `httpmock`+`tempfile` dev-deps.
  **Deferred:** the full `sync()` orchestrator end-to-end (needs a seeded account/token in DB+keychain).
- **Frontend unit + component** (`npm test` → Vitest + Testing-Library + jsdom, 71 tests):
  `vitest.config.ts` + `vitest.setup.ts` (mocks `@tauri-apps/api/window` + `plugin-dialog`). Covers
  pure utils (`time`/`blocks`/`import`), the Zustand store (mocked ipc), an **IPC contract test**
  (`ipcContract.test.ts` — parses `lib.rs` `generate_handler![]` vs `ipc.ts` `invoke<>` names so a
  renamed/removed command fails CI), and components (`TitleBar`, `QuickCapture`, `Sidebar`,
  `CommandPalette`, `InboxPane`). Test files are colocated `*.test.ts(x)` and **excluded from the
  app `tsconfig`** so `npm run build` doesn't compile them.
- **Mocked-IPC E2E** (`npm run test:e2e` → Playwright): drives the real React app on the Vite dev
  server with a faked `window.__TAURI_INTERNALS__.invoke` (`tests/e2e/_mockBridge.ts`, an in-memory
  fake backend) — boot/nav, vault create, quick-capture→Inbox, Cmd-K + ask-your-vault. **CI-only**:
  Playwright has no browser build for this WSL sandbox's OS, so it runs on `ubuntu-latest`.
- **Live model eval** (`tests/llm_eval.rs`, `--ignored`): the parser-quality battery; needs a running
  `:8080`, stays out of CI (manual gate). Baseline ~90% of checks, judge per-category (gotcha #1).

## Build / run / test (macOS, IMPORTANT specifics)
- **`rustc`/`cargo` are NOT on the default PATH.** Prefix with `export PATH="$HOME/.cargo/bin:$PATH"`.
- **The Bash cwd resets to the project root between calls.** Use absolute paths or `--manifest-path`.
- Build backend: `cargo build --manifest-path /Users/lucky/Documents/GitHub/Pushin/src-tauri/Cargo.toml`
- Test backend (**174 tests** at last count): `cargo test --manifest-path .../src-tauri/Cargo.toml`
  (on WSL use the Windows `cargo.exe`; see memory `build-test-env`).
- Build/typecheck frontend: `npm run build` (`tsc && vite build`).
- Run the app (dev): `npm run tauri dev` (watches Rust → rebuilds + relaunches; Vite HMR for frontend).
- Regenerate app icons from a PNG: `npm run tauri icon <path-to-1024.png>`.
- **Test the model directly** without the GUI: a `llama-server` runs on `:8080` when the app is up — POST to `/v1/chat/completions` with the `json_schema` body to validate parser behavior (this is how parser changes were verified — invaluable, do it).

## Local data (outside the repo, gitignored)
`~/Library/Application Support/com.pushin.app/`
- `models/*.gguf` — downloaded models
- `bin/llama-server` (+ dylibs) — auto-downloaded llama.cpp engine
- `pushin.db` — SQLite (tasks, events, blocks, settings, Google tokens)

## Current status (released **v0.3.1**; tagged on GitHub, `release.yml` builds installers)
- **Working — calendar core:** on-device planning pipeline, auto-scheduler, full-day (00–24) week +
  month calendar with drag-to-move/pin + re-plan, conversational create/update/remove of events, task
  list, habits, settings, first-run model+engine auto-download, two-way Google sync
  (compiles + leaf fns httpmock-tested; first live connect still unverified).
- **Working — second brain (v0.2.x):** collapsible left **sidebar** shell + **Cmd-K palette**;
  Notion-style **vault**, Obsidian-style **`[[wikilinks]]` + backlinks + connection graph**, **daily
  notes**, task/event↔page **entity links**, on-device **semantic recall**, chat→memory chips,
  ask-your-vault RAG, **quick capture → Inbox**, **Markdown/Obsidian import**, editor templates.
- **Working — Context Engine + execution loop (v0.3.x):** cross-entity recall spine (`entity_index`,
  reindex sweep, `recall_context` assembler) feeding planner auto-recall + `vault_ask`; **People/CRM**
  (auto-created from bookings) + `PeoplePane`; **keyword auto-labeling** (confirm-chips); **Daily
  Briefing** banner + **Cmd-K "Run" NL action bar**; **Focus timer** + **adaptive scheduler** (learned
  durations); **Meeting Companion** (deterministic brief + confirm-chip action-item extraction); **public
  booking page** via a hardened local HTTP server + tunnel (see `SECURITY_TEST_PLAN.md`). Several
  model-dependent bits (action-item quality, planner recall on a real corpus) are **unverified live**.
- **Working — shell polish:** custom **frameless `TitleBar`** (own min/max/close) that **auto-hides
  when maximized/fullscreen**, revealing on a top-edge hover (F11 toggles fullscreen).
- **Tested:** layered suite — Rust `cargo test --lib` (174) + httpmock integration, Vitest (71) +
  IPC/bridge contract tests, Playwright mocked-IPC E2E (CI), live `llm_eval` battery (~90%, manual).
  CI: `.github/workflows/test.yml`. See **Test suite** above.
- **Branding:** pushpin 📌 — sidebar brand, favicon, dock icon (`src-tauri/icons/`).
- **Repo:** on GitHub (`Ilakkiyan/Pushin`), `main` is the default; releases are version tags (`v0.2.0`…).

## Known limitations / follow-ups
- **Mobile:** the spawn-a-server approach won't work on iOS (no subprocess). Mobile needs **in-process
  inference** (link llama.cpp via FFI, or MLC/MediaPipe) + a smaller default model (1.5B/0.5B). Memory
  (~2GB for 3B) is the real wall on phones.
- Google **tokens → OS keychain**; smarter **block-mirror diffing** (avoid delete+recreate churn).
- **Engine auto-download now spans macOS/Linux/Windows** (CPU build). Still TODO: optionally
  **bundle `llama-server`** as a per-OS sidecar (offline installs), and offer GPU builds
  (CUDA/Vulkan/Metal) instead of the GPU-agnostic CPU asset. Live-verify on Linux/Windows.
- **Public booking page** is served by a hardened local HTTP server (`booking_server.rs`) exposed via a
  user-run tunnel (ngrok/cloudflared). A managed hosted relay (no manual tunnel) is still a follow-up.
- No **drag-to-resize** on the calendar yet (only drag-to-move).
- **Test gap:** the full Google `sync()` orchestrator end-to-end (the leaf fns are httpmock-tested);
  needs a seeded account/token in DB+keychain. PageEditor real-editing is Playwright-only (jsdom can't
  drive ProseMirror).
- **Labeling system (core SHIPPED):** a flat+grouped, user-defined, **cross-cutting** label taxonomy
  over tasks/events/habits/pages/projects (`0010_labels`: `labels` + polymorphic `entity_labels`,
  mirroring `entity_links`). Built: label CRUD/merge, a shared **`LabelPicker`** attached to tasks/
  habits/projects/pages, a sidebar **Labels** section + **`LabelPane`** (cross-cutting filtered view +
  inline manager with scheduling prefs), Cmd-K label jumps, and **actionable scheduling** — labels'
  time-of-day window + min-chunk bias the planner (`db::resolve_task_prefs` → `scheduler::schedule_with_prefs`;
  a *soft* preference that always falls back). **Now also shipped:** calendar color-by-label + filter
  chips (`CalendarLabelControls`); **AI auto-labeling** (keyword word-boundary post-pass → "Suggested"
  confirm-chips in `LabelPicker`, all kinds); **event labeling UI** (the calendar event-detail popover);
  `person` is now a label kind. **Still TODO:** read-only "system labels" for structural kinds; a
  `#`-trigger inline label chip in the editor; batching in the scheduler. See memory `labeling-system-plan`.

## Working style with this user
Wants fast iteration and **honest assessment** — when something flaky is the model's limitation vs. a
code bug, say which (and prove it: test against the live `:8080` server, don't just compile). Verify
that changes *actually work*, not just that they build. Recommend the 7B when reliability matters.
