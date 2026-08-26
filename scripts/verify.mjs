#!/usr/bin/env node
// One entry point for every Pushin test suite, in tiers.
//
//   npm run verify           # full  — Rust unit + Vitest + build/tsc + Playwright E2E
//   npm run verify:fast      # fast  — Rust unit + Vitest only (the edit loop)
//   npm run verify:live      # live  — full + the model evals (needs a llama-server on :8080)
//
// WHY A RUNNER AND NOT ONE MERGED SUITE: these layers have incompatible requirements. The Rust unit
// tests are ~30s and deterministic; the model evals need a GPU, a 4.7GB model, and several minutes,
// and their scores legitimately bounce run-to-run (CLAUDE.md gotcha #1). Merging them would destroy
// the fast feedback loop and make CI — which has no model — unable to run the suite at all. So the
// tiers stay separate processes and this script just orchestrates them and reports once.
//
// Deterministic tiers GATE (a failure fails the run). Live model evals are REPORTED, never gating:
// a bouncing score must not turn into a red build. Use --gate-live to opt into thresholds.

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const OUT_DIR = join("target", "verify");

/** Summarise a "N passed / M failed" style tail. Always surfaces failures — a summary that prints
 *  only the pass count reads as success on a failing run, which is how "9 passed ✗" happened. */
function tally(out, passRe, failRe) {
  const p = out.match(passRe)?.[1];
  const f = out.match(failRe)?.[1];
  if (p == null && f == null) return null;
  const parts = [];
  if (p != null) parts.push(`${p} passed`);
  if (f != null && Number(f) > 0) parts.push(`${f} FAILED`);
  return parts.join(", ");
}

/** What to show for a FAILING step. `parse` describes the happy path (the build one just says
 *  "clean"), so trusting it on a failure prints "clean ✗" — a row that argues with its own verdict.
 *  Prefer a parse result that already names failures, else the exit code plus the first real error
 *  line, so the summary alone is enough to tell whether the failure is yours. */
function failureDetail(out, code, parsed) {
  if (parsed && /FAILED|failed/.test(parsed)) return `${parsed} · exit ${code}`;
  const line = out
    .split("\n")
    .map((l) => l.trim())
    .find((l) => /^(error|Error|error TS|✘|FAIL|panicked)/.test(l) || /error TS\d+/.test(l));
  return line ? `exit ${code} — ${line.slice(0, 120)}` : `exit ${code}`;
}

/** @typedef {{name:string, label:string, cmd:string, tier:"fast"|"full"|"live", live?:boolean, retryOnce?:boolean, parse?:(o:string)=>string|null}} Step */

/** @type {Step[]} */
const STEPS = [
  {
    name: "rust-unit",
    label: "Rust unit (cargo test --lib)",
    cmd: "cargo test --manifest-path src-tauri/Cargo.toml --lib",
    tier: "fast",
    parse: (o) => tally(o, /(\d+) passed/, /(\d+) failed/),
  },
  {
    name: "vitest",
    label: "Frontend unit (Vitest)",
    cmd: "npx vitest run",
    tier: "fast",
    parse: (o) => tally(o, /Tests\s+(\d+) passed/, /Tests\s+(?:\d+ failed \| )?(\d+) failed/),
  },
  {
    name: "build",
    label: "Type-check + production build",
    cmd: "npm run build",
    tier: "full",
    // Flaky in a shared working tree: `tsc` reading a file another session is mid-write fails the
    // whole step. Retry once so a genuine break still fails but a transient one surfaces as "~".
    retryOnce: true,
    parse: (o) => (o.match(/built in ([\d.]+m?s)/) ?? [])[1] ? `clean (${o.match(/built in ([\d.]+m?s)/)[1]})` : "clean",
  },
  {
    name: "e2e",
    label: "Playwright E2E (real app)",
    cmd: "npx playwright test",
    tier: "full",
    // The suite has shown an unexplained all-tests-fail flake under load; retry once so a flake is
    // visible in the report rather than either hidden or fatal. See docs/notes/DEVLOG.md.
    retryOnce: true,
    parse: (o) => tally(o, /(\d+) passed/, /(\d+) failed/),
  },
  {
    name: "llm_eval",
    label: "Model eval — per-category scorecard",
    cmd: "cargo test --manifest-path src-tauri/Cargo.toml --test llm_eval -- --ignored --nocapture",
    tier: "live",
    live: true,
    parse: (o) => {
      const m = o.match(/TOTAL:\s+(\d+)\/(\d+) checks\s+\((\d+)%\)/);
      return m ? `${m[1]}/${m[2]} (${m[3]}%)` : null;
    },
  },
  {
    name: "model_battery",
    label: "Model battery — UI-projected pre-push gate",
    cmd: "cargo test --manifest-path src-tauri/Cargo.toml --test model_battery -- --ignored --nocapture",
    tier: "live",
    live: true,
    parse: (o) => {
      const m = o.match(/TOTAL \(crisp checks\):\s+(\d+)\/(\d+)\s+\((\d+)%\)/);
      return m ? `${m[1]}/${m[2]} (${m[3]}%)` : null;
    },
  },
  {
    name: "real_world_eval",
    label: "Real-world battery — stored-calendar scoring",
    cmd: "cargo test --manifest-path src-tauri/Cargo.toml --test real_world_eval -- --ignored --nocapture",
    tier: "live",
    live: true,
    parse: (o) => {
      const m = o.match(/REAL-WORLD TOTAL:\s+(\d+)\/(\d+)/);
      return m ? `${m[1]}/${m[2]}` : null;
    },
  },
];

const TIER_ORDER = { fast: 0, full: 1, live: 2 };

function parseArgs(argv) {
  const tier = argv.includes("--live") ? "live" : argv.includes("--fast") ? "fast" : "full";
  return { tier, gateLive: argv.includes("--gate-live"), only: (argv.find((a) => a.startsWith("--only=")) ?? "").slice(7) };
}

function run(cmd) {
  return new Promise((resolve) => {
    const started = Date.now();
    // shell:true so `npx`/`npm` resolve their shims on Windows. Output is captured here rather than
    // inherited, which also sidesteps the rtk wrapper compressing --nocapture output to nothing.
    const child = spawn(cmd, { shell: true, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (d) => {
      out += d;
      process.stdout.write(".");
    });
    child.stderr.on("data", (d) => {
      out += d;
    });
    child.on("close", (code) => resolve({ code: code ?? 1, out, ms: Date.now() - started }));
    child.on("error", (e) => resolve({ code: 1, out: out + String(e), ms: Date.now() - started }));
  });
}

/** Is a llama-server answering? Live evals self-skip without one; we check first so we can say so. */
async function serverUp() {
  try {
    const c = new AbortController();
    const t = setTimeout(() => c.abort(), 3000);
    const r = await fetch("http://127.0.0.1:8080/health", { signal: c.signal });
    clearTimeout(t);
    return r.ok;
  } catch {
    return false;
  }
}

const fmt = (ms) => (ms >= 60000 ? `${(ms / 60000).toFixed(1)}m` : `${(ms / 1000).toFixed(1)}s`);

async function main() {
  const { tier, gateLive, only } = parseArgs(process.argv.slice(2));
  mkdirSync(OUT_DIR, { recursive: true });

  let steps = STEPS.filter((s) => TIER_ORDER[s.tier] <= TIER_ORDER[tier]);
  if (only) steps = steps.filter((s) => s.name === only);

  const liveWanted = steps.some((s) => s.live);
  const haveServer = liveWanted ? await serverUp() : false;
  if (liveWanted && !haveServer) {
    console.log("\n⚠  No llama-server on :8080 — the live model evals will be SKIPPED.");
    console.log("   Open Pushin (or start a server) and re-run to include them.\n");
  }

  console.log(`Pushin verify — tier: ${tier}${gateLive ? " (live gating ON)" : ""}\n`);

  const results = [];
  for (const step of steps) {
    if (step.live && !haveServer) {
      results.push({ step, status: "skip", detail: "no server on :8080", ms: 0 });
      console.log(`○ ${step.label} — skipped`);
      continue;
    }
    process.stdout.write(`▶ ${step.label} `);
    let r = await run(step.cmd);
    let retried = false;
    if (r.code !== 0 && step.retryOnce) {
      process.stdout.write(" retry ");
      retried = true;
      r = await run(step.cmd);
    }
    writeFileSync(join(OUT_DIR, `${step.name}.log`), r.out);

    const parsed = step.parse?.(r.out) ?? null;
    const summary = r.code === 0 ? (parsed ?? "ok") : failureDetail(r.out, r.code, parsed);
    // A live eval that ran is reported on its score; only a crashed process is a failure, and even
    // then it never gates unless --gate-live. Scores bounce run-to-run; a red build must mean a bug.
    const failed = r.code !== 0 && (!step.live || gateLive);
    results.push({
      step,
      status: failed ? "fail" : r.code !== 0 ? "warn" : retried ? "flaky" : "pass",
      detail: summary,
      ms: r.ms,
    });
    const mark = failed ? "✗" : retried ? "~" : r.code !== 0 ? "!" : "✓";
    console.log(`\r${mark} ${step.label} — ${summary ?? (r.code === 0 ? "ok" : `exit ${r.code}`)} (${fmt(r.ms)})`);
  }

  const pad = Math.max(...results.map((r) => r.step.label.length));
  const icon = { pass: "✓", fail: "✗", skip: "○", flaky: "~", warn: "!" };
  const lines = results.map((r) => `${icon[r.status]}  ${r.step.label.padEnd(pad)}  ${r.detail}  (${fmt(r.ms)})`);

  const failures = results.filter((r) => r.status === "fail");
  const flaky = results.filter((r) => r.status === "flaky");
  const total = results.reduce((a, r) => a + r.ms, 0);

  const report = [
    `# Pushin verify — ${tier}`,
    "",
    `Run at ${new Date().toISOString()} · total ${fmt(total)}`,
    "",
    "```",
    ...lines,
    "```",
    "",
    failures.length ? `**${failures.length} failing:** ${failures.map((f) => f.step.name).join(", ")}` : "**All gating suites passed.**",
    flaky.length ? `\n⚠ Passed only on retry (flake): ${flaky.map((f) => f.step.name).join(", ")}` : "",
    "",
    `Per-suite logs: \`${OUT_DIR}/<name>.log\``,
    "",
  ].join("\n");
  writeFileSync(join(OUT_DIR, "report.md"), report);

  console.log(`\n${"─".repeat(pad + 24)}`);
  lines.forEach((l) => console.log(l));
  console.log("─".repeat(pad + 24));
  console.log(`total ${fmt(total)} · report ${join(OUT_DIR, "report.md")}`);
  if (flaky.length) console.log(`⚠ passed only on retry: ${flaky.map((f) => f.step.name).join(", ")}`);
  if (failures.length) {
    console.log(`\n✗ ${failures.length} failing: ${failures.map((f) => f.step.name).join(", ")}`);
    process.exit(1);
  }
  console.log("\n✓ all gating suites passed");
}

main();
