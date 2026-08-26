import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  // Pre-bundle these up front instead of letting Vite discover them on the first request. Discovery
  // ends in a full page reload, which can land mid-boot and break whatever is already running (it is
  // the suspected cause of the Playwright suite failing all-tests-at-once on a cold start).
  //
  // The list is what Vite itself discovers, NOT hand-picked — regenerate it after adding or removing
  // an import, or a missing entry silently reintroduces the reload:
  //   rm -rf node_modules/.vite && npx vite optimize --force
  //   node -p "Object.keys(require('./node_modules/.vite/deps/_metadata.json').optimized).sort()"
  optimizeDeps: {
    include: [
      "@blocknote/core",
      "@blocknote/mantine",
      "@blocknote/react",
      "@tauri-apps/api/app",
      "@tauri-apps/api/core",
      "@tauri-apps/api/event",
      "@tauri-apps/api/window",
      "@tauri-apps/plugin-dialog",
      "@tauri-apps/plugin-opener",
      "@tauri-apps/plugin-process",
      "@tauri-apps/plugin-updater",
      "clsx",
      "lucide-react",
      "react",
      "react-dom",
      "react-dom/client",
      "react-force-graph-2d",
      "react/jsx-dev-runtime",
      "react/jsx-runtime",
      "zustand",
    ],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
