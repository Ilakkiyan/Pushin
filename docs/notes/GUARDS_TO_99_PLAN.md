# Path to a real-world 99% — deterministic guards + an honest battery

**Thesis (proven this session):** the reliable way to raise Pushin's parse accuracy is *deterministic
guards on the shipped model*, not fine-tuning. Three guards this session (multi-task deadline,
hedged-event fabrication, explicit-duration override) each converted a real failure class into a
permanent, model-independent fix — pushing the shipped 7B to ~94%+, above every fine-tune attempt
(best 91%, ±5% noise).

**But 99% only means something if the battery reflects reality.** Today's battery is ~20% leaked into
training (E0) and hand-weighted by developer intuition, so a high score is partly a mirage. So this plan
runs **two interlocked workstreams**: fix the *measure* so the number is honest, and systematically drive
guards against it. You can't optimize toward a number that doesn't map to real use.

**Definition of done:** on a *de-leaked, frequency-weighted* battery that includes realistic phrasings
**and** a real-user field holdout, **≥99% of inputs produce either the correct plan OR a good clarifying
question.** (A well-placed question is a success, not a miss.)

---

## Workstream A — Make the battery reflect the real world (do FIRST; the number is meaningless otherwise)

**A1. Kill leakage, permanently.** 14/69 eval prompts are in the training data. Add a **denylist** to
datagen (skip any templated prompt whose normalized form matches an eval prompt) **and a CI/unit test
that FAILS if any `llm_eval.rs` prompt appears (normalized) in any training row.** Leakage becomes
impossible going forward. (Detector already written: `scratchpad/eval_leakage.py` — promote it into the
repo as a test.)

**A2. Weight by real-world frequency.** Tag every case with a `weight` = how often that shape actually
occurs in use (simple single event/task ≫ gnarly 4-way multi-intent). Report a **weighted headline**
(what users feel) alongside the unweighted stress score. 99% *weighted* is the real target.

**A3. Expand with realistic phrasing.** Current cases are clean/textbook. Real inputs are messy:
lowercase, typos, "gotta", voice-to-text run-ons, trailing "yeah". Grow 69 → several hundred cases
across realistic variants (reuse the `--paraphrase` realistic-datagen style, but for the EVAL set, kept
strictly held-out).

**A4. Field holdout (gold standard).** Collect *real* user inputs — on-device, opt-in — into a separate
eval set **guaranteed never trained on**. This is the truest real-world measure. Start tiny, grow it;
it's the ultimate check that guards generalize instead of gaming the synthetic battery.

**A5. Per-case provenance tags.** Each case: `category`, `weight`, `source` (synthetic|realistic|field),
`difficulty`, `leaked` (should be false everywhere after A1). Enables honest sub-scores and prevents
silent leakage regressions.

---

## Workstream B — Drive guards to 99%

**B1. The repeatable loop** (exactly what worked this session):
serve model → run battery with `PUSHIN_PLAN_DEBUG` + `PUSHIN_EVAL_DEBUG` → dump each failure's *raw model
output* → classify → for guardable ones write a RED unit test, add the guard in `parser::apply_recovery`,
verify GREEN + re-run the live battery to confirm no regression. Repeat until the residual is only
"ambiguous" or "model-limit."

**B2. Failure taxonomy** — every failure is exactly one of:
- **Guardable** → deterministic fix in `apply_recovery`. Sub-types: fabrication (extra/placeholder
  event), mis-route (event↔task↔habit), dropped field (deadline/duration/date), format salvage
  (malformed-JSON times), positional assignment (N ranges → N events).
- **Ambiguous** → emit a clarifying question (Workstream B4).
- **Model-limit** → can't guard, can't ask → accept, or note for the next tune.

**B3. Known guard targets** (from this session's audit, prioritized by impact):
1. **multi-event range cross-assignment** — "lunch 12-2 and party 6-10": the model gives one event a
   start w/ no end and the other an end w/ no start. Extend the positional-range guard
   (`backfill_event_fields` ~L1968) to *repair* cross-assigned/half-filled ranges, not just fill unset.
2. **absolute-date multi-week span** — "from 6/12 for two weeks": model emits nothing. Synthesize one
   all-day multi-day event from `find_explicit_date` + `find_span_days` when the text clearly has both.
3. **vague-time → task** — "spend Saturday afternoon cleaning": model fabricates a clock time + makes an
   event. Demote to a task when the only time signal is a part-of-day word (no clock time, no range).
4. **malformed-JSON time salvage** — `startTime="09:00','endTime':'17:30"`: split the crammed end back
   out into `end_time` in `unescape_plan`. (Won't fix a mis-read time, but fixes the structure.)
5. **create+update dup on relative dates** — "project review in two weeks": model emits create *and* a
   phantom update of the same title; reconcile to one, resolve +14d.

**B4. Clarification lever (the last mile).** For genuinely ambiguous inputs (missing an essential field
with no safe default), emit a question instead of guessing. Metric becomes **correct-or-asks**. Requires
an ambiguity detector + making the battery *credit* a good clarifying question. This is how the residual
few percent become "successes."

**B5. Anti-overfit guardrails (critical — this is how the number stays real):**
- Every guard: conservative, tightly gated, unit-tested, **live-validated to not regress** other cases.
- **NEVER pattern-match a specific eval string** — a guard must key on a *general* signal (a hedge
  parenthetical, a part-of-day word, an explicit-duration phrase), never "if title == 'Break Time'".
- The A1 CI leakage check + the A4 field holdout are the tripwires: if guards start gaming the synthetic
  battery, the field-holdout score diverges and exposes it.

---

## Milestones & metrics
- **M0 — honest baseline:** de-leaked + weighted score of *shipped 7B + current 3 guards*. The true
  starting number (today's ~94% is inflated by leakage; M0 tells us the real floor). [PENDING final run]
- **M1:** land B3 targets 1–5 → target ~96-97% honest.
- **M2:** realistic-phrasing expansion (A3) + clarification lever (B4) → ~98%.
- **M3:** field-holdout (A4) validation + residual guards → **real-world 99% (correct-or-asks)**.

## Sequencing (recommended)
1. **A1 first** (de-leak + CI check) — makes every subsequent number trustworthy. ~half day, no GPU.
2. **B3 guard targets** via the B1 loop — the bulk of the accuracy gain, reliable, unit-tested. Iterative.
3. **A2 weighting + A3 realistic expansion** — reshape the battery toward real use; recompute the honest number.
4. **B4 clarification lever** — convert the ambiguous residual to "asks a good question."
5. **A4 field holdout** — stand up the real-world tripwire; graduate guards against it.

> Guards are cheap, permanent, and testable; each is a small PR with a RED→GREEN test. This is a steady
> grind to 99%, not a gamble — the opposite of the fine-tune's ±5% dice-rolling.
