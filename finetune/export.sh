#!/usr/bin/env bash
# Stage 6 (manual path) — convert a merged 16-bit HF model to a Q4_K_M GGUF for Pushin.
# Prefer `train.py --gguf` if Unsloth's bundled llama.cpp works; use this when you want control or
# Unsloth's exporter fails. Requires a built llama.cpp (set LLAMA_CPP to its dir).
set -euo pipefail

MERGED="${1:-finetune/out/merged}"
OUT="${2:-finetune/out/pushin-3b-tuned-q4_k_m.gguf}"
LLAMA="${LLAMA_CPP:-$HOME/llama.cpp}"
F16="finetune/out/pushin-3b-tuned-f16.gguf"

if [ ! -d "$MERGED" ]; then
  echo "Merged model dir not found: $MERGED (run train.py first)" >&2
  exit 1
fi
if [ ! -f "$LLAMA/convert_hf_to_gguf.py" ]; then
  echo "llama.cpp not found at $LLAMA — set LLAMA_CPP=/path/to/llama.cpp" >&2
  exit 1
fi

python "$LLAMA/convert_hf_to_gguf.py" "$MERGED" --outfile "$F16" --outtype f16
"$LLAMA/llama-quantize" "$F16" "$OUT" Q4_K_M

echo
echo "Wrote $OUT"
echo "Next: copy it into Pushin's models dir and add an entry to model_manager::MODELS"
echo "      (id e.g. \"pushin-3b-tuned\"), then run the eval (Stage 7) to compare per-category."
