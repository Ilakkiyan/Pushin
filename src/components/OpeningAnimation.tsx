import { useCallback, useEffect, useRef, useState } from "react";

/**
 * The opening animation: the wide PUSHIN wordmark fades/letter-spacing-settles in on a slate field,
 * holds briefly, then fades out to reveal the app. Kept short (~1.8s) and **skippable** — any key or
 * click jumps straight to the app, so a fast user is never held up.
 *
 * Control via `?splash=`:
 *   - `logo`  → freeze on the settled wordmark (for screenshots), never advance.
 *   - `off`   → skip entirely (used for inner-app captures).
 * Skipped automatically under unit tests (`import.meta.env.MODE === "test"`).
 */
export default function OpeningAnimation({ onDone }: { onDone: () => void }) {
  const splash = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("splash") : null;
  const skip = import.meta.env.MODE === "test" || splash === "off";
  const frozen = splash === "logo";
  const [out, setOut] = useState(false);
  const doneRef = useRef(onDone);
  doneRef.current = onDone;
  const finished = useRef(false);

  const beginExit = useCallback(() => {
    if (finished.current) return;
    setOut(true);
    window.setTimeout(() => {
      if (finished.current) return;
      finished.current = true;
      doneRef.current();
    }, 460); // let the fade-out finish before unmounting
  }, []);

  useEffect(() => {
    if (skip) {
      onDone();
      return;
    }
    if (frozen) return;
    const hold = window.setTimeout(beginExit, 1350);
    const onKey = () => beginExit();
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(hold);
      window.removeEventListener("keydown", onKey);
    };
  }, [skip, frozen, onDone, beginExit]);

  if (skip) return null;

  return (
    <div className={`splash${out ? " splash--out" : ""}`} aria-hidden onClick={frozen ? undefined : beginExit}>
      <div className="wordmark splash__mark">Pushin</div>
    </div>
  );
}
