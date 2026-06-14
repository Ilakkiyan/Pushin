import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Test config kept separate from vite.config.ts (which carries Tauri dev-server settings).
export default defineConfig({
  plugins: [react()],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./vitest.setup.ts"],
    // E2E specs live under tests/e2e and run via Playwright, not Vitest.
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    exclude: ["tests/e2e/**", "node_modules/**", "dist/**"],
    coverage: {
      provider: "v8",
      include: ["src/lib/**", "src/state/**", "src/components/**", "src/panes/**"],
      exclude: ["src/**/*.test.*", "src/main.tsx", "src/vite-env.d.ts"],
      reporter: ["text", "html", "lcov"],
    },
  },
});
