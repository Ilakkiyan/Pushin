import { useEffect, useState } from "react";

/**
 * True when the viewport is phone-sized. Reactive to resize, so it also flips when a desktop window
 * is narrowed (and on a real phone it's just always true). jsdom-safe: `matchMedia` is mocked in the
 * test setup to report `false`, so component tests keep exercising the desktop layout.
 */
export function useIsMobile(breakpointPx = 768): boolean {
  const query = `(max-width: ${breakpointPx - 1}px)`;
  const read = () =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(query).matches
      : false;
  const [isMobile, setIsMobile] = useState(read);

  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(query);
    const onChange = () => setIsMobile(mq.matches);
    onChange();
    mq.addEventListener?.("change", onChange);
    return () => mq.removeEventListener?.("change", onChange);
  }, [query]);

  return isMobile;
}
