# Pushin — project guide for Claude Code

Pushin is a **local-AI, Motion-style calendar** evolving into a **"second brain"**. You describe your
tasks/events in plain language; a **small LLM running 100% on-device** turns that into structured tasks
+ fixed events, and a **deterministic Rust auto-scheduler** packs the tasks around fixed events. It also
has **two-way Google Calendar sync**, a **Notion-style vault with Obsidian `[[wikilinks]]` + a connection
graph**, and **device-to-device sync** over a private Iroh mesh. The whole app lives behind a
**collapsible left sidebar** (`Sidebar.tsx`) with a **Cmd/Ctrl-K command palette** (`CommandPalette.tsx`).

> **Deep detail lives in `docs/notes/ARCHITECTURE_NOTES.md`** (file map + every subsystem: Google sync, device sync,
> updater, vault, Context Engine, test suite, known limitations) and `docs/developer-guide/*`. This file
> keeps only the essentials + the hard-won gotchas. Read the deep-dive for the subsystem you're touching.

---

## Locked product decisions (don't relitigate without the user)
- **Desktop-first** (Tauri 2), stack chosen so the React frontend + Rust core can later extend to PWA/mobile.
- **On-device only** inference (no cloud fallback). Privacy + offline are the point.
- **LLM parses, deterministic solver schedules.** Tiny models are good at extraction, bad at constraint-solving — keep the scheduler in Rust.
- **Google sync = full mirror** (events **and** task blocks) to the user's **primary** calendar.

## Stack
- **Shell:** Tauri 2 — Rust backend (`src-tauri/`), web frontend (`src/`).
- **Frontend:** React 19 + TypeScript + Vite + Tailwind v4; state via **Zustand** (`src/state/store.ts`); SQLite is source of truth. Editor: `@blocknote/*`; graph: `react-force-graph-2d`. Tests: Vitest + Playwright.
- **Inference:** llama.cpp **`llama-server`** as a child process, OpenAI-compatible API at `http://127.0.0.1:8080`, using **`response_format: json_schema`** for constrained JSON. A second `llama-server` in `--embeddings` mode on `:8181` serves bge-small for recall.
- **Models** (`model_manager::MODELS`): Qwen2.5 **3B** (default, ~2GB), **7B** (recommended, ~4.7GB), **14B** (~9GB). 4-bit GGUF from bartowski, auto-downloaded on first run. Engine + (NVIDIA) CUDA build also auto-download (memory `gpu-inference`).
- **DB:** SQLite via `rusqlite` (Rust) + `@tauri-apps/plugin-sql` (frontend), at the app-data dir.
- Target: **macOS arm64**, **Linux x64/arm64**, **Windows x64/arm64**. Engine auto-download/spawn is cross-platform; macOS is most-tested.

## Architecture
```
React UI (sidebar shell | chat | week/month calendar | tasks | habits | vault editor | graph | ⌘K palette)
  │  Tauri invoke (commands.rs)
  ▼
Rust core
  ├─ model_manager : first-run download of model + llama.cpp engine; spawn/kill llama-server
  ├─ llm           : HTTP client → llama-server; json_schema requests; retry; anti-runaway sampling
  ├─ parser        : NL → events/tasks/edits; day-word→date resolution; dedupe; merge; recovery guards
  ├─ scheduler     : dependency DAG + EDF/priority greedy + chunking + conflicts; parse_dt/fmt_dt
  ├─ calendar/google : OAuth(PKCE loopback) + token refresh + two-way sync
  ├─ booking       : availability via scheduler free-slots + booking_server (hardened local HTTP)
  ├─ hermes        : memory layer — embeddings + cosine/keyword recall; backs the vault
  ├─ context       : Context Engine — cross-entity recall over `entity_index`
  ├─ briefing / meeting : deterministic Daily Briefing + Meeting Companion brief
  ├─ sync          : device-to-device sync — Iroh P2P mesh + custom changeset log (LWW over SQLite)
  │                  + mobile→desktop inference bridge (a modelless phone borrows a peer's llama-server)
  └─ db            : projects, tasks, events, blocks, settings, notes(=vault pages), page_links,
                     labels, entity_index/links, people, focus_sessions, bookings, …
       │ spawns child process              │ OAuth + HTTPS
       ▼                                    ▼
  llama-server (GGUF)                  Google Calendar API v3 (optional)
```
See `docs/notes/ARCHITECTURE_NOTES.md` for the per-file map and each subsystem's engineering detail.

---

## ⚠️ Gotchas & hard-won lessons (read before editing parser/llm/scheduler/db)

1. **Small models are EXTREMELY prompt-sensitive and unreliable at reasoning.** The 3B is the reliability ceiling for multi-op/edit routing and relative dates. Tiny prompt wording changes flip results run-to-run. Recommend the **7B** for consistency. Keep prompts **short** with one event + one task example — longer prompts *degrade* accuracy.

2. **Dates: the model emits a `day` WORD (today/tomorrow/weekday) + `startTime`/`endTime`; RUST computes the actual date** (`parser::resolve_day`/`resolve_event`). Letting the model output absolute dates failed badly — consistently **+7 days off**. Never trust model date math.

3. **PM-less end times.** The model often drops PM on the *end* of a range ("12-2" → end "02:00"). `parser::compute_end` does up to **two +12h bumps** to recover dropped-PM (same day) and overnight ranges. `parse_hm` accepts "2pm"/"2:00 PM"/"14:00:00".

4. **Anti-runaway.** The model used to loop a `notes` string until it overran `max_tokens` and truncated the JSON. Fixes: **dropped `notes` from the schema**, added `maxLength`/`maxItems` caps (grammar bounds in llama.cpp), `repeat_penalty`+`frequency_penalty`, and a retry in `chat_json`.

5. **Edit-routing safety net.** The 3B routes edits ("move the sleepover") as fresh *creates*. `store_plan` reconciles: a "create" whose **title matches an existing event becomes a merge/update**, not a duplicate. Consequence: **one event per title** (deliberate trade to kill the duplicate bug). `merge_event` keeps unspecified fields.

6. **Conversational CRUD.** Schema has `events` (create), `updateEvents` (fuzzy-title match + changed fields), `removeEvents` (fuzzy-title delete). The model SEES the current calendar in the system prompt. Order in `store_plan`: remove → update → create-with-reconcile.

7. **`parse_dt` tolerates** `Z`/offset/seconds-less ISO. All times stored as naive-local `YYYY-MM-DDTHH:MM:SS`.

8. **DB locking:** the `Mutex<Connection>` guard must be dropped before any `.await` (async commands must stay `Send`). Google sync + all embed paths do a careful lock→read→unlock→http→lock→write dance for this reason.

9. **Conversation history** is passed to the planner for follow-ups ("this friday at 7pm" needs the prior turn for context, else it hallucinates a "Meeting").

10. **Task-field recovery.** The `deadline` format is never shown to the model, so deadlines are usually dropped (and it defaults lengths to 60 min). `parser::backfill_task_fields` recovers both from the user's text, guarded (single/unambiguous, no competing event). See `find_deadline_dates`.

11. **Task/event de-dup.** "I'll work on X from 12–2" makes the 3B emit BOTH a task and a fixed event. `store_plan` drops a task that fuzzy-matches an event created/updated the same turn — but **only when that event has an explicit user start time**, so a duration-only "study ~3h" task survives. Unit-tested + a `dedup` llm_eval category.

12. **Fabrication guards (`parser.rs::apply_recovery`, all deterministic + unit-tested).** Even the 7B fabricates. Three tightly-gated recovery passes make the AI **ask instead of guess** — judge each against the live model, they're conservative on purpose:
    - `promote_timed_work_to_block` — "work on X **from A–B**" → ONE fixed block (promote a lone floating task, drop the redundant sibling task matched by a shared significant word, clear spurious event edits).
    - `collapse_unrequested_decomposition` — a lone deliverable with NO list/breakdown cue must not explode into a fabricated multi-subtask project: collapse to one task (keep the deadline), drop fabricated deadline-**marker** events (`is_deadline_marker`), and **ask a follow-up**. Any comma / " and " / breakdown keyword keeps the decomposition.
    - `drop_unrequested_prep_tasks` — mentioning an activity shouldn't spawn a "Prepare for X" task unless the user said prepare/prep/get-ready.
    Follow-up clarifications survive `filter_clarifications`.

13. **Measuring the model: the eval harnesses.** `tests/llm_eval.rs` (per-category scorecard) and `tests/model_battery.rs` (UI-projected pre-push gate) run the **real** parser→store_plan(→reschedule) path against a live `:8080`. Both `#[ignore]`d + self-skip with no server, so `cargo test` stays green. **Baseline ~90% of checks; the TOTAL bounces run-to-run (gotcha #1) — judge per-category.** On WSL the test exe is a Windows binary so it reaches `:8080`; if relinking `pushin.exe` fails while the app runs, run the built exe directly. See `docs/notes/ARCHITECTURE_NOTES.md` → Test suite.

---

## Build / run / test
- Environment specifics (WSL uses the Windows `cargo.exe`; no live llama-server there): memory `build-test-env`.
- **Backend:** `cargo build`/`cargo test --lib` (~188 tests) with `--manifest-path src-tauri/Cargo.toml`. The Bash cwd resets to project root between calls — use absolute paths or `--manifest-path`.
- **Frontend:** `npm run build` (`tsc && vite build`); tests `npm test` (Vitest), `npm run test:e2e` (Playwright).
  "CI-only" means CI *runs* it, NOT that you may skip it: **run it locally before tagging a release, and
  after ANY change to nav structure, pane copy, or `_mockBridge.ts`.** It is the only suite that renders the
  real app, so it catches what the others can't — v0.8.0 shipped with it red (nav walk + a `getByText`
  strict-mode break), and it was the suite that caught PeoplePane unmounting the whole app on a null payload.
- **Run (dev):** `npm run tauri dev` (watches Rust + Vite HMR). App icons: `npm run tauri icon <1024.png>`.
- **Test the model directly** without the GUI: a `llama-server` runs on `:8080` when the app is up — POST to `/v1/chat/completions` with the `json_schema` body. This is how parser changes are verified — **do it, don't just compile.**
- **`[profile.release]`** is tuned for install size (`strip` + `lto` + `codegen-units=1` + `opt-level="s"`) → slower *release* builds only; dev/debug unaffected. `panic` is intentionally unwind (booking server is thread-per-connection).
- **Booking tests** use a **7-day** horizon so they're date-independent (a 2-day horizon fails on weekends).

## Local data (outside the repo, gitignored) — app-data dir `com.pushin.app/`
`models/*.gguf` (downloaded models) · `bin/llama-server` (+libs, auto-downloaded engine) · `pushin.db` (SQLite).

## Current status (released **v0.8.1**; `release.yml` builds installers for all platforms)
Full changelog + per-feature status in `docs/notes/ARCHITECTURE_NOTES.md` and `docs/notes/DEVLOG.md`. Headline:
- **Calendar core:** on-device planning pipeline, auto-scheduler, week/month calendar with drag-to-move/pin + re-plan, conversational create/update/remove, tasks, habits (draggable + learned time, `0017`), first-run model+engine auto-download, two-way Google sync (leaf fns httpmock-tested; first live connect unverified) — now **shared across paired devices** (`0020_google_link`: one link + a keychain-borne refresh token replicate over the mesh, so connecting once connects them all).
- **Second brain:** sidebar + Cmd-K palette, Notion vault + `[[wikilinks]]` + backlinks + graph, daily notes, entity links, semantic recall, chat→memory chips, ask-your-vault RAG, quick capture → Inbox, Markdown import, two-way markdown file vault (0016, live-unverified).
- **Context Engine + execution loop:** `entity_index` recall spine feeding planner auto-recall + `vault_ask`; People/CRM; keyword auto-labeling; Daily Briefing + Cmd-K "Run" bar; Focus timer + adaptive scheduler; Meeting Companion; hardened public booking page.
- **Device sync (pairing proven on loopback, cross-machine still unverified):** private Iroh mesh + changeset log (`0015`), pairing-by-invite, LWW. `two_real_iroh_endpoints_pair_and_converge` runs a real ticket→dial→session between two endpoints in one process; only NAT traversal is untested. Iroh pinned **0.90**.
- **Shell polish:** frameless `TitleBar`, opening animation, in-app auto-update (v0.5.0+, signed).
- **Tested:** Rust `cargo test --lib` (**304**) + httpmock, Vitest (**80**, 19 files) + IPC/bridge contract tests, Playwright E2E (**4**), live `llm_eval`/`model_battery` (~90%, manual).
- **Repo:** GitHub `Ilakkiyan/Pushin`; `main` default; releases are version tags.

## Working style with this user
Wants fast iteration and **honest assessment** — when something flaky is the model's limitation vs. a code
bug, say which **and prove it** (test against the live `:8080` server, don't just compile). Verify changes
*actually work*, not just that they build. Recommend the 7B when reliability matters.
