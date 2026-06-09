# Pushin — project guide for Claude Code

Pushin is a **local-AI, Motion-style calendar**. You describe your tasks/events in plain language; a
**small LLM running 100% on-device** turns that into structured tasks + fixed events, and a
**deterministic Rust auto-scheduler** packs the tasks into your calendar around fixed events. It also
has **two-way Google Calendar sync**.

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
- **Frontend:** React 18 + TypeScript + Vite + Tailwind; state via **Zustand** (`src/state/store.ts`); SQLite is source of truth.
- **Inference:** llama.cpp **`llama-server`** run as a child process, OpenAI-compatible API at `http://127.0.0.1:8080`, using **`response_format: json_schema`** for constrained JSON.
- **Models** (`model_manager::MODELS`): Qwen2.5 **3B** ("lite", default download, ~2GB), **7B** ("recommended", ~4.7GB), **14B** ("most powerful", ~9GB). 4-bit GGUF from bartowski on HuggingFace, auto-downloaded on first run. Default `Settings.model_id` stays the lite 3B (fast first run); the card flags 7B as recommended.
- **DB:** SQLite via `rusqlite` (Rust) + `@tauri-apps/plugin-sql` (frontend). Lives at `~/Library/Application Support/com.pushin.app/pushin.db`.
- Target: **macOS arm64**, **Linux x64/arm64**, **Windows x64/arm64**. The engine
  auto-download/unpack/spawn is cross-platform (`model_manager.rs`); macOS is the most-tested.

## Architecture
```
React UI (chat | full-day week calendar | task list | settings)
  │  Tauri invoke (commands.rs)
  ▼
Rust core
  ├─ model_manager : first-run download of model + llama.cpp engine; spawn/kill llama-server
  ├─ llm           : HTTP client → llama-server; json_schema requests; retry; anti-runaway sampling
  ├─ parser        : NL → events/tasks/edits; day-word→date resolution; dedupe; merge
  ├─ scheduler     : the IP — dependency DAG + EDF/priority greedy + chunking + conflicts; parse_dt/fmt_dt
  ├─ calendar/google : OAuth(PKCE loopback) + token refresh + two-way sync
  ├─ booking       : availability via scheduler free-slots (booking-page seam)
  ├─ hermes        : memory layer ("second brain") — note embeddings + cosine/keyword recall
  └─ db            : projects, tasks, task_deps, events, blocks, settings, calendar_accounts, event_types, bookings, notes
       │ spawns child process              │ OAuth + HTTPS
       ▼                                    ▼
  llama-server (GGUF, Metal)          Google Calendar API v3 (optional)
```

## File map
**Rust (`src-tauri/src/`)**
- `lib.rs` — Tauri setup, command registration, app-exit kills the llama-server child.
- `commands.rs` — the IPC surface. **Never hold the DB `Mutex<Connection>` across an `.await`** (async commands must stay `Send`); use short scoped lock blocks.
- `model.rs` — `Settings`, `Event`, `Block`, `Task`, `GoogleAccount`, etc.
- `db.rs` — all SQL. Migrations are `user_version`-gated (`0001_init`, `0002_google`).
- `model_manager.rs` — model + engine auto-download, `llama-server` spawn/health, `MODELS` list.
  Cross-platform: picks the **CPU** llama.cpp release asset per OS/arch (asset substrings are
  extension-less since llama.cpp churns extensions — Linux moved `.zip`→`.tar.gz`). macOS/Linux
  `.tar.gz` unpack via system `tar`; Windows `.zip` via the in-process `zip` crate. Extracts to a
  staging dir then **flattens binary + co-located libs into `bin/`**. Spawn sets `LD_LIBRARY_PATH`
  (Linux) and `CREATE_NO_WINDOW` (Windows).
- `llm.rs` — `chat_json(messages, schema)`; sampling tuned to stop runaway (see Gotchas).
- `parser.rs` — **the trickiest file.** NL→plan, day-word→date, dedupe, edit-merge. See Gotchas.
- `scheduler.rs` — auto-scheduler + `parse_dt`/`fmt_dt` time helpers. Has unit tests.
- `calendar/google.rs` — Google two-way sync (OAuth/API/sync engine).
- `calendar/mod.rs`, `calendar/local.rs` — vestigial `CalendarProvider` trait + LocalProvider seam (only `sync_calendar` uses Local; real Google path is free functions).
- `booking.rs` — booking-page availability (reuses scheduler free-slots).

**Frontend (`src/`)**
- `lib/ipc.ts` — typed wrappers over every Tauri command + shared types.
- `state/store.ts` — Zustand store; `mutate()` runs a change → stores conflicts → refresh → `maybeSync()` (debounced Google sync).
- `panes/` — `ChatPane`, `CalendarPane` (full 24h grid, drag-to-move/pin), `TaskListPane`, `SettingsPane`, `BookingPane`.
- `components/` — `TopBar`, `InferenceSetup` (first-run model download/start), `ConflictBanner`.

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
      `./target/debug/deps/llm_eval-*.exe --ignored --nocapture`.

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

## Hermes — the memory layer / "second brain" (`hermes.rs`, `notes` table)
The user's chosen direction: grow Pushin into a second brain. **v1 (built) = notes + recall.** Design
follows the locked decisions (on-device, deterministic core):
- **Storage:** `notes` table (migration `0006_notes.sql`); embeddings are little-endian f32 BLOBs in
  the `embedding` column, NULL until indexed. `Note` never serializes the vector to the frontend
  (just an `indexed` flag, plus a `score` on recall results).
- **Embeddings = all-in-one, zero setup:** Pushin runs a **second `llama-server` in `--embeddings`
  mode** on `EMBED_PORT` (8181), serving a tiny auto-downloaded model (`EMBED_MODEL` =
  bge-small-en-v1.5 Q8, ~37 MB, 384-dim). `ensure_embeddings` (idempotent, best-effort) downloads it
  + spawns the server; it's triggered from `store.load()`, after "Start the AI", and on opening the
  Hermes pane. Second child lives in `AppState.embed_server`, killed on exit alongside the chat one.
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
- **UI:** `HermesPane` (TopBar "Hermes" 🧠) — capture, recall search (shows mode + % match), notes
  list with an indexed dot. Embedding model is set in Settings → On-device AI.
- **Next steps (not built):** auto-recall relevant notes into the planner's context; extract durable
  facts/preferences from chats; semantic search across tasks/events; re-index notes created before
  the embed server was up (they stay keyword-only until then).

---

## Build / run / test (macOS, IMPORTANT specifics)
- **`rustc`/`cargo` are NOT on the default PATH.** Prefix with `export PATH="$HOME/.cargo/bin:$PATH"`.
- **The Bash cwd resets to the project root between calls.** Use absolute paths or `--manifest-path`.
- Build backend: `cargo build --manifest-path /Users/lucky/Documents/GitHub/Pushin/src-tauri/Cargo.toml`
- Test backend (**15 tests** at last count): `cargo test --manifest-path .../src-tauri/Cargo.toml`
- Build/typecheck frontend: `npm run build` (`tsc && vite build`).
- Run the app (dev): `npm run tauri dev` (watches Rust → rebuilds + relaunches; Vite HMR for frontend).
- Regenerate app icons from a PNG: `npm run tauri icon <path-to-1024.png>`.
- **Test the model directly** without the GUI: a `llama-server` runs on `:8080` when the app is up — POST to `/v1/chat/completions` with the `json_schema` body to validate parser behavior (this is how parser changes were verified — invaluable, do it).

## Local data (outside the repo, gitignored)
`~/Library/Application Support/com.pushin.app/`
- `models/*.gguf` — downloaded models
- `bin/llama-server` (+ dylibs) — auto-downloaded llama.cpp engine
- `pushin.db` — SQLite (tasks, events, blocks, settings, Google tokens)

## Current status
- **Working:** on-device planning pipeline, auto-scheduler, full-day (00–24) week calendar with
  drag-to-move/pin + re-plan, conversational create/update/remove of events, task list, settings,
  first-run model+engine auto-download, two-way Google sync (compiles; not live-tested).
- **Branding:** pushpin 📌 — top bar, favicon, and dock icon (`src-tauri/icons/`, generated via a Swift
  emoji-render script → `tauri icon`).
- **Git:** the repo was about to be initialized when `gh` was found **not installed** — `git init` +
  GitHub push are still pending. (Install `gh` or add a remote manually, then commit & push. `.gitignore`
  already excludes node_modules/dist/target/models/binaries.)

## Known limitations / follow-ups
- **Mobile:** the spawn-a-server approach won't work on iOS (no subprocess). Mobile needs **in-process
  inference** (link llama.cpp via FFI, or MLC/MediaPipe) + a smaller default model (1.5B/0.5B). Memory
  (~2GB for 3B) is the real wall on phones.
- Google **tokens → OS keychain**; smarter **block-mirror diffing** (avoid delete+recreate churn).
- **Engine auto-download now spans macOS/Linux/Windows** (CPU build). Still TODO: optionally
  **bundle `llama-server`** as a per-OS sidecar (offline installs), and offer GPU builds
  (CUDA/Vulkan/Metal) instead of the GPU-agnostic CPU asset. Live-verify on Linux/Windows.
- **Public booking page** needs a hosted relay (the in-app booking is a local mockup).
- No **drag-to-resize** on the calendar yet (only drag-to-move).

## Working style with this user
Wants fast iteration and **honest assessment** — when something flaky is the model's limitation vs. a
code bug, say which (and prove it: test against the live `:8080` server, don't just compile). Verify
that changes *actually work*, not just that they build. Recommend the 7B when reliability matters.
