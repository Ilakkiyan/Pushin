# Overnight fine-tune runbook — beat the shipped 7B (~93%) on the held-out battery

**Goal:** produce a tuned 7B that **beats the shipped `pushin-arch7b-chat-tuned`** on `tests/llm_eval.rs`
(per-category, not just the noisy TOTAL) while keeping the deterministic wins already in the tree.

**Autonomy model:** each `train`/`eval` runs as a tracked background job; its completion re-invokes me,
so I walk this tree step-by-step overnight with no user input. One GPU (16GB) → **one training at a time**.
Every result is appended to `finetune/out/overnight_results.md`.

## Ship gate (never ship a regression)
Register a model **only if**: TOTAL ≥ baseline across 3 runs **AND** no category drops >1 check vs
baseline. Otherwise keep iterating. If nothing clears by morning → **fall back to shipping the
deterministic `multi-task` guard** (already proven 96% on the shipped 7B) and report findings.

## Baselines (per-category, from this session)
Shipped 7B ≈ **93%** (91/94/95). Weak/again-check categories: hard-event(16 checks), odd-time(8),
multi-event(3), correction(2), dates(7), multiday. v5=82% (overfit), v6=88% (undertrained+diluted).

---

## Decision tree

**NOW (while v7 trains, no GPU needed): E0 — eval-integrity audit.**
Detect train↔eval prompt overlap (e.g. "Prep for my exam Friday" appears in both a template AND the
battery). Quantify leakage → compute a **true** baseline. If baseline is inflated, the whole
comparison shifts. Output → `overnight_results.md`.

**GATE after v7 eval (3 runs):**
- **v7 ≥ ship gate** → register as `pushin-arch7b-tuned-v7`, stop, report **SUCCESS**.
- **v7 close but lagging CAPABILITY categories** (hard-event/odd-time/ranges — data exists, learning
  is the issue) → **E1 (capacity)**.
- **v7 lagging THIN-DATA categories** (correction/multiday/single-task — few kept rows) → **E2 (teacher)**.
- **Both** → E2 first (can't learn absent data), then E1.

### E1 — capacity/recipe (v8)  [~2.5h]
LoRA **rank 16→32** (alpha 32), keep 2ep / lr 2e-4 / upweighted broad corpus. More adapter capacity to
absorb new behaviors without forgetting. If still lagging: try **rank 64** or **3 epochs** (watch overfit).

### E2 — bigger teacher for thin categories (v9)  [~1h datagen + ~2.5h train]
Regenerate ONLY the low-yield categories (correction, multiday-window, single-task, hard-task) with the
**Qwen3-30B-A3B** teacher (`PUSHIN_TEACHER_MODEL=qwen3-30b-a3b`) + **2× candidate counts**, so kept-row
yield rises. Merge into the corpus, retrain the best recipe so far. **First cheap-check:** audit ~10
rejected rows per weak category — if the template `check` is over-strict (rejecting valid labels), loosen
the check instead of swapping teachers (free data, no GPU).

### E3 — realistic/paraphrase expansion (v10)  [~1.5h datagen + ~2.5h train]
`--paraphrase --paraphrase-model qwen3-30b-a3b` to rewrite templates into human phrasings → diversity →
generalization (this is what `realistic.jsonl` did). Add to corpus, retrain best recipe.

### E4 — continue from the shipped model (only if artifacts exist)
Check `finetune/out/` for the shipped model's adapters/merged. If present, LoRA-continue from IT (not the
base) on just the new templates — preserves what made it good, adds the fixes. If only the q4 GGUF exists,
**skip** (can't continue-train from a quantized GGUF).

## Orthogonal (model-independent, do between GPU jobs)
- **Deterministic guards:** for any category whose failures are deterministically fixable (not model
  quality), add an `apply_recovery` guard + unit test (the `multi-task` pattern). Reliable, ships
  regardless of the tune.

## Guardrails / hygiene
- Stop each `llama-server` after eval (free VRAM). One training at a time.
- Keep every version's adapters (`adapters7b_vN`) — never overwrite.
- WSL RAM is 26GB (merge fits). `train.py --gguf` → Q4_K_M. Eval **q4-merged**, default `plan()` path,
  3 runs, judge per-category.
- Append every scorecard + the recipe that produced it to `overnight_results.md`.

## Time budget (~8-10h) → realistically v7 + 2-3 experiments. Priority: v7 → (E1|E2 by diagnosis) → the other → E3.
