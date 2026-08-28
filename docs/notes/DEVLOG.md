# Dev log

Running, reverse-chronological record of notable changes — what changed, why, and how it was
verified. Newest first. Keep entries short; link to the deeper doc when there is one.

Conventions: one `###` entry per change-set; always note verification (tests/build). Companion docs:
[ROADMAP.md](ROADMAP.md) (vision), [CONTEXT_ENGINE_PLAN.md](CONTEXT_ENGINE_PLAN.md) (Phase 1),
[SECURITY_TEST_PLAN.md](SECURITY_TEST_PLAN.md) (booking-page audit).

---

## 2026-08-27

### v0.8.4 — the vault becomes a place, and files follow the notes ✅
Two changesets, built in parallel sessions against one working tree: the vault gets a Drive-style
browser with real folders, and device sync learns to carry the vault's *files* and not just its rows.

**The vault browser + folders (migration `0024_page_folders`)**
- **A folder is a `notes` row with `is_folder = 1`**, not a table of its own. The page tree,
  `move_page`, `sort_order` and 0015's device-sync triggers therefore all applied unchanged. The
  price is subtractive and easy to get wrong: a folder rides along in *everything* that reads pages,
  so it has to be explicitly excluded from `search_pages`, `title_index` (or `[[Work]]` resolves to a
  folder named Work instead of the page), `page_graph`, and `entities_for_index`. See CLAUDE.md
  gotcha 21 — seven SELECTs feed `row_to_page` and two are table-aliased, which is how a version that
  compiled cleanly failed at runtime with `Invalid column name: is_folder`.
- **`create_folder` / `rename_page` are their own commands.** `update_page` rewrites the body and
  re-embeds it, so renaming through it would blank a folder and force a full document round-trip on
  a page.
- **The Journal is a virtual folder** (`JOURNAL_ID = -1`), not a row. Daily notes are keyed by
  `daily_date` and created parentless by `get_or_create_daily`; giving them a physical parent would
  have meant teaching `daily_note`, the markdown file mirror and the graph about it. It is always
  present at the root — a folder that appears only once you have used the thing it holds is one
  nobody discovers — so the root is never an empty grid and the first-run nudge sits *alongside* the
  listing rather than replacing it.
- **Two entry points, two questions.** The sidebar's Vault button and `g v` land on `files` (the
  browser, rooted) — "where is it?"; `vault` is the editor — "what was I writing?". Opening a folder
  browses, opening a document hands off.
- **`openPageIds`** backs the sidebar's "Open" switcher: session-only, most-recent-last, fed by one
  `openState` helper that every route into the editor passes through, so a new entry point cannot
  leave the switcher lying about what is open.
- One React trap worth recording: the rename field was first written as a component defined inside
  the pane's render. That is a new component *type* every render, so React remounted it and the input
  lost focus after the first keystroke. It is a plain render function now, and the test types a whole
  word rather than one character precisely to catch a regression.

**Vault file sync (migration `0023_vault_file_sync`)**
- The vault's files replicate alongside its rows: the index syncs as ordinary rows, the bytes move in
  their own capability-negotiated phase, so a v0.8.4 device paired with a v0.8.3 one syncs pages
  exactly as before and skips the file step. Files over `MAX_SYNC_FILE` (100 MB) stay local.
- A Diagnostics panel under Settings ▸ Devices surfaces what sync actually did on this device.
- **Status: live-unverified across two physical machines**, the same footing as cross-device row
  sync. Exercised end to end in tests (attachments crossing, multi-chunk reassembly, both
  directions, deletes propagating, old-peer and no-vault-folder fallbacks).

**Verification**
`npm run verify` — Rust **471**, Vitest **371**, tsc + production build clean, Playwright **17**.
Up from 430 / 271 / 13 in v0.8.3. The browser was also driven by hand through the real app (mocked
IPC) and screenshotted in both grid and list layouts before release.

The vault's own coverage was then taken from "proves it works" to "proves it cannot break": the
seven `row_to_page` queries walked in one test (gotcha 21's failure mode), folder exclusion from the
recall index, the pre-0024 upgrade path, and on the frontend the whole interaction surface — drag
refusals, delete confirmation wording, rename abandonment, and the switcher dropping a page deleted
from under it. Two facts turned out to be different from what the implementation commit claimed and
are pinned as comments rather than quietly corrected: `list_pages` does not filter inbox captures
(the frontend does), and `search_pages` matches bodies as well as titles.

---

### v0.8.3 — a split task that comes back together, and seven quiet failures ✅
A correctness pass across every surface, driven by building the tests that were missing. The suite
grew from 329/109/10 to **430 Rust / 271 Vitest / 13 Playwright**, and the model eval from 72 cases
to **108**. Everything below was found by a test, not by reading.

**Found and fixed**
- **Four tests were already red on `main`, and a fifth was latently red.** All five anchored fixtures
  to `Local::now()` plus an offset, which stops meaning what it says at particular hours — a block
  planted "yesterday" at 23:30 *ends* today, and a 60-minute estimate outruns the elapsed day for the
  50 minutes after midnight. See CLAUDE.md gotcha 15.
- **`.ics`: `VALARM` clobbered event titles.** Nested sub-component properties were read flat, so any
  feed whose events carry reminders — i.e. most of them — imported titled after the alarm. Also a
  `DTEND` not strictly after `DTSTART` (zero-length and reversed both ship in the wild) made an
  interval that every overlap check reads as "never overlaps": the event drew as a hairline and the
  scheduler planned straight through it. `calendar::ics` tests 7 → 28.
- **The booking server was bricked by a poisoned mutex.** `AppState::db()` recovers from poison
  precisely so one panic can't kill the session; `booking_server` held the *same* `Arc<Mutex<_>>` and
  still called `.unwrap()`, so an unrelated panic anywhere silently 500'd every public booking link
  with no sign inside the app. Tests 11 → 23.
- **The planner router could never fire for a habit.** `has_scheduling_cue` needs a WHEN and a DO, but
  "every day"/"daily" sat only in the DO list — so "I'll go for a run every day", the most common way
  anyone states a habit, never tripped the override. Recurrence words are WHEN now, and stay out of DO
  so a bare "the bus comes every day" still reads as chat.
- **`slug()` could name files Windows refuses to create.** A page called `CON` or `NUL` produced a
  filename that cannot exist, and an 80-char cut could leave a trailing space — which Windows strips,
  so the recorded `rel_path` no longer matched the file and the page lost its mapping.
- **Two panes fired their loaders with no `.catch`.** `HabitsPane` and `InboxPane` were the only two
  in the app doing this; a rejected load left an unhandled rejection and a blank panel.
- **`g l` (Labels) worked but was missing from the ⌘K help.** Pinned by a test that walks every letter
  and fails on any binding the help list doesn't document, and vice versa.
- **The E2E mock bridge formatted mutated times as UTC** (`toISOString`) while the app stores naive
  local — invisible in a UTC CI container, wrong on every developer machine.

**Dragging a split task now merges it back** (`schedule_service::move_task_to`). A task the scheduler
split around a meeting is several blocks sharing one title, and dragging one pinned only that half,
stranding the rest as a duplicate-looking event. A drag now addresses the **task**: its chunks are
pooled and re-laid from the drop point, merging into one block where there is room and splitting only
around what is genuinely in the way. Free time for a drag comes from `scheduler::free_after`, not
`free_slots` — a hand drag is an explicit instruction, so it may land in the evening or the sleep
window; the one thing it will not do is overlap. See CLAUDE.md gotcha 17.

**Two deterministic parser recoveries**, both from live-model evidence rather than guesswork:
- `fix_flipped_meridian` — the 7B answers `"21:00"` to "at 9am" often enough to matter, and the
  confirmation text still reads correctly. Gated to a single marked time, a single timed item, and an
  exact ±12h disagreement.
- `find_worded_duration` — "half an hour", "an hour and a half", "two and a half hours". The model
  answers all of these with its 60-minute default.

**The benchmark got harder, on purpose.** `llm_eval` went from 72 to 108 cases with a hard tier aimed
at daily load rather than tidy demo sentences: `date-math`, `duration-words`, `pronoun-ref`,
`ambiguity`, `overload`, `noisy-input` (typos, run-ons, voice-transcript filler), `adversarial`
(self-correction mid-sentence, negation, an injection attempt in the message body), `odd-time-hard`,
`deadline-hard`, and `restraint-hard` (venting, hypotheticals, past-tense reports, someone else's
plans — all of which must create **nothing**). Every original category still scores 100%; the total is
**261/276 (95%)** and the whole of the remaining gap is in the new tier, which is the point. Weakest:
`date-math` and `ambiguity` at 50–60%. `PUSHIN_EVAL_DEBUG=1` now dumps the **raw model plan** beside
the resolved calendar, which is what separates "the model said the wrong thing" from "Rust mangled a
correct answer" — the two failures need opposite fixes.

**Coverage built where there was none.** `model.rs` 0 → 13 (including the wire-format contract the
hand-written TypeScript types depend on, and the v1-settings upgrade path), `sync/frame.rs` 0 → 13
(hostile-peer framing: oversized prefixes, truncated headers and bodies, desync recovery), `vault.rs`
2 → 12 (path-traversal in every shape), plus `vaultExport`/`vaultImport`/`useHotkeys`/`useIsMobile`/
`updates` on the frontend and smoke suites that mount **every** previously-untested pane and component
against an empty backend and a rejecting one.

**Settled, not just pinned:** ticking a task off deletes its auto-scheduled block, and that is the
intended behavior — the calendar shows what is PLANNED, the done bin in the task list is the record of
what was finished. `sweep_missed`'s doc comment said the opposite ("done tasks keep their blocks");
corrected, and `ticking_a_task_off_removes_it_from_the_calendar` holds the line, since the deletion
happens two layers from the checkbox.

**The done bin is collapsible.** It only ever grows, so it now starts collapsed with its count on the
header (`Done · 3`), remembers the choice in `localStorage`, and reports `aria-expanded`. Reading and
writing that preference is guarded — a private window or blocked site data throws on access, and the
bin forgetting between sessions is a better outcome than the task list crashing. Matches the existing
collapsible-group pattern in `StaleTasks`.

**What's New is version-aware.** The post-update intro used to show one fixed list, so anyone updating
saw the same five v0.8.0 cards forever. Each card now carries the release it shipped in, and the
overlay renders only what is new to *that* install: update every time and you see one card; skip three
releases and you see all three releases' worth. `lib/version.ts` does the comparison (numeric per
segment, so `0.10.0` correctly beats `0.9.0`; prereleases sort below their release; garbage sorts
oldest rather than throwing). Cards for v0.8.1 and v0.8.2 were backfilled from this log — the list had
gone two releases without an update, and version-aware cards with a hole in them are worse than none.
A release with nothing user-visible now shows no overlay at all instead of a heading over empty space,
and the forced dev preview (`?whatsnew=1`) still shows the full list — filtering it against itself
would have rendered it empty, which an E2E now guards.

- Verified: `cargo test --lib` **430**, Vitest **271** (30 files), Playwright E2E **13**,
  `npm run verify:live` — `llm_eval` 261/276.

---

## 2026-08-26

### v0.8.2 — feeds that follow you, tasks that don't get lost, and four quiet failures ✅
A correctness release. v0.8.1's headline was device sync, and v0.8.1 shipped with device sync broken
for anyone using a .ics feed — that is the reason this cut exists.
- **The .ics sync blocker** (`9ae15ce`, entry below). One feed rejected a peer's ENTIRE changeset
  batch. Shipped in v0.8.0, live in v0.8.1, fixed here.
- **.ics feeds now replicate** (`3d888ea`, migration `0022`). Import a calendar on one device and
  every paired device shows it. The subscription syncs; its mirrored events do not — each device
  re-derives them, because `replace_ics_events` is a delete-and-reinsert mirror and replicating those
  rows would have devices tombstoning each other's copies on every refresh. Building it turned up two
  more bugs: an upgrade would have doubled the calendar list (two devices that both had a feed each
  minted a random uuid — the id is now derived from the feed URL so they converge), and a latent
  `apply_upsert` fault that inferred row existence from `updated_hlc`, so a locally-created row that
  had never been stamped read as absent and got INSERTed over. That one affected every synced table.
- **Feeds refresh themselves** (`b835bf4`). Previously a feed only updated when you opened Settings
  and pressed Refresh, so a subscribed calendar could sit stale forever. Now on launch and every 30
  minutes — and the mirror short-circuits when nothing changed, so an unmoving feed never reshuffles
  your day.
- **Missed tasks roll forward** (`1a466fb`, migration `0021`, entry below).
- **Four quiet failures** (`56dee6e`): no error boundary existed, so one throwing pane blanked the
  whole app; a failed vault autosave rendered identically to "nothing to save", so you kept typing
  into a page that had stopped persisting; one hover-revealed click deleted a calendar event with no
  confirm and no undo; and 113 `db.lock().unwrap()` sites meant a single panic poisoned the mutex and
  bricked every later database call until restart.
- **Test infrastructure** (`f3269f3`, `3fb88ff`, `e7a86f8`): `npm run verify` runs every suite in
  tiers with one consolidated report; the calendar — which had ZERO tests despite being the app's
  point — gained 21; and the runner learned not to describe a failing step as passing.
- Verified: `cargo test --lib` **329**, Vitest **109** (22 files), Playwright E2E **10**, `npm run
  build` clean, live `llm_eval` **175/175**, `model_battery` 57–58/58 (the one adversarial case bounces run to run), `real_world_eval` 10/12.

### One .ics feed could block all device sync ✅
`events.ics_sub_id` is an FK into `ics_subscriptions`, which is **not** a synced table. The column was
neither declared in the events `TableSpec::fks` (so never rewritten to a uuid) nor in `skip` (so never
dropped) — it went on the wire as a raw *local* rowid. With `foreign_keys = ON`, a peer holding no
subscription at that id failed the apply INSERT with `FOREIGN KEY constraint failed (787)`, and that
error propagates out of the batch: **the whole apply was rejected, so nothing landed on that device at
all.** Anyone with a .ics feed had sync silently broken against any device not sharing that rowid.
The `ON DELETE CASCADE` hazard the column also carried (deleting an unrelated local feed at a colliding
rowid cascade-deleting a synced event) needed the collision, and was the milder case.
- Fix: `ics_sub_id` joins the events `skip` list — dropped on the wire rather than rewritten, since the
  far side has no equivalent row to point at. The event still replicates, just without a meaningless
  feed id. Reproduced first; the test fails 787 against the old registry.
- **Still open, deliberately:** whether .ics events should replicate between devices at all. They're
  re-derivable from the feed, so a peer subscribed to the same calendar holds both copies. Product call.
- Shipped in v0.8.0, live through v0.8.1. Found by `pushin-3a`, severity correctly re-read by
  `pushin-f2`, fixed by `pushin-5b` in `9ae15ce`. **Verified:** Rust **325**, Vitest 101, build, E2E 10.

### Missed tasks are kicked to the next available slot, deterministically ✅
If a task's planned time passed and you hadn't done it, nothing happened — and in one case it got
actively worse. Fixed end to end.

- **The real bug underneath it:** `schedule_one` capped placement at the task's deadline. Once that
  deadline was *behind* `now`, the cap was a zero-width window, so `place` returned nothing and an
  overdue task got **no block at all** — it silently vanished from the calendar exactly when it most
  needed to be seen. A blown deadline can no longer act as a constraint; the task is planned into the
  next free slot and the `DeadlineMiss` conflict still fires so it reads as late, not lost.
- **The sweep** (`schedule_service::sweep_missed`, migration `0021_task_missed`). A block is stale once
  it ends before midnight of today; stale blocks are deleted, the task is counted missed once for that
  date, and the scheduler pass that follows re-places the freed minutes. Runs inside `reschedule_inner`,
  so every existing path gets it.
- **Pinned blocks roll too, and drop the pin** — a pin says "do it at *this* time", and a day later
  there is no such time left to hold. Previously a pinned block sat in the past forever with the task
  still reading "scheduled": silently dead work. New `db::delete_blocks` is the only caller allowed to
  delete a locked block.
- **Nothing moves mid-day** (the user's chosen cadence). `reschedule_inner`'s sticky cutoff moved from
  `now` to `today_start`, so a block you blew past at 9am holds its slot — and keeps counting against
  its task's estimate — until the day is actually over.
- **Something has to notice time passing.** Nothing called reschedule when the clock merely advanced,
  so `App.tsx` got a day-rollover watcher: once on open (the app may have been shut for days), then a
  60s `toDateString()` comparison, which handles sleep/wake, timezone changes and DST for free.
- **Made visible:** `tasks.missed_count` renders as a "↻ N×" chip on the task row and the Today pane's
  due chips, and is cleared on completion so the nag can't outlive the work.
- Scope held deliberately narrow: only active tasks. A missed meeting is history; habits have streak
  logic a rollover would corrupt; done tasks keep their past blocks as the record of the work.
- **Verified:** `cargo test --lib` **324** green (8 new, incl. the passed-deadline regression and an
  idempotency test that reschedules three times and asserts one count); Vitest **101** (2 new);
  Playwright **10/10**; `npm run build` clean. One E2E casualty worth noting — `getByTitle("Next")`
  substring-matched the new chip's tooltip and broke strict mode, so that locator is now `exact: true`
  (the same class of break that shipped red in v0.8.0).
- Known edge, documented not fixed: the Google block mirror's window starts at `now - 1 day`, so a
  swept block older than that leaves a stale mirrored event behind. Pre-existing; the sweep only adds
  pinned blocks to the set.

---

## 2026-08-25

### v0.8.1 — one Google calendar across every paired device, and pairing that actually pairs ✅
The release that makes multi-device real: connect Google once, and every device you own is connected.
- **Shared Google link** (migration `0020_google_link`). A synced single-row table holds the account
  + the user's OAuth client; `db::adopt_google_link` projects it onto a joining device after every
  sync session. The **refresh token has no SQLite column** — it rides the changeset as a
  keychain-backed secret field (`TableSpec::secrets`) into the peer's OS keychain, and is cleared
  there when the link's tombstone lands. Disconnecting on one device tears down all of them.
- **Google tokens moved to the OS keychain** (`secrets.rs`); `calendar_accounts` keeps only
  non-secret metadata, with the old columns as a fallback when the keychain is unavailable.
- **Concurrent writers on one calendar.** `events.provider`/`external_id` now sync (without them a
  peer treats a synced event as "never pushed" and inserts a duplicate into Google); `adopt_existing`
  matches title+start+end when an event outran its `external_id` over the mesh; and
  `plan_block_mirror` replaces the delete-every-block-and-recreate mirror with a `uuid`-keyed diff
  (insert/patch/delete orphans) that two devices can run at once and that self-heals a raced insert.
- **Device pairing fixes — three real bugs, found by pairing two real Iroh endpoints in one process.**
  The old session ended on a responder *read* while the initiator closed the connection the instant
  it finished; a QUIC close discards unacknowledged data, so the **inviting** device's session failed
  every single time (no peer recorded, no changes applied) while the joiner reported success. The
  session now ends on a `Bye` the initiator waits for. Invites also carried **only the LAN address** —
  `node_addr()` resolves on the first address and local interfaces beat the relay handshake by
  seconds — leaving no fallback when the direct path is firewalled; `make_ticket` now waits for the
  home relay. And `ensure_engine` rebuilds when the mesh secret changes (a device that had its own
  network kept presenting the old secret and failed mesh auth), with `sync_join` bounded at 45s and
  an actionable error instead of an endless spinner.
- **A deadline is not a start time** (`d56f73a`) — ten parser misroutes, all fixed deterministically
  and unit-tested. The headline one: "I need to test some stuff for my job due EOD today" became a
  fixed *"Job Testing" event on the wrong day*. Now a deadline/work cue demotes to a task (skipped for
  explicit clock times and appointments), and untimed events stop silently jumping a day. Behind it:
  duplicate creates fold so the relative date applies, fabricated "…Follow-Up"/"Clarification" sibling
  events are dropped, a cancel clause no longer swallows unrelated creates, mis-routed edit verbs
  route to `updateEvents`, all-day trip spans collapse to one span, "all my X" sweeps remove properly,
  and the user's written time range beats the model's invented one. `llm_eval` gained a `deadline`
  category and went **91% → 100%** (175/175, three consecutive runs); `model_battery` **91% → 98%**
  (57/58 — the one miss is the adversarial "the thing with the guy about the stuff", where asking for
  detail instead of inventing a title is the intended behaviour).
- **Fresh installs could not migrate** (`sync/schema.rs`). `TABLES` lists `google_link`, but
  `migration_sql()` is the frozen `0015` generator, which runs at `user_version` 15 — before `0020`
  creates that table. A brand-new database therefore failed with "no such table: google_link". The
  generator now filters on `TableSpec::added_in`, and a table joining the registry after 0015 applies
  its own columns/triggers from its own migration via `table_sync_sql`.
- **Also in this release**, already on `main` since v0.8.0: the Playwright smoke-suite repair and the
  crash it exposed (`01b43b1` — v0.8.0 shipped with that suite red), a month-old miss now reads as
  stale rather than "due" plus an archive action (`cd87d58`), and the updater re-checks periodically
  instead of only at launch (`1d5a5d0`).
- Verified on the release SHA: `cargo test --lib` **316** green, Vitest **80** green (19 files),
  Playwright E2E **4** green, `npm run build` clean, live `llm_eval` **175/175**, `model_battery` 57/58.

### v0.8.0 — Today landing, tuned models by default, .ics subscriptions ✅
The release that makes Pushin open on your day and ship its own model.
- **Two-space nav + Today pane.** The sidebar holds one space at a time — planner (Today/Calendar/
  Projects/Habits/People) or vault (notes/journal/inbox/graph/labels) — with `prevPlannerView` so
  leaving the vault returns you where you were. `TodayPane` is the new default landing view. Plus a
  flat visual pass across every pane (`--ink-*` tokens, sharp corners, tabular numerals).
- **Pushin's tuned models are the default.** `recommend_model` + `Settings::default()` now pick the
  tuned 7B (≥8GB RAM) or tuned 3B, looked up by id rather than `MODELS[i]`. Settings gained a real
  model switch (download-if-missing + `restart_inference`; saving used to leave the OLD model serving
  until an app restart) and a one-time base→tuned nudge.
- **Read-only .ics subscriptions** (migration 0019, `calendar/ics.rs`): folded lines, escaped text,
  all-day and UTC times → naive-local. RRULE expansion is explicitly out of scope for v1.
- **"Why is this here"** — `scheduler::explain_block`/`explain_all` derive one dominant placement
  reason per block (continuation → dependency → earliest-start → commitment → deadline → earliest).
  Derived on demand, never stored or synced.
- **Parser:** a fuzzy `updateEvents` match no longer rewrites a whole series (the "updated 17 events"
  bug — `best_update_target` picks the single best), timed comma lists get chunked, crammed
  `startTime` JSON is salvaged, and a bare "weekend" is no longer read as a cadence.
- **Habits step aside** from an event dropped on top of them (`resolve_habit_conflicts`), future
  occurrences only, before task scheduling.
- **Lighter runtime:** idle-unload (`idle_unload_minutes`, default 10) + `llm::warmup` +
  lazy memory engine (the embeddings server no longer spawns at boot).
- **Mesh inference bridge** (built, live-unverified): `sync/frame.rs` + `sync/infer.rs` on a separate
  ALPN let a modelless phone borrow a paired desktop's llama-server and embed server.
- **Honest eval:** new `tests/real_world_eval.rs` battery scored against the STORED calendar, and
  datagen paraphrase is now skipped for truth-fragile categories (it drifted rel-date/remove to 0% kept).
- **Repo:** working notes moved to `docs/notes/`, `CLAUDE.md` slimmed ~600 lines into
  `ARCHITECTURE_NOTES.md`, and VitePress `srcExclude` keeps the notes off the public docs site.
- Verified: `cargo test --lib` **267** green, Vitest **80** green (19 files), `tsc` clean,
  `npm run docs:build` clean. Live `llm_eval`/`real_world_eval` not re-run for this release.

## 2026-07-14

### v0.7.1 — parser reliability layer + honest eval ✅
A patch focused on making the on-device parser robust to real-world, messy input — via **deterministic
recovery guards** on the shipped 7B (no model change). This path reliably beat every QLoRA fine-tune
attempt (which plateaued ~91% ±5%); guards reach ~97% *stably*.
- **~15 recovery guards** in `parser::apply_recovery`, each reproduced from real model output, tightly
  gated, unit-tested (lib 224→**243**): multi-task deadline, hedged/absurd-duration/split-fabrication
  guards, vague-time→task, dropped-trip synthesis, multi-event range pairing, compact-time parsing,
  same-time dedup, cancel→remove, negation-aware all-day, per-event duration, and a **clarification
  lever** ("move it" → asks which event instead of guessing).
- **Input chunking** (`split_intents`): long rambling multi-intent messages are split on safe clause
  boundaries (incl. comma-then-action-verb) and extracted per-clause, then merged — fixing the small
  model's habit of dropping intents in busy messages. Battery held **96–98%** (no regression).
- **Honest eval (A1):** removed ~20% eval leakage (14/69 battery prompts were in the training data) —
  a datagen denylist + `finetune/check_leakage.py` CI tripwire. Honest (held-out) accuracy **~93%→~97%**
  (peak run 99%), and *stable* run-to-run.
- New **pressure harness** (`src-tauri/tests/pressure.rs`) drives weird/conversational prompts through the
  full pipeline to surface real-world folds. Plans: [GUARDS_TO_99_PLAN.md](GUARDS_TO_99_PLAN.md),
  [HARD_CASES_PLAN.md](HARD_CASES_PLAN.md).
- **What's New** flow updated to reflect the reliability improvements.
- Verified: `cargo test --lib` **243** green, `tsc` clean, live `llm_eval` 96–98%.

## 2026-06-18

### Mobile UI — responsive phone shell + single-day calendar + quick-capture FAB ✅
Made the Android build actually usable on a phone (verified on the emulator at each step).
- `lib/useIsMobile.ts` (viewport `matchMedia`, reactive) flips `App` between the desktop layout and a
  new `components/MobileShell.tsx`: a bottom tab bar (Calendar / Plan / Tasks / Notes / More) showing
  one full-height pane at a time, the sidebar's other destinations in a "More" bottom sheet, and the
  frameless `TitleBar` hidden on mobile.
- `CalendarPane` parameterized with a `days` prop — `7` = the desktop week grid (default, unchanged),
  `1` = a phone day-view (full-width column, day-stepping nav, single-date header, always-visible
  day-note button). All the old hardcoded-7 spots (grid templates, nav step, all-day-bar clamping) now
  derive from `dayCount`.
- Quick-capture **FAB** in `MobileShell` opens the existing `QuickCapture` modal (the desktop
  Cmd/Ctrl+Shift+N has no touch equivalent).
- Verified: `tsc` clean + Vitest **71** (desktop layout untouched — `matchMedia` mock reports non-mobile),
  and rebuilt APK + emulator screenshots of the shell, day-view, and FAB→capture flow.
- Follow-ups: per-pane touch polish, swipe-between-days; the bigger arc (in-process llama.cpp for
  on-device AI, iOS on a Mac) is unchanged.

### Mobile keystone — Rust core (incl. Iroh sync) cross-compiles to Android ✅
Validated the riskiest assumption in the mobile plan (`~/.claude/plans/virtual-noodling-hoare.md`)
*before* investing in UI/scaffold: does the synced Rust core build for a phone? **Yes** — produced
`target/aarch64-linux-android/debug/libpushin_lib.so` (ELF aarch64, Android 24, NDK r27c). iroh +
tokio + rusqlite + tauri + the whole `sync/` stack compile + link for Android.
- Two desktop-safe changes unblocked it: **(1)** `keyring` scoped to `cfg(not(target_os="android"))`
  (no Android backend) + an in-memory Android stub in `secrets.rs` (Keystore is a follow-up);
  **(2)** `reqwest` → **rustls** (`default-features=false` + `rustls-tls`), since native-tls pulls
  `openssl-sys` which won't cross-compile. The first build got all the way to `openssl-sys` before
  failing — everything else was already compiling for Android.
- Recipe (NDK r27c, env passed via `cmd.exe` since WSL env doesn't cross to Win32): set
  `ANDROID_NDK_ROOT`, `CC_/CXX_/AR_aarch64-linux-android`, `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER`
  → `cargo build --lib --target aarch64-linux-android`. See memory `mobile-cross-compile`.
- Verified no desktop regression: `cargo test --lib` still **188** green.
- **Scope:** this proves the *compile*. Running on a device still needs the full Android SDK + emulator;
  iOS needs a Mac (untested, but same pure-Rust deps + existing keychain path → expected to work).

### Mobile — Pushin builds + RUNS on Android (full APK on an emulator) ✅
Went past the compile: installed the full Android toolchain headlessly (JDK 17 + SDK + emulator), scaffolded
`tauri android init`, built a universal debug APK, and launched it on a headless Android 14 emulator. The
React UI + Rust core came up live (logcat showed the `path/getDataDir` IPC for the SQLite path; no crash).
Screenshot captured. Full recipe + gotchas in memory `mobile-cross-compile`. Notable gotchas: the NDK must
live *inside* the SDK (`$ANDROID_HOME/ndk/<ver>`), and the generated `gen/android` `BuildTask.kt` needed a
`node.exe` + `tauri.js`-path fix (it's gitignored, so reapply after re-init). The debug APK is ~650 MB
(unstripped ×2 ABIs). UI is still the desktop layout (cramped on a phone) — Phase 2 responsive/touch is next.
iOS remains Mac-gated.

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
