#!/usr/bin/env bash
# Self-contained overnight pipeline: datagen (v2 balanced) -> train -> GGUF -> quantize -> eval.
# Runs as ONE background command so it needs only a single permission approval. All output -> LOG.
LOG=/tmp/pipeline_overnight.log
ROOT=/mnt/c/Users/waves/Documents/Projects/Pushin
CARGO=/mnt/c/Users/waves/.cargo/bin/cargo.exe
PY=~/ftvenv/bin/python
EVAL_EXE='target\debug\deps\llm_eval-b692a766d815308c.exe'
{
  echo "=== OVERNIGHT PIPELINE START $(date) ==="
  cd "$ROOT" || exit 1

  # 1) WAIT for the already-running datagen to finish (don't restart — preserve its progress).
  #    Cap the wait at ~3h so a hang can't block the rest forever.
  echo "=== [1/5] WAIT FOR DATAGEN $(date) ==="
  waited=0
  while pgrep -f 'example datagen' >/dev/null 2>&1; do
    sleep 30; waited=$((waited+30))
    if [ $waited -ge 10800 ]; then echo "datagen wait timed out after 3h — proceeding"; break; fi
  done
  echo "datagen done (waited ${waited}s). train_rows=$(wc -l < finetune/data/dataset.jsonl 2>/dev/null) holdout_rows=$(wc -l < finetune/data/holdout.jsonl 2>/dev/null)"

  # 2) TRAIN — manual QLoRA loop, 2 epochs on the balanced set (saves adapters + merged)
  echo "=== [2/5] TRAIN $(date) ==="
  "$PY" finetune/train.py --data finetune/data/dataset.jsonl --epochs 2

  # 3) CONVERT merged HF -> f16 GGUF (llama.cpp; handles Qwen2.5 vocab correctly)
  echo "=== [3/5] CONVERT $(date) ==="
  "$PY" ~/llama.cpp/convert_hf_to_gguf.py finetune/out/merged \
      --outfile finetune/out/pushin-3b-v2.f16.gguf --outtype f16

  # 4) OLLAMA import + Q4_K_M quantize (re-register pushin-3b-tuned)
  echo "=== [4/5] QUANTIZE $(date) ==="
  ollama rm pushin-3b-tuned 2>/dev/null
  printf 'FROM %s/finetune/out/pushin-3b-v2.f16.gguf\n' "$ROOT" > /tmp/Mf.v2
  ollama create pushin-3b-tuned --quantize q4_K_M -f /tmp/Mf.v2

  # 5) EVAL v2 — held-out battery, union single-call mode (env passed via cmd.exe to the Windows exe)
  echo "=== [5/5] EVAL $(date) ==="
  ( cd src-tauri && cmd.exe /c "set PUSHIN_LLM_URL=http://localhost:11434&&set PUSHIN_LLM_MODEL=pushin-3b-tuned&&set PUSHIN_EVAL_UNION=1&&$EVAL_EXE --ignored --nocapture" ) > /tmp/eval_v2.log 2>&1
  echo "--- v2 SCORECARD (baselines: base-3B 84%, 7B 82%, 14B 84%, tuned-v1 83%) ---"
  sed -n '/by category/,$p' /tmp/eval_v2.log

  echo "=== PIPELINE COMPLETE $(date) ==="
} > "$LOG" 2>&1
