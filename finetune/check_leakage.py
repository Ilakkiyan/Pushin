#!/usr/bin/env python3
"""Anti-leakage tripwire (docs/notes/GUARDS_TO_99_PLAN.md A1).

Fails (exit 1) if any held-out battery prompt in `src-tauri/tests/llm_eval.rs` appears (normalized) as
a user message in a training dataset. The datagen denylist prevents this at generation time; this is the
belt-and-suspenders CI check for any dataset on disk.

    python finetune/check_leakage.py finetune/data/dataset.jsonl [more.jsonl ...]

Run from the project root. Exit 0 = clean, 1 = leakage found (prints the offending prompts).
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", " ", s.lower()).strip()


def eval_prompts() -> set[str]:
    src = (ROOT / "src-tauri/tests/llm_eval.rs").read_text(encoding="utf-8", errors="ignore")
    raw = re.findall(r'prompt:\s*"((?:[^"\\]|\\.)*)"', src)
    return {norm(p.encode().decode("unicode_escape")) for p in raw}


def train_user_msgs(paths) -> set[str]:
    out = set()
    for p in paths:
        fp = pathlib.Path(p)
        if not fp.exists():
            print(f"  (skip, not found: {p})")
            continue
        for line in fp.open(encoding="utf-8", errors="ignore"):
            try:
                for m in json.loads(line)["messages"]:
                    if m.get("role") == "user":
                        out.add(norm(m["content"]))
            except Exception:
                pass
    return out


def main() -> int:
    files = sys.argv[1:] or ["finetune/data/dataset.jsonl"]
    evals = eval_prompts()
    train = train_user_msgs(files)
    leaked = sorted(e for e in evals if e in train)
    print(f"eval prompts: {len(evals)} | training user-msgs: {len(train)} | files: {', '.join(files)}")
    if leaked:
        print(f"FAIL - LEAKAGE: {len(leaked)} battery prompt(s) found in training data:")
        for e in leaked:
            print("   " + e.encode("ascii", "replace").decode("ascii"))
        return 1
    print("OK - no leakage; the battery is held out.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
