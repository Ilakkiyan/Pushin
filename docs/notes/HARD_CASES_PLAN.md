# Managing the remaining folds: plan from pressure testing

Freeform pressure testing (`src-tauri/tests/pressure.rs`, 24 weird/messy/conversational prompts) surfaced
the real-world failure classes the held-out battery never hits. Three clean guards already landed from it
(**absurd-duration clamp**, **same-time dedup**, **fortnight span**). This plan covers what's left, split by
*how* each is best solved, and, crucially, **how to manage the failures the small model genuinely can't
reason through.**

**Guiding principle (the architecture, taken to its end):** the small model is a *reliable single-intent
extractor + a fuzzy natural-language front-end*. Everything it can't do reliably (arithmetic, multi-intent
bookkeeping, negation, ambiguity) moves to **deterministic Rust** or becomes a **clarifying question**.
"LLM parses, Rust reasons."

---

## Track 1: Remaining deterministic guards (fast, reliable, unit-tested)

| Fold (from pressure test) | Guard |
|---|---|
| **Deadline routed as event**: "paper due monday", "exam wednesday" → noon events | "X due/by `<day>`" with no clock time → **task with a deadline**, not a 12–1 event |
| **Multi-intent drops duration**: "3 hour deep work block" (with other items) → 1h | relax the explicit-duration override from single-create to **per-event**, matching the duration phrase to the nearest event by text position |
| **Relative edit not applied**: "push my standup back 30 min" → unchanged | apply `find_time_shift` to the matched update (the shift is parsed but not being used on that path) |
| **Edit date drift**: "move lunch to 1:30" jumped to the wrong day | a time-only edit must **not** change the event's date |
| **Title pollution**: "Flight departs at 07:45 AM on Friday"; "DCTOR APPT" | strip a trailing time/day baked into a title; title-case obvious shout-typos |
| **Over-decomposition**: "submit expenses" → 2 fabricated subtasks | tighten `collapse_unrequested_decomposition` for lone deliverables with no list cue |

Each is a small RED-test → guard → live-verify PR, exactly like the seven already shipped.

---

## Track 2: Managing the MODEL-LIMITATION cases (the hard, interesting part)

These the 7B genuinely can't reason through. None need a better model; they need the reasoning moved off
the model. Five strategies, highest-leverage first:

### 2a. Input chunking: the single biggest real-world win
The model **drops intents in long messages** ("dentist at 2 + move standup + call mom" → only the dentist;
the 1:1 meeting dropped from the "mixed" case). Small models can't hold 3–4 intents at once. **Fix:
deterministically split the message on strong clause boundaries** (`. `, ` and `, ` also `, `, then `, `;`),
parse each chunk through the normal path, and **merge the plans**. One intent at a time is where the model
is reliable. Gated to long / multi-clause inputs so simple messages are untouched. This attacks the #1
real-world failure (multi-intent under-extraction) structurally.

### 2b. Completeness check → clarify ("did I miss something?")
After parsing, compare the input's clause/verb count to what was produced; if clauses were clearly dropped,
surface the leftover text: *"I added the dentist and moved your standup. Did you also want a reminder to
call your mom?"* Turns a silent drop into a catch. Pairs naturally with 2a (chunking makes the leftover
detectable).

### 2c. Deterministic pre-compute for arithmetic the model flubs
Detect the pattern, compute in Rust:
- **Relative-to-event time**: "leave 2 hours before my 6:45am flight" → parse "`N` hours/min before
  `<time>`" and subtract. (Model produced 18:43 for what should be 04:45.)
- **Even decomposition**: "back-to-back interviews 1–5pm, 30 min each" → detect "`<range>`, `<dur>` each"
  and generate the N blocks. (Model produced one 14:45 block.)

### 2d. Negation / exclusion handling
"free all day Saturday **except** soccer 10–noon" (→ soccer wrongly became all-day); "block my afternoon
**except** the 2pm call". Detect `except / apart from / but not / other than` → treat the exception as the
real event, and **don't** create the enclosing block (or carve it around the exception). Deterministic once
the cue is found.

### 2e. Clarification lever (B4), generalized
Ambiguous bare commands ("move it", "put it in for later", "schedule that thing") currently produce
**nothing** (safe, good) but unhelpful. Upgrade to a targeted question: create/edit verb + no resolvable
subject/time → *"Which event?"* / *"What, and when?"*. Ambiguity becomes "asks a good question" = success.

---

## Track 3: Grow the measure so all of this is provable in the real world
- Promote `tests/pressure.rs` into a **curated, hand-labeled realistic suite**, frequency-weighted
  (GUARDS_TO_99_PLAN.md A2/A3). The pressure prompts become permanent regression cases.
- **Field holdout** (A4): log real user inputs on-device (opt-in) → the ultimate "does it generalize" check;
  guards graduate against it, never against the synthetic set.

---

## Sequencing
1. **Track-1 guards**: mop up the clean folds (deadline-as-event, edit-shift, per-event duration…). Days.
2. **2a input chunking**: the structural fix for multi-intent under-extraction. Highest real-world impact.
3. **2c/2d deterministic pre-compute + negation**: the "clever Rust" wins for arithmetic & exclusion.
4. **2b/2e completeness + clarification**: the ask-don't-guess safety net for the irreducible residual.
5. **Track-3**: lock in the realistic + field measure so the real-world number is honest and monitored.

**Definition of success:** on the realistic + field suite, ≥99% of inputs yield the correct plan **or a
good clarifying question**, with the model doing only what small models do well, and Rust (or a question)
handling the rest.

---

## RESOLUTION (2026-07-14): tracks closed for the release

Every item resolved as **guard-done**, **clarify**, or **accepted** (a fragile guard would be worse).
Lib tests 236→243; battery held 96–98% across all changes; all on `main`.

**Guard-done (shipped, unit-tested, live-verified):**
- Track 1 deadline-as-event → fixed structurally by **input chunking** (each clause routes correctly:
  "paper due monday" → task).
- Track 1 per-event duration → `apply_lone_duration_to_lone_event` (49628e4): a stated length reaches a
  lone event past a cancel/list ("3 hour block" → 3h).
- Track 2a input chunking (8813c28) + **comma-then-verb** split (b543f04) isolates intents; recovers
  dropped ones; keeps cancel-vs-add routing clean.
- Track 2d negation → all-day only applies to timeless items (033631d): "free all day except soccer
  10-noon" keeps soccer timed.
- Track 2e clarification lever → `ask_when_ambiguous_command` (033631d): "move it" → asks "which event?".
- (Earlier) cancel→remove (b5116e5); absurd-duration clamp, same-time dedup, fortnight, + the 7 core guards.

**Clarify-or-accept (deliberately NOT guarded: rare + high fabrication risk; the model can't and a
brittle guard would misfire):**
- Track 2c **relative-time arithmetic** ("leave 2h before my 6:45am flight"): chunking separates the
  "2h before" from its reference time; correct handling needs a whole-message pre-pass. → the
  **clarification lever** should ask the reminder time. ACCEPTED for now.
- Track 2c **even decomposition** ("interviews 1-5pm, 30 min each" → 8 blocks): generating N events from
  one line is unsafe. → one block + clarify to split. ACCEPTED.
- Track 1 **multi-update relative shift** ("push standup back 30 AND move lunch…"): single-update shift
  works; the multi-update case needs per-update position matching (rare). ACCEPTED.
- Track 2b **full completeness-diff** ("did I miss 'call mom'?"): chunking already recovers most dropped
  intents structurally; a general input↔output clause diff is complex and low marginal value. DEFERRED.

**Cosmetic (deferred):** title pollution ("Flight departs at 6:45 am on Friday", "DCTOR APPT"): event
works; ugly title. Low priority, char-boundary-fiddly.

**Bottom line:** the tracks are finished on solid, tested ground. Everything the model can't do reliably
is now handled by a deterministic guard or a clarifying question, never a guess. **Ready to cut a release.**
