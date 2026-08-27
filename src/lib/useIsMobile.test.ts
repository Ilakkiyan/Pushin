import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useIsMobile } from "./useIsMobile";

// This hook decides whether the app renders its desktop shell or `MobileShell`. Getting it wrong is
// not a cosmetic bug — it swaps the entire chrome. It also has to survive environments where
// `matchMedia` is missing entirely (older webviews, SSR-ish contexts, the Tauri Android webview
// during startup), which is where a naive implementation throws before the first paint.

type Listener = () => void;

/** A controllable `matchMedia` that records the query it was asked about. */
function installMatchMedia(initial: boolean) {
  const listeners = new Set<Listener>();
  const state = { matches: initial, queries: [] as string[] };
  // `matches` must be a live GETTER on every returned object — spreading a getter copies its value
  // at spread time, which quietly freezes the fake and makes the resize test pass trivially.
  window.matchMedia = ((q: string) => {
    state.queries.push(q);
    return {
      get matches() {
        return state.matches;
      },
      media: q,
      onchange: null,
      addEventListener: (_: string, cb: Listener) => listeners.add(cb),
      removeEventListener: (_: string, cb: Listener) => listeners.delete(cb),
      addListener: (cb: Listener) => listeners.add(cb),
      removeListener: (cb: Listener) => listeners.delete(cb),
      dispatchEvent: () => true,
    } as unknown as MediaQueryList;
  }) as typeof window.matchMedia;
  return {
    state,
    listeners,
    set(matches: boolean) {
      state.matches = matches;
      listeners.forEach((cb) => cb());
    },
  };
}

const original = window.matchMedia;
afterEach(() => {
  window.matchMedia = original;
});
beforeEach(() => vi.restoreAllMocks());

describe("useIsMobile", () => {
  it("reports the current match on the first render, with no flash of the wrong shell", () => {
    installMatchMedia(true);
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(true);
  });

  it("reports false on a desktop-width viewport", () => {
    installMatchMedia(false);
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);
  });

  it("flips when the window is resized across the breakpoint", () => {
    const mm = installMatchMedia(false);
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);

    act(() => mm.set(true));
    expect(result.current).toBe(true);

    act(() => mm.set(false));
    expect(result.current).toBe(false);
  });

  it("asks about a max-width one pixel below the breakpoint", () => {
    // Off-by-one here means a device exactly at the breakpoint gets the wrong shell.
    const mm = installMatchMedia(false);
    renderHook(() => useIsMobile(768));
    expect(mm.state.queries.some((q) => q === "(max-width: 767px)")).toBe(true);
  });

  it("honours a custom breakpoint", () => {
    const mm = installMatchMedia(false);
    renderHook(() => useIsMobile(1024));
    expect(mm.state.queries.some((q) => q === "(max-width: 1023px)")).toBe(true);
  });

  it("unsubscribes on unmount so a torn-down shell cannot set state", () => {
    const mm = installMatchMedia(false);
    const { unmount } = renderHook(() => useIsMobile());
    expect(mm.listeners.size).toBeGreaterThan(0);
    unmount();
    expect(mm.listeners.size).toBe(0);
  });

  it("falls back to desktop when matchMedia is missing entirely", () => {
    // Some webviews have no matchMedia at first paint. Throwing here would take the whole app down
    // before anything renders, so the hook must degrade to the desktop layout instead.
    // @ts-expect-error deliberately removing the API
    delete window.matchMedia;
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);
  });

  it("survives a matchMedia with no addEventListener (legacy API only)", () => {
    // Safari < 14 and some embedded webviews only implement addListener/removeListener.
    window.matchMedia = ((q: string) =>
      ({ matches: true, media: q, onchange: null }) as unknown as MediaQueryList) as typeof window.matchMedia;
    const { result, unmount } = renderHook(() => useIsMobile());
    expect(result.current).toBe(true);
    expect(() => unmount()).not.toThrow();
  });
});
