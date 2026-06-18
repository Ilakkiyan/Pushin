import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// jsdom doesn't implement these; components/editors may touch them.
if (!window.matchMedia) {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
}
if (!window.requestAnimationFrame) {
  window.requestAnimationFrame = (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0) as unknown as number;
}
// jsdom doesn't implement scrollTo (ChatPane auto-scrolls its transcript).
if (!Element.prototype.scrollTo) {
  Element.prototype.scrollTo = vi.fn();
}

// The Tauri window API isn't present outside the webview — give TitleBar a controllable stub.
// Exposed on globalThis so tests can assert calls.
const tauriWindow = {
  isMaximized: vi.fn().mockResolvedValue(false),
  isFullscreen: vi.fn().mockResolvedValue(false),
  onResized: vi.fn().mockResolvedValue(() => {}),
  minimize: vi.fn().mockResolvedValue(undefined),
  toggleMaximize: vi.fn().mockResolvedValue(undefined),
  close: vi.fn().mockResolvedValue(undefined),
  setFullscreen: vi.fn().mockResolvedValue(undefined),
};
(globalThis as Record<string, unknown>).__tauriWindow = tauriWindow;

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => tauriWindow,
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));

// Event bus isn't present outside the webview — stub listen/emit so components that subscribe
// (App's sync-applied refresh, InferenceSetup's progress) don't blow up under jsdom.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  once: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
}));

// Event bus isn't present outside the webview — stub listen/emit so components that subscribe
// (App's sync-applied refresh, InferenceSetup's progress) don't blow up under jsdom.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  once: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
}));
