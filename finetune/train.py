#!/usr/bin/env python3
"""Pushin parser fine-tune — Stage 5 of finetune/PLAN.md (QLoRA SFT on Qwen2.5-3B).

Trains the small default model to emit Pushin's schema JSON directly from the single-call union
format the datagen labels use, so training and inference match.

Uses a **plain manual training loop** rather than trl's SFTTrainer: on this bleeding-edge stack
(Unsloth 2026.6 + transformers 5 + trl 0.23/0.24) trl's trainer + Unsloth's trainer patches don't
mesh — the run executes but gradients don't flow (grad_norm 0, flat loss). A direct forward/backward
on the Unsloth model trains correctly, so we wrap that in a small loop with completion-only masking.

Designed for one consumer GPU (RTX 5080, 16GB). 3B QLoRA fits comfortably.

    python finetune/train.py --data finetune/data/dataset.jsonl
    python finetune/train.py --data finetune/data/dataset.jsonl --gguf   # also export Q4_K_M GGUF
"""
import argparse
import math
import os
import random

# Unsloth omits logits by default; we compute loss from labels via the model, which returns it
# either way, but keep this on to match the validated path. Set before importing unsloth.
os.environ.setdefault("UNSLOTH_RETURN_LOGITS", "1")

import torch
from datasets import load_dataset
from unsloth import FastLanguageModel
import bitsandbytes as bnb
from transformers import get_linear_schedule_with_warmup


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="finetune/data/dataset.jsonl", help="ChatML SFT JSONL from datagen")
    ap.add_argument("--model", default="unsloth/Qwen2.5-3B-Instruct-bnb-4bit", help="4-bit base to specialize")
    ap.add_argument("--max-seq", type=int, default=4096, help="must cover system prompt + calendar + turn")
    ap.add_argument("--epochs", type=int, default=3)
    ap.add_argument("--max-steps", type=int, default=0, help=">0 caps optimizer steps (smoke); skips merge/export")
    ap.add_argument("--lr", type=float, default=2e-4)
    ap.add_argument("--batch", type=int, default=1)
    ap.add_argument("--grad-accum", type=int, default=8)
    ap.add_argument("--rank", type=int, default=16, help="LoRA rank (and alpha)")
    ap.add_argument("--adapters", default="finetune/out/adapters", help="where to save LoRA adapters")
    ap.add_argument("--merged", default="finetune/out/merged", help="where to save the merged 16-bit model")
    ap.add_argument("--gguf", action="store_true", help="also export a Q4_K_M GGUF for Pushin")
    args = ap.parse_args()

    # --- base model (4-bit) + LoRA adapters ---
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=args.model, max_seq_length=args.max_seq, load_in_4bit=True, dtype=None
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
    pad_id = tokenizer.pad_token_id if tokenizer.pad_token_id is not None else tokenizer.eos_token_id

    # --- dataset: tokenize each row, mask the prompt so loss is on the assistant JSON only ---
    raw = load_dataset("json", data_files=args.data, split="train")

    def encode(ex):
        msgs = ex["messages"]
        full = tokenizer.apply_chat_template(msgs, tokenize=False, add_generation_prompt=False)
        prompt = tokenizer.apply_chat_template(msgs[:-1], tokenize=False, add_generation_prompt=True)
        ids = tokenizer(full, add_special_tokens=False)["input_ids"][: args.max_seq]
        p_len = len(tokenizer(prompt, add_special_tokens=False)["input_ids"])
        labels = list(ids)
        for i in range(min(p_len, len(labels))):
            labels[i] = -100  # mask the prompt; train only on the completion
        return {"input_ids": ids, "labels": labels}

    data = [encode(ex) for ex in raw]
    print(f"Loaded {len(data)} training examples from {args.data}")

    def make_batches():
        order = list(range(len(data)))
        random.shuffle(order)
        for i in range(0, len(order), args.batch):
            chunk = [data[j] for j in order[i : i + args.batch]]
            maxlen = max(len(c["input_ids"]) for c in chunk)
            input_ids, labels, attn = [], [], []
            for c in chunk:
                pad = maxlen - len(c["input_ids"])
                input_ids.append(c["input_ids"] + [pad_id] * pad)
                labels.append(c["labels"] + [-100] * pad)
                attn.append([1] * len(c["input_ids"]) + [0] * pad)
            yield (
                torch.tensor(input_ids, device="cuda"),
                torch.tensor(labels, device="cuda"),
                torch.tensor(attn, device="cuda"),
            )

    # --- optimizer + schedule ---
    params = [p for p in model.parameters() if p.requires_grad]
    trainable = sum(p.numel() for p in params)
    print(f"Trainable params: {trainable/1e6:.1f}M")
    opt = bnb.optim.AdamW8bit(params, lr=args.lr, weight_decay=0.01)
    batches_per_epoch = math.ceil(len(data) / args.batch)
    opt_steps_per_epoch = math.ceil(batches_per_epoch / args.grad_accum)
    total_opt_steps = args.max_steps if args.max_steps > 0 else opt_steps_per_epoch * args.epochs
    sched = get_linear_schedule_with_warmup(opt, num_warmup_steps=5, num_training_steps=total_opt_steps)

    FastLanguageModel.for_training(model)
    model.train()
    step, micro = 0, 0
    opt.zero_grad()
    print(f"Training: {args.epochs} epochs, ~{total_opt_steps} optimizer steps (effective batch {args.batch*args.grad_accum})")
    done = False
    for epoch in range(args.epochs):
        if done:
            break
        for input_ids, labels, attn in make_batches():
            out = model(input_ids=input_ids, attention_mask=attn, labels=labels)
            loss = out.loss / args.grad_accum
            loss.backward()
            micro += 1
            if micro % args.grad_accum == 0:
                gnorm = torch.nn.utils.clip_grad_norm_(params, 1.0)
                opt.step()
                sched.step()
                opt.zero_grad()
                step += 1
                if step % 10 == 0 or step == 1:
                    print(f"  step {step}/{total_opt_steps}  loss {out.loss.item():.4f}  grad_norm {gnorm:.3f}  lr {sched.get_last_lr()[0]:.2e}")
                if args.max_steps > 0 and step >= args.max_steps:
                    done = True
                    break

    model.save_pretrained(args.adapters)
    tokenizer.save_pretrained(args.adapters)
    print(f"Saved LoRA adapters → {args.adapters}")

    if args.max_steps > 0:
        print("Smoke run (--max-steps) — skipping merge/export.")
        return

    model.save_pretrained_merged(args.merged, tokenizer, save_method="merged_16bit")
    print(f"Saved merged 16-bit model → {args.merged}")

    if args.gguf:
        model.save_pretrained_gguf(args.merged, tokenizer, quantization_method="q4_k_m")
        print(f"Wrote Q4_K_M GGUF under {args.merged} — copy into Pushin's models dir.")


if __name__ == "__main__":
    main()
