#!/usr/bin/env bash
# Convert the GRPO model → GGUF → Ollama (Q4_K_M) → held-out battery via production plan().
set -x
ROOT=/mnt/c/Users/waves/Documents/Projects/Pushin
PY=~/ftvenv/bin/python
cd "$ROOT"

echo "=== convert grpo_merged → f16 GGUF ==="
"$PY" ~/llama.cpp/convert_hf_to_gguf.py finetune/out/grpo_merged \
  --outfile finetune/out/pushin-3b-grpo.f16.gguf --outtype f16 2>&1 | tail -3

echo "=== ollama import + Q4_K_M ==="
ollama rm pushin-3b-grpo 2>/dev/null
printf 'FROM %s/finetune/out/pushin-3b-grpo.f16.gguf\n' "$ROOT" > /tmp/Mf.grpo
ollama create pushin-3b-grpo --quantize q4_K_M -f /tmp/Mf.grpo 2>&1 | tr '\r' '\n' | grep -iE "success|error" | tail -2

echo "=== eval (production plan() path = union) ==="
cd "$ROOT/src-tauri"
EXE=$(ls -t target/debug/deps/llm_eval-*.exe | head -1 | sed 's#.*/##')
cmd.exe /c "set PUSHIN_LLM_URL=http://localhost:11434&&set PUSHIN_LLM_MODEL=pushin-3b-grpo&&target\\debug\\deps\\$EXE --ignored --nocapture" > /tmp/eval_grpo.log 2>&1
echo "--- GRPO scorecard ---"
sed -n '/by category/,$p' /tmp/eval_grpo.log | grep -E "%\)|TOTAL"
echo "=== DONE ==="
