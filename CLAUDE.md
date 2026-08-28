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

13. **A passed deadline must stop being a constraint.** `scheduler::schedule_one` caps placement at a task's deadline — correct until the deadline is *behind* `now`, when the cap becomes a zero-width window, `place` returns nothing, and the overdue task gets **no block at all** (it vanishes from the calendar exactly when you need to see it). The cap is now `deadline.filter(|d| *d > earliest)`; `DeadlineMiss` still fires so it reads as late, not lost. Related: `schedule_service::sweep_missed` kicks work whose day is over into the next free slot (pinned blocks included — they lose the pin), and the sticky-block cutoff is **midnight today**, not `now`, so the calendar never rearranges itself mid-day. See `docs/notes/ARCHITECTURE_NOTES.md` → Missed-task rollover.

14. **Measuring the model: the eval harnesses.** `tests/llm_eval.rs` (per-category scorecard) and `tests/model_battery.rs` (UI-projected pre-push gate) run the **real** parser→store_plan(→reschedule) path against a live `:8080`. Both `#[ignore]`d + self-skip with no server, so `cargo test` stays green. **Baseline ~90% of checks; the TOTAL bounces run-to-run (gotcha #1) — judge per-category.** On WSL the test exe is a Windows binary so it reaches `:8080`; if relinking `pushin.exe` fails while the app runs, run the built exe directly. See `docs/notes/ARCHITECTURE_NOTES.md` → Test suite.

15. **Never anchor a test to `Local::now()` plus an offset.** Five tests on `main` were red or
    latently red for this alone. `plant_block(now - Duration::days(1), 60)` is not "yesterday": run it
    at 23:30 and the block *ends* at 00:30 **today**, so the end-based rollover sweep correctly leaves
    it alone and every assertion flips. A fixed-60-minute estimate against "the elapsed part of today"
    breaks for the 50 minutes after midnight. Anchor to a fixed hour on a computed date
    (`days_ago_at(now, 1, 9)`) and size fixtures from the same clock you assert against. These fail at
    the hours nobody runs the suite, which is exactly when CI does.

16. **The model flips am/pm on the START time, and the confirmation text still reads right.** The
    live 7B answers `"21:00"` to "at 9am" often enough to matter — a morning appointment booked for
    the evening, discovered by missing it. `parser::fix_flipped_meridian` takes the user's own words
    as authoritative, but only under three gates: exactly ONE explicitly `am`/`pm`-marked time in the
    message, exactly ONE timed item in the plan, and a disagreement of exactly ±12h. A merely
    *different* time is a different reading of the sentence and is left alone. Related:
    `find_worded_duration` reads "half an hour" / "an hour and a half" / "two and a half hours", which
    the model reliably answers with its 60-minute default — and it must run BEFORE the digit scan,
    since "1 hour and a half" contains a digit the scanner would stop at.

17. **Dragging a calendar block addresses the TASK, not the block.** A task the scheduler split
    around a meeting is several blocks sharing one title. Pinning just the dragged one left its
    siblings stranded as duplicate-looking events that could never be reunited. `move_task_to` pools
    the task's chunks and re-lays them from the drop point — merging into one block where the space
    allows, splitting only around real obstacles. It uses `scheduler::free_after`, NOT `free_slots`: a
    drag is an explicit instruction, so it may land outside work hours and in the sleep window; the
    only thing it will not do is overlap. Finished/archived tasks are excluded — their blocks are
    history. Habits keep the old slide-to-nearest-free behavior (one fixed-length thing, nothing to
    merge).

18. **A shared `Arc<Mutex<Connection>>` needs poison tolerance on *every* side.** `AppState::db()`
    recovers from a poisoned lock precisely so one panic can't brick the session — but
    `booking_server` held the *same* mutex and still called `.unwrap()`, so an unrelated panic
    anywhere in the app silently 500'd every public booking link with no sign inside the app. When you
    add a new holder of that Arc, use a poison-tolerant lock.

19. **`.ics` feeds are not well-formed.** `VALARM` blocks nest inside `VEVENT` and carry their own
    `SUMMARY`; read flat, the alarm's subject overwrote the event's title, so every feed with
    reminders imported mis-titled. And a `DTEND` that is not strictly after `DTSTART` (zero-length or
    reversed — both ship in the wild) produces an interval that every overlap check
    (`a.start < b.end && b.start < a.end`) reports as "never overlaps", so the event drew as a
    hairline and the scheduler planned straight through it. Both are handled in `calendar::ics`; when
    extending the parser, compose fixtures from lines (`ics(&[...])`) rather than inline literals — a
    hand-written one hid a folded-line bug in the *test data*.

20. **Vault FILES sync separately from vault PAGES, and the two must not both own a path.** Pages
    replicate as `notes` rows; everything else in the vault folder (attachments, PDFs, images)
    replicates through `sync/blobs.rs` — the index as LWW rows, the bytes in their own chunked phase
    after the row exchange (migration `0023`). Three things are load-bearing. `vault_file_seen` holds
    only paths *this* device has actually had on disk, which is the ONLY thing separating "the peer's
    file, not fetched yet" from "a file I deleted" — get it wrong and a file deletes itself moments
    after arriving. The want-list is the receiver's **whitelist**: a blob's bytes are written to the
    path *we* recorded for that hash, never the one the sender chose, and the SHA-256 is re-verified
    before anything lands. And a path that is a page mirror is skipped by both `wanted` and
    `apply_index_deletions` — a loose `.md` that the watcher later folds into a page leaves a frozen
    index row behind, and without the guard two devices ask each other every 20 seconds for a file
    neither can serve. `Hello.blobs` is `#[serde(default)]` and the phase runs only when both sides
    advertise it, so an un-updated peer skips files instead of desynchronising the choreography and
    breaking sync outright.

21. **A vault folder is a page row, so every page query has to opt it back out.** `0024` adds
    `is_folder` to `notes` rather than a folders table — the tree, `move_page`, sort order and device
    sync then apply unchanged. The cost is that a folder rides along in *everything* that reads
    pages: it must be excluded from `search_pages` (an empty container as a search hit), `title_index`
    (`[[Work]]` resolving to a folder named Work instead of the page), `page_graph` (a degree-0 dot per
    container) and `entities_for_index`. There are **seven** SELECTs feeding `row_to_page`, and two of
    them are table-aliased (`n.`) — adding a column to five of them compiles fine and fails at runtime
    with `Invalid column name`. Grep `row_to_page(r, ` for the full set. Rename is its own
    `rename_page`, not `update_page`, which rewrites body + embedding.

---

## Build / run / test
- **`npm run verify` runs everything** — Rust unit + Vitest + tsc/build + Playwright — and prints one
  table, with per-suite logs and a report in `target/verify/`. `verify:fast` is the edit loop (Rust +
  Vitest only); `verify:live` adds the three model evals, skipping them automatically when no
  llama-server is on :8080. Deterministic tiers gate; **live eval scores are reported, never gating** —
  they bounce run-to-run (gotcha #1) and a bouncing score must not turn into a red build. Prefer it
  over running the suites by hand: it also captures output the `rtk` wrapper would otherwise compress
  away, which once silently ate a full 14-minute `llm_eval` run.
- Environment specifics (WSL uses the Windows `cargo.exe`; no live llama-server there): memory `build-test-env`.
- **Backend:** `cargo build`/`cargo test --lib` (**430** tests) with `--manifest-path src-tauri/Cargo.toml`. The Bash cwd resets to project root between calls — use absolute paths or `--manifest-path`.
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

## Current status (released **v0.8.3**; `release.yml` builds installers for all platforms)
Full changelog + per-feature status in `docs/notes/ARCHITECTURE_NOTES.md` and `docs/notes/DEVLOG.md`. Headline:
- **Calendar core:** on-device planning pipeline, auto-scheduler, week/month calendar with drag-to-move/pin + re-plan, conversational create/update/remove, **missed-task rollover** (a task whose day passed unfinished is kicked to the next free slot; pinned blocks included), tasks, habits (draggable + learned time, `0017`), first-run model+engine auto-download, two-way Google sync (leaf fns httpmock-tested; first live connect unverified) — now **shared across paired devices** (`0020_google_link`: one link + a keychain-borne refresh token replicate over the mesh, so connecting once connects them all).
- **Second brain:** sidebar + Cmd-K palette, Notion vault + `[[wikilinks]]` + backlinks + graph, daily notes, entity links, semantic recall, chat→memory chips, ask-your-vault RAG, quick capture → Inbox, Markdown import, two-way markdown file vault (0016, live-unverified), **Drive-style file browser + folders** (0024: `is_folder` pages, virtual Journal folder, sidebar "Open" switcher).
- **Context Engine + execution loop:** `entity_index` recall spine feeding planner auto-recall + `vault_ask`; People/CRM; keyword auto-labeling; Daily Briefing + Cmd-K "Run" bar; Focus timer + adaptive scheduler; Meeting Companion; hardened public booking page.
- **Device sync (pairing proven on loopback, cross-machine still unverified):** private Iroh mesh + changeset log (`0015`), pairing-by-invite, LWW. `two_real_iroh_endpoints_pair_and_converge` runs a real ticket→dial→session between two endpoints in one process. Iroh runs **1.x**: the previous `0.90` pin meant relays accepted the connection but would not route, so cross-network pairing could never work — see ARCHITECTURE_NOTES ▸ Device sync before touching the iroh version.
- **Shell polish:** frameless `TitleBar`, opening animation, in-app auto-update (v0.5.0+, signed).
- **Tested:** Rust `cargo test --lib` (**430**) + httpmock, Vitest (**271**, 30 files) + IPC/bridge contract tests, Playwright E2E (**13**), live `llm_eval` **261/276** across 108 cases — the original tier still scores 100% per category; the **hard tier** (`date-math`, `duration-words`, `pronoun-ref`, `ambiguity`, `restraint-hard`, `noisy-input`, `adversarial`, `overload`) is where the remaining gap lives and is the tuning signal. `model_battery` 57–58/58 (one adversarial case bounces), `real_world_eval` 10/12 (run them with `npm run verify:live`, which skips them when no llama-server is on :8080).
- **Repo:** GitHub `Ilakkiyan/Pushin`; `main` default; releases are version tags.

## Working style with this user
Wants fast iteration and **honest assessment** — when something flaky is the model's limitation vs. a code
bug, say which **and prove it** (test against the live `:8080` server, don't just compile). Verify changes
*actually work*, not just that they build. Recommend the 7B when reliability matters.
