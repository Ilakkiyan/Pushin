import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { fireEvent } from "@testing-library/dom";
import { useHotkeys, NAV_HOTKEYS } from "./useHotkeys";
import { useStore } from "../state/store";

// The `g`-leader is a BARE-key handler on window, which makes it the one piece of UI that can eat a
// keystroke anywhere in the app. Both halves matter: it must navigate when it should, and it must
// stay out of the way when the user is typing.

const view = () => useStore.getState().view;

function press(key: string, init: KeyboardEventInit = {}) {
  fireEvent.keyDown(window, { key, ...init });
}

beforeEach(() => {
  useStore.setState({ view: "today" } as never);
  vi.useRealTimers();
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("useHotkeys — g-leader navigation", () => {
  it("jumps to a view on g then a mapped key", () => {
    renderHook(() => useHotkeys());
    press("g");
    press("c");
    expect(view()).toBe("calendar");
  });

  it("supports g g for the graph", () => {
    // The leader key doubling as a target is the easy one to break.
    renderHook(() => useHotkeys());
    press("g");
    press("g");
    expect(view()).toBe("graph");
  });

  it("does nothing for a bare key with no leader", () => {
    renderHook(() => useHotkeys());
    press("c");
    expect(view()).toBe("today");
  });

  it("disarms after one use — g c c does not fire twice", () => {
    renderHook(() => useHotkeys());
    press("g");
    press("c");
    useStore.setState({ view: "today" } as never);
    press("c");
    expect(view()).toBe("today");
  });

  it("an unmapped key after g cancels the leader instead of arming it", () => {
    renderHook(() => useHotkeys());
    press("g");
    press("z"); // no such target
    press("c"); // must NOT navigate — the leader was consumed
    expect(view()).toBe("today");
  });

  it("ignores the leader once the window has expired", () => {
    vi.useFakeTimers();
    const now = Date.now();
    vi.setSystemTime(now);
    renderHook(() => useHotkeys());
    press("g");
    vi.setSystemTime(now + 2000); // past the ~1.2s window
    press("c");
    expect(view()).toBe("today");
  });

  it("never hijacks a modifier combo", () => {
    // ⌘K, Ctrl+T and friends belong to their own handlers; swallowing them here would break the
    // command palette and the browser/OS shortcuts alike.
    renderHook(() => useHotkeys());
    for (const mod of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }]) {
      press("g", mod);
      press("c", mod);
    }
    expect(view()).toBe("today");
  });
});

describe("useHotkeys — staying out of the way while typing", () => {
  function focusIn(tag: "input" | "textarea"): HTMLElement {
    const el = document.createElement(tag);
    document.body.appendChild(el);
    el.focus();
    return el;
  }

  it("does not navigate while an input has focus", () => {
    // Typing "again" into the quick-capture box must not fling you to the Graph on the g.
    renderHook(() => useHotkeys());
    focusIn("input");
    press("g");
    press("c");
    expect(view()).toBe("today");
  });

  it("does not navigate while a textarea has focus", () => {
    renderHook(() => useHotkeys());
    focusIn("textarea");
    press("g");
    press("v");
    expect(view()).toBe("today");
  });

  it("does not navigate inside a contenteditable (the vault editor)", () => {
    renderHook(() => useHotkeys());
    const el = document.createElement("div");
    el.setAttribute("contenteditable", "true");
    // jsdom does not derive isContentEditable from the attribute, so set it the way the DOM would.
    Object.defineProperty(el, "isContentEditable", { value: true });
    el.tabIndex = 0;
    document.body.appendChild(el);
    el.focus();
    press("g");
    press("p");
    expect(view()).toBe("today");
  });

  it("does not navigate inside an element with role=textbox", () => {
    renderHook(() => useHotkeys());
    const el = document.createElement("div");
    el.setAttribute("role", "textbox");
    el.tabIndex = 0;
    document.body.appendChild(el);
    el.focus();
    press("g");
    press("h");
    expect(view()).toBe("today");
  });

  it("resumes working once focus leaves the field", () => {
    renderHook(() => useHotkeys());
    const el = focusIn("input");
    press("g");
    el.blur();
    press("g");
    press("i");
    expect(view()).toBe("inbox");
  });
});

describe("useHotkeys — the help list matches the real bindings", () => {
  it("every documented combo actually navigates somewhere", () => {
    // NAV_HOTKEYS is what the ⌘K palette shows. A combo listed there that does nothing is a lie in
    // the UI; a binding that exists but is undocumented is invisible.
    renderHook(() => useHotkeys());
    for (const { combo } of NAV_HOTKEYS) {
      const [, key] = combo.split(" ");
      // Park on a sentinel no combo can target, so "g t" is distinguishable from doing nothing
      // (every real view IS a target, so there is no safe real value to start from).
      useStore.setState({ view: "__unset__" } as never);
      press("g");
      press(key);
      expect(view(), `"${combo}" is advertised but does not navigate`).not.toBe("__unset__");
    }
  });

  it("documents every binding the handler actually has", () => {
    renderHook(() => useHotkeys());
    const documented = new Set(NAV_HOTKEYS.map(({ combo }) => combo.split(" ")[1]));
    const undocumented: string[] = [];
    for (const key of "abcdefghijklmnopqrstuvwxyz") {
      if (documented.has(key)) continue;
      useStore.setState({ view: "today" } as never);
      press("g");
      press(key);
      if (view() !== "today") undocumented.push(`g ${key} → ${view()}`);
    }
    expect(undocumented, `bindings missing from the ⌘K help: ${undocumented.join(", ")}`).toEqual([]);
  });
});
