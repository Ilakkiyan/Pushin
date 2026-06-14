import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Keeps the Playwright mock bridge (tests/e2e/_mockBridge.ts) honest: its handler names must all be
// real registered commands (no typos), and it must implement at least the commands the app calls on
// boot + the headline flows (so E2E doesn't silently get `null` where it expects data).
const bridgeSrc = readFileSync(resolve("tests/e2e/_mockBridge.ts"), "utf8");
const libSrc = readFileSync(resolve("src-tauri/src/lib.rs"), "utf8");

const registered = new Set<string>();
for (const m of libSrc.match(/generate_handler!\[([\s\S]*?)\]/)![1].matchAll(/commands::([a-z0-9_]+)/g)) {
  registered.add(m[1]);
}

const handlerBlock = bridgeSrc.slice(bridgeSrc.indexOf("const handlers"), bridgeSrc.indexOf("(window as any)"));
const handlerNames = new Set<string>();
for (const m of handlerBlock.matchAll(/^\s*([a-z0-9_]+):\s*\(/gm)) handlerNames.add(m[1]);

describe("E2E mock bridge contract", () => {
  it("every bridge handler is a real registered command (no typos)", () => {
    const bogus = [...handlerNames].filter((h) => !registered.has(h));
    expect(bogus, `bridge implements commands that don't exist: ${bogus.join(", ")}`).toEqual([]);
  });

  it("implements the commands needed to boot + drive the headline flows", () => {
    const required = [
      "load_all", "llm_status", "list_pages", "list_inbox", "ensure_inference", "ensure_embeddings",
      "create_page", "get_page", "update_page", "capture_note", "daily_note", "search_pages", "vault_ask",
    ];
    const missing = required.filter((c) => !handlerNames.has(c));
    expect(missing, `bridge is missing handlers for: ${missing.join(", ")}`).toEqual([]);
  });
});
