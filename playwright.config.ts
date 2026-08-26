import { defineConfig, devices } from "@playwright/test";

// Mocked-IPC E2E: drives the real React app (Vite dev server on :1420) with a faked Tauri bridge
// (tests/e2e/_mockBridge.ts) instead of the Rust process. Browser runs in CI (ubuntu-latest).
export default defineConfig({
  testDir: "./tests/e2e",
  // `_*.spec.ts` are dev-only screenshot utilities (e.g. _capture.spec.ts) — not run in CI.
  testIgnore: "**/_*.spec.ts",
  timeout: 30_000,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["html", { open: "never" }], ["list"]] : "list",
  use: {
    baseURL: "http://localhost:1420",
    trace: "on-first-retry",
  },
  webServer: {
    command: "npm run dev",
    // `url` rather than `port`: the port form resolves as soon as Vite's socket accepts a TCP
    // connection, which is before it can actually serve — so workers could start against a server
    // that then reloaded underneath them. `url` polls with a real HTTP request instead.
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});
