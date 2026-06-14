# Pushin — Agent Handoff

A continuation brief for a fresh agent. **Read [`CLAUDE.md`](CLAUDE.md) first** — it's the canonical,
hard-won knowledge handoff (architecture, gotchas, file map, test suite). This file adds: the current
release state, the must-know workflow specifics, and the **remaining labeling-system work** (the main
in-flight feature).

---

## Where things stand (v0.3.0)
- **Repo:** `github.com/Ilakkiyan/Pushin`. `main` is default; **releases are version tags** (`v0.1.1` …
  `v0.3.0`). CI: `.github/workflows/test.yml` (rust + vitest + playwright on push/PR) and `release.yml`
  (builds installers + a *draft* release on a version tag).
- **Shipped:** the on-device calendar/scheduler core; the "second brain" (Notion vault + Obsidian
  `[[wikilinks]]`/graph + on-device recall + AI-over-vault + quick-capture/Inbox + Markdown import);
  a frameless auto-hiding title bar; a layered test suite (**133 Rust + 64 frontend + Playwright E2E**);
  and the **core labeling system** (see below).
- **Tree is clean and fully pushed** (`main` == `Working-Branch` == origin at the v0.3.0 commit).

## Must-know workflow (this is a WSL box; the app is a Windows app)
- **Rust: use Windows `cargo.exe`**, not Linux `cargo` (Linux build fails on openssl/pkg-config/webkit).
  `npm` works fine in WSL.
- **Run the app:** double-click **`dev.bat`** on Windows (`npm run tauri dev` with Windows Node). Do
  **not** `npm run tauri dev` from WSL (no webkit/display).
- **Rust tests:** `cd src-tauri && cargo.exe test --lib`. Force a rebuild with `touch src/lib.rs` first
  — `/mnt/c` mtime skew sometimes makes cargo think nothing changed. If the app is running, the
  `pushin.exe` relink can fail but **test exes still build/run**.
- **Frontend tests:** `npm test` (Vitest), `npm run coverage`, `npm run test:e2e` (Playwright — **CI
  only**; this sandbox's OS has no Playwright browser). `npx tsc --noEmit` to typecheck.
- **Live model eval (manual gate):** `cargo test --test llm_eval -- --ignored --nocapture` with the app
  open (serves `:8080`). Baseline ~90% of checks; **judge per-category, not the total** (CLAUDE.md gotcha #1).
- **Contract tests guard drift:** `ipcContract.test.ts` (every `ipc.ts` `invoke("cmd")` must be a
  registered `commands::cmd` in `lib.rs`, and vice-versa) + `bridgeContract.test.ts` (the E2E mock
  bridge). **When you add/rename/remove a command, update `commands.rs` + `lib.rs` + `ipc.ts` together**
  (and `tests/e2e/_mockBridge.ts` if it's used in a flow) or CI goes red.
- **Cutting a release:** bump the version in `package.json`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml`; commit; `git push origin main`; `git tag vX.Y.Z && git push origin vX.Y.Z`
  (triggers `release.yml`). A standalone Windows exe locally = `npm run build` then
  `cd src-tauri && cargo.exe build --release` → `target/release/pushin.exe`.

---

## In-flight feature: the labeling system

**Concept:** a flat, user-defined, **cross-cutting** label taxonomy (the layer above the rigid
task/event/habit *types*) that is **scheduling-aware**. Locked design decisions: **reach = everything**
(tasks/events/habits/pages/projects), **actionable** (labels carry scheduling prefs the deterministic
scheduler honors), **structure = flat labels + an optional `group_name` string** (Context/Area/Energy).
Don't collapse the structural kinds into labels — labels are orthogonal.

### Built (Phases 1, 2, 4) — `git show v0.3.0`
- **Schema** `migrations/0010_labels.sql`: `labels` (name/color/icon/group_name + `pref_window_start/end`,
  `pref_min_chunk`, `pref_max_chunk`, `pref_batch`) + polymorphic `entity_labels(label_id, entity_kind,
  entity_id)` — mirrors `entity_links` (0009).
- **Backend** `db.rs`: `list_labels`/`create_label`/`get_or_create_label`/`update_label`/`delete_label`/
  `merge_labels`/`set_entity_labels`/`labels_for`/`entities_for_label`, and **`resolve_task_prefs`**
  (a task's labels → a merged `scheduler::SchedulePref`). `model.rs`: `Label`/`LabelInput`. `commands.rs`
  + `lib.rs`: the 9 label commands.
- **Actionable scheduling** `scheduler.rs`: `SchedulePref { window, min_chunk, batch }` +
  `schedule_with_prefs(...)` (the old `schedule` now delegates with empty prefs). A label window is a
  **soft** preference — `partition_by_window` orders the free list window-first for `place`, then falls
  back; `min_chunk` overrides the task default. `commands::reschedule_inner` resolves prefs and calls it.
- **Frontend:** `components/LabelPicker.tsx` (shared chip multiselect + create-on-the-fly), attached to
  `TaskListPane`, `HabitsPane`, `ProjectsPane`, `PageEditor`. `panes/LabelPane.tsx` (the cross-cutting
  filtered view + an inline manager/editor with the scheduling prefs). `Sidebar` Labels section.
  `CommandPalette` label jumps. `ipc.ts` + `store.ts` label slice. `view: "label"` in the store.
- **Tested:** db label CRUD/join/merge/`resolve_task_prefs`; scheduler window-pref + min-chunk + soft
  fallback. The IPC contract test enforces the 9 commands.

### TODO (Phases 3, 5, 6) — the actual handoff
**Phase 3 — Calendar color-by-label + filter chips**
- Add a **bulk** query first: `labels_for_entities(kind, ids[]) -> map<id, Label[]>` (db + command +
  ipc) so the calendar doesn't make N `labels_for` calls.
- `CalendarPane`/`MonthPane`: a **color-by-label** toggle (color blocks/events by their primary label
  instead of `kind`) + **filter chips** that filter the grid to a label set. Reuse the existing toolbar
  (mind the responsive collapse already there).

**Phase 5 — AI auto-labeling (on-device, confirmed not silent)**
- A **deterministic keyword→label post-pass** on `parser::plan` output (gym→#health, meeting/standup→
  #work, errands→@errands). **Do not touch the extraction prompt** (small-model reliability, gotcha #1).
- Surface as **confirm chips** in `ChatPane` (reuse the "Remember this?" memory-chip UI). On confirm,
  `setEntityLabels` on the just-created task/event.
- Later: a tiny `chat_json` classifier over the existing label set (opt-in).

**Phase 6 — System labels + polish**
- **Read-only "system labels"** derived from the structural kinds (Fixed event / Busy / Habit / Task)
  so unified views/filters can include them without storing rows.
- A **`#`-trigger inline label chip** in the BlockNote editor (mirror the `[[` `pageLink` spec in
  `editorSchema.tsx` + a `SuggestionMenuController triggerCharacter="#"` in `PageEditor`).
- **Scheduler batching** (`pref_batch`): order same-label tasks consecutively in the EDF/priority queue
  so their blocks cluster.
- **Events UI surface:** events have no editor pane, so they're currently label-able only via the
  backend/AI, not the UI. Add an event-detail popover (in `CalendarPane`) with a `LabelPicker`.
- **Not run live yet:** migration 0010 + the scheduling bias need a `dev.bat` launch to eyeball (tag a
  task with a morning window → confirm its blocks land AM and at the min-chunk length).

---

## Other open items (see CLAUDE.md "Known limitations")
- **Test gap:** the full Google `sync()` orchestrator end-to-end (leaf fns are httpmock-tested; needs a
  seeded account/token in DB + keychain). PageEditor *real* editing is Playwright-only (jsdom can't drive
  ProseMirror).
- **Google sync** compiles + leaf-fns tested but the **first live connect is unverified**.
- Mobile (needs in-process inference), GPU engine builds, a bundled `llama-server` sidecar, a hosted
  relay for the public booking page, and calendar drag-to-resize are all still open.

## Working style (the human)
Wants **fast iteration + honest assessment** — when something is the model's limitation vs. a code bug,
say which and prove it (test against the live `:8080`, don't just compile). Verify changes *work*, not
just build. Recommend the 7B model when reliability matters. They release frequently (version tag per
batch) and like a standalone `.exe` sent over after a release.
