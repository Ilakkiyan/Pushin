#!/usr/bin/env python3
"""Path B — GRPO with a VERIFIABLE reward (the Rust eval checks), so the model optimizes toward
producing the right calendar rather than imitating a capped teacher. Can exceed the SFT ceiling.

Reward: a persistent `grpo.exe --serve` subprocess scores each completion by running it through the
real apply_recovery + store_plan + the template's check (0.0 bad / 0.2 valid-JSON / 1.0 check-pass).

    python finetune/train_grpo.py --data finetune/data/grpo_prompts.jsonl
    python finetune/train_grpo.py --data finetune/data/grpo_prompts.jsonl --steps 4 --num-gen 4   # smoke
"""
import argparse
import json
import os
import subprocess
import threading

# (GRPO optimizes token log-probs via policy gradient — unlike SFT it does NOT need full logits, so
# UNSLOTH_RETURN_LOGITS is left off to avoid OOM across the generation group.)

from datasets import load_dataset
from unsloth import FastLanguageModel
from trl import GRPOConfig, GRPOTrainer

REWARD_EXE = "/mnt/c/Users/waves/Documents/Projects/Pushin/src-tauri/target/debug/examples/grpo.exe"


class Rewarder:
    """Persistent line-protocol bridge to the Rust verifiable-reward scorer."""

    def __init__(self):
        self.p = subprocess.Popen([REWARD_EXE, "--serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.lock = threading.Lock()

    def reward(self, idx, completion):
        with self.lock:
            self.p.stdin.write(json.dumps({"idx": int(idx), "completion": completion}) + "\n")
            self.p.stdin.flush()
            line = self.p.stdout.readline()
        try:
            return float(line.strip())
        except ValueError:
            return 0.0


REWARDER = Rewarder()


def _text(c):
    # trl conversational completions are [{"role":"assistant","content":...}]; sometimes plain str.
    if isinstance(c, list):
        return c[-1].get("content", "") if c else ""
    if isinstance(c, dict):
        return c.get("content", "")
    return str(c)


def reward_func(prompts, completions, **kwargs):
    idxs = kwargs["idx"]
    return [REWARDER.reward(idxs[i], _text(c)) for i, c in enumerate(completions)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="finetune/data/grpo_prompts.jsonl")
    ap.add_argument("--model", default="unsloth/Qwen2.5-3B-Instruct-bnb-4bit")
    ap.add_argument("--max-seq", type=int, default=1536)
    ap.add_argument("--steps", type=int, default=400)
    ap.add_argument("--num-gen", type=int, default=8)
    ap.add_argument("--lr", type=float, default=1e-5)
    ap.add_argument("--rank", type=int, default=16)
    ap.add_argument("--adapters", default="finetune/out/grpo_adapters")
    ap.add_argument("--merged", default="finetune/out/grpo_merged")
    ap.add_argument("--no-merge", action="store_true")
    args = ap.parse_args()

    model, tok = FastLanguageModel.from_pretrained(
        model_name=args.model, max_seq_length=args.max_seq, load_in_4bit=True, fast_inference=False, dtype=None
    )
    model = FastLanguageModel.get_peft_model(
        model,
        r=args.rank,
        lora_alpha=args.rank,
        lora_dropout=0.0,
        bias="none",
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj", "gate_proj", "up_proj", "down_proj"],
        use_gradient_checkpointing="unsloth",
        random_state=3407,
    )

    ds = load_dataset("json", data_files=args.data, split="train")
    # GRPO wants a conversational `prompt` (list of messages) and chokes on having BOTH prompt+messages.
    # Overwrite `prompt` with the messages list and drop `messages`; `idx` stays (passed to the reward).
    ds = ds.map(lambda ex: {"prompt": ex["messages"]}, remove_columns=["messages"])
    # GRPO only learns where generations vary in reward. The base already saturates these categories
    # (every sample correct → zero variance → no gradient), so drop them and shuffle the rest so each
    # step sees a hard prompt with headroom.
    saturated = {"single-event", "single-task", "restraint", "ranges"}
    ds = ds.filter(lambda ex: ex["category"] not in saturated)
    ds = ds.shuffle(seed=3407)
    print(f"GRPO dataset: {len(ds)} prompts (saturated categories dropped)")

    cfg = GRPOConfig(
        output_dir="finetune/out/grpo_ck",
        per_device_train_batch_size=args.num_gen,
        gradient_accumulation_steps=1,
        num_generations=args.num_gen,
        max_prompt_length=1024,
        max_completion_length=256,
        learning_rate=args.lr,
        max_steps=args.steps,
        logging_steps=5,
        save_steps=100000,
        temperature=0.9,
        optim="adamw_8bit",
        report_to="none",
    )
    # Same Unsloth↔trl quirk as SFT: a sentinel eos leaks via to_dict — pin the real Qwen eos.
    cfg.eos_token = "<|im_end|>"

    trainer = GRPOTrainer(model=model, processing_class=tok, reward_funcs=[reward_func], args=cfg, train_dataset=ds)
    trainer.train()

    model.save_pretrained(args.adapters)
    tok.save_pretrained(args.adapters)
    print(f"Saved GRPO adapters → {args.adapters}")
    if not args.no_merge:
        model.save_pretrained_merged(args.merged, tok, save_method="merged_16bit")
        print(f"Saved merged 16-bit → {args.merged}")


if __name__ == "__main__":
    main()
