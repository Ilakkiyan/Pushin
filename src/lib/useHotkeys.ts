import { useEffect } from "react";
import { useStore } from "../state/store";

type View = ReturnType<typeof useStore.getState>["view"];

// Linear-style "g then key" navigation. Press `g`, then a letter within ~1.2s to jump to a view.
// Keep this in sync with the help shown in the ⌘K palette (HOTKEYS below is exported for it).
const NAV: Record<string, View> = {
  c: "calendar",
  t: "calendar", // tasks live in the calendar aside
  v: "vault",
  p: "projects",
  h: "habits",
  i: "inbox",
  g: "graph",
  e: "people",
  b: "booking",
  l: "label",
  s: "settings",
};

/** The `g`-leader navigation map as display pairs, for the palette/help. */
export const NAV_HOTKEYS: { combo: string; label: string }[] = [
  { combo: "g c", label: "Calendar" },
  { combo: "g v", label: "Vault" },
  { combo: "g p", label: "Projects" },
  { combo: "g h", label: "Habits" },
  { combo: "g i", label: "Inbox" },
  { combo: "g g", label: "Graph" },
  { combo: "g e", label: "People" },
  { combo: "g b", label: "Booking" },
  { combo: "g s", label: "Settings" },
];

/** True when focus is in a text field — we never hijack keys the user is typing into. */
function isTypingTarget(el: EventTarget | null): boolean {
  const node = el as HTMLElement | null;
  if (!node || !node.tagName) return false;
  const tag = node.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || node.isContentEditable || node.getAttribute("role") === "textbox";
}

/** Install the global navigation hotkeys (mount once, in App). Modifier combos (⌘K, ⌘⇧N, Ctrl+T) are
 *  left to their own handlers — this only owns the bare `g`-leader. */
export function useHotkeys() {
  useEffect(() => {
    let leaderUntil = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return; // not ours
      if (isTypingTarget(document.activeElement)) return;
      const key = e.key.toLowerCase();
      const now = Date.now();

      if (now < leaderUntil) {
        leaderUntil = 0;
        const view = NAV[key];
        if (view) {
          e.preventDefault();
          useStore.getState().setView(view);
        }
        return;
      }
      if (key === "g") {
        leaderUntil = now + 1200; // arm the leader window
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
