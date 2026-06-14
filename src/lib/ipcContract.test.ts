import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Contract test: every `invoke<...>("cmd")` in ipc.ts must target a command actually registered in
// the Rust `generate_handler![]`. Catches renames/typos/removals that the type system can't (the
// command name is a plain string on both sides). cwd is the project root under Vitest.
const ipcSrc = readFileSync(resolve("src/lib/ipc.ts"), "utf8");
const libSrc = readFileSync(resolve("src-tauri/src/lib.rs"), "utf8");

function invokedCommands(): string[] {
  const names = new Set<string>();
  for (const m of ipcSrc.matchAll(/invoke<[^>]*>\(\s*"([a-z0-9_]+)"/g)) names.add(m[1]);
  return [...names];
}

function registeredCommands(): Set<string> {
  const block = libSrc.match(/generate_handler!\[([\s\S]*?)\]/);
  expect(block, "generate_handler![] block found in lib.rs").toBeTruthy();
  const names = new Set<string>();
  for (const m of block![1].matchAll(/commands::([a-z0-9_]+)/g)) names.add(m[1]);
  return names;
}

describe("IPC contract (ipc.ts ⇄ lib.rs)", () => {
  const invoked = invokedCommands();
  const registered = registeredCommands();

  it("finds a healthy number of commands on both sides", () => {
    expect(invoked.length).toBeGreaterThan(40);
    expect(registered.size).toBeGreaterThan(40);
  });

  it("every invoked command is registered in generate_handler!", () => {
    const orphans = invoked.filter((c) => !registered.has(c));
    expect(orphans, `ipc.ts calls commands not registered in lib.rs: ${orphans.join(", ")}`).toEqual([]);
  });

  it("registered commands not used by ipc.ts are only the known internal ones", () => {
    // Informational guard: anything here is registered but never called from ipc.ts. Allowed set is
    // empty today; if a command is intentionally invoked elsewhere, add it here so drift still fails.
    const allowedUnused: string[] = [];
    const unused = [...registered].filter((c) => !invoked.includes(c) && !allowedUnused.includes(c));
    expect(unused, `registered but never invoked from ipc.ts: ${unused.join(", ")}`).toEqual([]);
  });
});
