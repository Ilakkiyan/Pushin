# Training run — 30B-teacher datagen → QLoRA (2026-07)

Goal: push the tuned student past its real-world ceiling by regenerating the SFT set with a **stronger
teacher** (Qwen3-30B-A3B, a 30B-total/~3B-active MoE — the "3B+32B combo") for higher-quality labels AND
realistic human phrasing. Motivation: the held-out battery reads ~96% but real prompts fail often — the
gap is (a) label quality on hard categories and (b) template phrasing being cleaner than real usage.
Both are what a 30B teacher + `--paraphrase` fix.

Two environments (this box):
- **Datagen** — Windows: `cargo` example + a `llama-server` serving the teacher. Produces the JSONL.
- **Training** — WSL: `~/ftvenv` (Unsloth) + `~/llama.cpp` + `ollama`, on the RTX 5080.

---

## Step 1 — Serve the 30B teacher (Windows)

Use the **Q3_K_M** GGUF (`Qwen3-30B-A3B-Instruct-2507-Q3_K_M.gguf`, 14.08 GB from
`bartowski/Qwen_Qwen3-30B-A3B-Instruct-2507-GGUF`). The Q4 (17.7 GB) does NOT fit the 5080's 16 GB —
CPU-offloading its experts collapses to ~88 s/template (~45 h run). Q3 fits fully on GPU and hits
**~162 tok/s**, IF you avoid the 4-slot default (its KV cache steals the VRAM that would otherwise hold
the model). Serve it single-slot:

```bash
BIN="$APPDATA/com.pushin.app/bin/llama-server.exe"       # or /c/Users/<you>/AppData/Roaming/...
MODEL="$APPDATA/com.pushin.app/models/Qwen3-30B-A3B-Instruct-2507-Q3_K_M.gguf"
"$BIN" -m "$MODEL" --host 127.0.0.1 --port 8082 -c 4096 --parallel 1 -ngl 99 --no-mmap
```

`--parallel 1` is the key (datagen is sequential, so 1 slot is all it needs, and it frees ~1.2 GB of KV
cache → the whole model fits on GPU → ~14× faster than the 4-slot default). Confirm: VRAM ≈ 14.6/16.3 GiB
and `curl -s http://127.0.0.1:8082/health` → `{"status":"ok"}`. Free other GPU apps if it spills to CPU
(watch `nvidia-smi`; a warm 200-token gen should be ~1.2 s, not ~18 s). At ~162 tok/s the full datagen is
~1.2 h.

## Step 2 — Generate the dataset with the 30B (Windows)

The model NAME is cosmetic to llama-server (it serves the loaded GGUF); point datagen at the port. The
30B is both the **label** teacher and the **paraphrase** teacher (realistic human phrasing — the biggest
lever for closing the real-world gap). Anti-leakage auto-drops any battery prompt.

```bash
cargo run --release --example datagen --manifest-path src-tauri/Cargo.toml -- \
  --teacher-url http://127.0.0.1:8082 --teacher-model qwen3-30b-a3b \
  --paraphrase --paraphrase-url http://127.0.0.1:8082 --paraphrase-model qwen3-30b-a3b \
  --out finetune/data/dataset.jsonl --holdout finetune/data/holdout.jsonl
```

- ~1750 templates × (router + union label attempts) + 1 paraphrase each → several thousand calls; on a
  partially-offloaded 30B this is an **overnight** job. Smoke first: add `--limit 20 --only multi-intent`.
- Watch the `kept by category` scorecard at the end — low-yield categories (correction, multiday) are
  where the teacher struggles; if a category's kept% is poor, audit `--show-rejects`.
- **Verify no leakage** before training: `python finetune/check_leakage.py finetune/data/dataset.jsonl`
  (exit 0 = clean).

## Step 3 — Train (WSL, RTX 5080)

Env (once): CUDA-12.8 PyTorch FIRST, then `pip install -r finetune/requirements.txt` (see the file's
header — Blackwell/sm_120 needs cu128). Then:

```bash
~/ftvenv/bin/python finetune/train.py --data finetune/data/dataset.jsonl --epochs 2 --gguf
```

Default base = `katanemo/Arch-Function-3B` (format-hardened). `--gguf` exports Q4_K_M. Rank 16 / lr 2e-4 /
grad-accum 8 fit the 5080. The full **datagen→train→convert→quantize→eval** chain is scripted in
`finetune/overnight.sh` (adjust the teacher flags in it to the 30B).

## Step 4 — Evaluate honestly (ship-gate)

Run BOTH batteries against the new GGUF on a llama-server and compare per-category:

```bash
# clean held-out battery
PUSHIN_LLM_URL=http://127.0.0.1:8080 cargo test --test llm_eval llm_eval -- --ignored --nocapture
# real-world battery (rambling/overnight/ambiguous — matches how the user actually types)
PUSHIN_LLM_URL=http://127.0.0.1:8080 cargo test --test real_world_eval -- --ignored --nocapture
```

**Ship gate (never regress):** register the new GGUF in `model_manager::MODELS` only if the real-world
battery improves AND `llm_eval` holds ≥ baseline per-category across 3 runs. Otherwise keep the current
tuned 7B + the deterministic guards. Prior lesson (memory `tuned-model-eval-ceiling`): guards on a good
base beat marginal retunes — so the bar for shipping a retune is a clear real-world win, not a lateral move.

## Notes / gotchas
- rtk compresses `--nocapture`; run the built `target/debug/deps/<test>-*.exe` directly for raw eval dumps.
- WSL 7B/30B fp16 merges can OOM the WSL VM (default ~50% host RAM) → `~/.wslconfig` `memory=26GB` + `wsl --shutdown`.
- Keep the student (:8080) and teacher (:8082) on separate ports if serving both.
