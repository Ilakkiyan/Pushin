# Fine-tuning Pushin's on-device parser

Goal: specialize the **default small model (Qwen2.5-3B)** into Pushin's narrow `text → schema JSON`
function so it reaches ~7B reliability on *this task* — retiring the "use the 7B for consistency"
caveat. This is **task specialization, not new knowledge.** We keep the locked principle (*LLM
parses, Rust schedules*): labels carry day-words + times, never absolute dates — Rust still owns the
math, and the deterministic recovery layer (`parser::apply_recovery`) stays as a runtime safety net.

## The format contract (Stage 0 — everything hangs on this)

There is ONE format, used identically by the teacher (labeling), training, and inference:
- system = `parser::system_prompt(events, settings)` (instructions + current calendar + examples),
- user = the message (+ prior turns when `needs_history` fires),
- assistant = the `response_schema()` JSON.

The fine-tuned student runs the **single union call** (`parser::union_label`'s path), *not* the
router+extractor pipeline — that router exists to prop up an unreliable base model, which is exactly
what we're fixing. Format drift between train and inference is the #1 cause of fine-tune flops, so
datagen reuses the real `system_prompt` / `build_messages` / `response_schema` verbatim.

## Pipeline & how to run it

### Stages 1–4 — datagen (Rust, reuses the live parser)
`finetune/datagen/` is a cargo **example** (links `pushin_lib`, uses dev-deps). For each templated
prompt with a *known* correct outcome, it asks the teacher for a label, runs the real `store_plan` +
a per-template `check` (reject-sampling), and writes passing `(prompt → JSON)` rows as ChatML.

```bash
# Inspect the generated prompts offline (no server):
cargo run --example datagen -- --dry-run            # from src-tauri/  (use cargo.exe on WSL)

# Generate the dataset against a teacher (Qwen2.5-14B-Instruct — the GGUF Pushin already downloads):
#   serve it on :8080, then:
PUSHIN_TEACHER_MODEL=qwen2.5-14b-instruct-q4_k_m cargo run --example datagen --release
# → finetune/data/dataset.jsonl  +  finetune/data/holdout.jsonl  (stable hash split)
```
Flags: `--out`, `--holdout`, `--limit N`, `--holdout-frac 0.1`. Teacher via `PUSHIN_TEACHER_URL` /
`PUSHIN_TEACHER_MODEL`. The **label is the teacher's RAW schema JSON** (schema-valid by grammar);
only labels whose `store_plan` outcome passes the template's `check` are kept.

**Why templates (not the eval battery):** templates carry their own ground truth, so labeling is
fully automatic. The hand-written battery in `src-tauri/tests/llm_eval.rs` stays the **held-out
eval** — never train on your eval set. Widen the slot consts in `templates.rs` to scale up; add
Stage-1b free-form prompts later (label via teacher + self-consistency, since they lack known truth).

### Stage 5 — train (Python / Unsloth, on the GPU)
```bash
pip install torch --index-url https://download.pytorch.org/whl/cu128   # Blackwell/5080: CUDA 12.8 FIRST
pip install -r finetune/requirements.txt
python finetune/train.py --data finetune/data/dataset.jsonl            # QLoRA SFT, loss on assistant only
```
3B QLoRA fits 16GB comfortably (minutes–~1h). Saves LoRA adapters + a merged 16-bit model.

### Stage 6 — export to GGUF
`python finetune/train.py --gguf` (Unsloth's bundled llama.cpp) **or** `bash finetune/export.sh`
(manual; set `LLAMA_CPP`). Produces a **Q4_K_M** GGUF — the same quant Pushin ships, so you eval what
you ship.

### Stage 7 — eval & iterate
Serve the tuned GGUF and point the held-out battery at it:
```bash
PUSHIN_LLM_MODEL=pushin-3b-tuned cargo test --test llm_eval -- --ignored --nocapture
```
Compare the **per-category** scorecard vs baseline-3B and the 7B (judge per-category, not the noisy
total). A lagging category → widen its templates → regenerate → retrain.

## Remaining integration (TODO — after the first tuned model exists)
1. **Register the model:** add a `pushin-3b-tuned` entry to `model_manager::MODELS` and drop the GGUF
   in the models dir.
2. **Inference switch:** make `parser::plan` take the **union single-call path** for the tuned model
   (skip the router) — that's the path it was trained on, and it's faster (one call, not 2+). Gate it
   on the selected model id (or a `Settings` flag). `apply_recovery` already runs either way.

## Files
- `datagen/templates.rs` — slot templates → `(prompt, seed, history, check)` with known truth.
- `datagen/main.rs` — labels via `parser::union_label`, reject-samples, writes ChatML JSONL.
- `train.py` — Unsloth QLoRA SFT (assistant-only loss).
- `export.sh` / `requirements.txt` — GGUF export + training deps (Blackwell notes).
