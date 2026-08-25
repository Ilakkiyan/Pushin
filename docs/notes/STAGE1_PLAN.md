# Stage 1 — Harden the core wedge (individuals-first)

**Goal:** make Pushin's single differentiator — a reliable, 100%-on-device planning AI + a deterministic
scheduler — *actually reach the user*. Stage 0 (perf) is done; this stage is about reliability + trust,
not new surface area. Teams work is explicitly deferred (see `TEAMS_PLAN.md`).

**Status of the sub-goals from the roadmap sketch:**
- Load-speed / idle-RAM wins → **DONE in Stage 0** (warm start, backoff poll, lazy embed, idle-unload).
- Ship the fine-tuned model → **partially done** — tuned GGUFs (`pushin-arch3b-tuned`,
  `pushin-arch7b-chat-tuned`) are in `model_manager::MODELS` and hosted on HF, but **users don't get
  steered to them** (Item A). Pushing past the accuracy ceiling is Item B.
- Scheduler explainability → **not started** (Item C).

---

## Item A — Route users to the tuned model  *(highest leverage, smallest effort — do first)*

**Problem (verified in code):**
- `model::Settings::default().model_id` = `"qwen2.5-7b-instruct-q4_k_m"` — the **vanilla base 7B**. New
  users download and run the *untuned* model by default.
- `model_manager::recommend_model()` maps RAM tiers only to `MODELS[0..2]` (base 3B/7B/14B). The tuned
  entries (`MODELS[3]`, `MODELS[4]`) are **never** recommended, so the first-run setup card never offers
  them.
- Net: the reliability work we shipped is opt-in-by-luck. This is the biggest individual-facing win and
  it's a routing change, not model work.

**Tasks:**
1. **Default → tuned 7B.** Change `Settings::default().model_id` to `"pushin-arch7b-chat-tuned-q4_k_m"`.
   (Update the stale comment at `model.rs:550` that justifies the base 7B — it predates the tuned models.)
2. **`recommend_model()` → tuned tiers.** Map RAM to the tuned models: low-RAM → `pushin-arch3b-tuned`,
   comfortable → `pushin-arch7b-chat-tuned`. Decide the high-RAM (≥15 GB) case: keep base 14B (no tuned
   14B exists) **or** recommend tuned 7B as the reliability pick regardless — recommend tuned 7B, since
   accuracy > raw size for this task. Encode that reasoning in the `reason` string.
3. **Existing-user nudge (don't force).** A user already on the base 7B keeps it (their choice). Add a
   one-time, dismissible Settings hint: "A more reliable on-device model is available — switch?" wired to
   the existing `restart_inference` model-switch path. No silent re-download.
4. **Copy pass.** InferenceSetup + SettingsPane model names/notes should make "Pushin (tuned)" the obvious
   default without dark-patterning the base models out.

**Verify:** `cargo test --lib` + `npx tsc --noEmit` + `npm test` green; then a **live** run — fresh
profile → first-run card recommends tuned 7B; `llm_eval`/`model_battery` against the tuned model on
`:8080` still ≈ the baseline (~93%). Cannot be proven on the WSL box (no live server) — needs the Windows
sit-down session.

**Risk:** changes the default first-run download (tuned 7B ~4.7 GB vs base 7B). Aligned with the goal;
verify the HF URLs + SHAs resolve before shipping (network-gated, do it live).

---

## Item B — Push the tuned model past the ~93% ceiling  *(GPU box, iterative)*

**Grounding:** memory `tuned-model-eval-ceiling` + `finetune/OVERNIGHT_PLAN.md` — the shipped 7B sits
~93% on `llm_eval`; the remaining gap is malformed-JSON / model-quality, so **the next lever is datagen,
not more deterministic guards**. Weak categories: hard-event, odd-time, correction, multiday.

**Tasks (follow the OVERNIGHT_PLAN decision tree; each train/eval is a tracked background job):**
1. **Eval-integrity audit (E0)** — run `finetune/check_leakage.py`; quantify train↔eval prompt overlap
   and compute a *true* baseline before comparing new runs.
2. **Thin-data categories (E2)** — regenerate correction / multiday / single-task / hard-task with a
   bigger teacher (`qwen3-30b-a3b`) + 2× candidates. **Cheap-check first:** audit ~10 rejected rows per
   weak category — if the template `check` is over-strict, loosen it (free data, no GPU).
3. **Paraphrase expansion (E3)** — `--paraphrase` template rewrites for phrasing diversity.
4. **Capacity (E1)** — LoRA rank 16→32 if capability categories (not data) are the bottleneck.
5. **Ship gate (never regress):** register a new model only if TOTAL ≥ baseline across 3 runs AND no
   category drops >1 check. Else keep the current tuned 7B.

**Verify:** per-category scorecard on the held-out battery, 3 runs; append to
`finetune/out/overnight_results.md`. This is the user's GPU box only.

**Dependency:** ships *through* Item A's routing — a better tuned GGUF just replaces the HF asset + a new
`MODELS` entry/version.

---

## Item C — Scheduler explainability ("why is this here / why did it move?")

**Why:** Motion's most-complained-about weakness is the black-box reshuffle. Pushin's scheduler is
**already deterministic** (`scheduler.rs` docstring: "Deterministic, explainable, fast") — it can hand
back the *reason* for each placement essentially for free. This is a trust win unique to a local solver.

**Current state:** the scheduler surfaces reasons for **unplaced** tasks (deadline won't fit) and
conflicts, but scheduled blocks carry no placement rationale.

**Tasks:**
1. **Emit a placement reason per block** in the scheduler: the dominant constraint that fixed it — deadline
   pressure (EDF), priority order, a dependency (must follow X), fixed-event avoidance, work-hours/sleep
   window, or chunk-splitting. Deterministic, unit-testable.
2. **Thread it through** `ScheduleResult`/block model → IPC → the calendar block detail/hover.
3. **UI:** a one-line "why here" on a scheduled block (e.g. "Placed before your Fri deadline, after
   *Draft outline*"). Keep it quiet/hover-reveal per the design direction.
4. **Bonus:** on re-plan, a short diff ("moved *Study* earlier — new deadline on *Essay*").

**Verify:** unit tests on the reason selection (deterministic); Vitest on the block detail; live sanity
that reasons read true against real plans.

---

## Sequencing & recommendation
1. **Item A** now — small, high-leverage, mostly done infra; makes every existing reliability gain land.
2. **Item C** next — self-contained feature, deterministic + testable on this box, big trust payoff.
3. **Item B** in parallel on the GPU box — long-running, iterative, ships silently through A's routing.

**Definition of done for Stage 1:** new users get the tuned model by default, scheduled blocks explain
themselves, and the tuned model's per-category eval is at or above the current baseline.
