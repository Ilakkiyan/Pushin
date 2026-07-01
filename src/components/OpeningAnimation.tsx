import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";

/**
 * The opening animation — which doubles as the loading screen. The wide PUSHIN wordmark settles in on a
 * slate field; once it has settled, the splash HOLDS (showing a spinner) until `ready` (the on-device
 * model is loaded into memory), then fades out to reveal the app. This means the app never flashes
 * before the AI is ready — the model check + loading both live here. Still **skippable** (any key or
 * click jumps straight in).
 *
 * Control via `?splash=`:
 *   - `logo`  → freeze on the settled wordmark (for screenshots), never advance.
 *   - `off`   → skip entirely (used for inner-app captures).
 * Skipped automatically under unit tests (`import.meta.env.MODE === "test"`).
 */
export default function OpeningAnimation({ onDone, ready }: { onDone: () => void; ready: boolean }) {
  const splash = typeof window !== "undefined" ? new URLSearchParams(window.location.search).get("splash") : null;
  const skip = import.meta.env.MODE === "test" || splash === "off";
  const frozen = splash === "logo";
  const [out, setOut] = useState(false);
  const [held, setHeld] = useState(false); // wordmark has settled; now waiting on `ready`
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
    const hold = window.setTimeout(() => setHeld(true), 1350); // wordmark settled → start waiting on `ready`
    const onKey = () => beginExit(); // a key/click still skips straight in
    window.addEventListener("keydown", onKey);
    return () => {
      clearTimeout(hold);
      window.removeEventListener("keydown", onKey);
    };
  }, [skip, frozen, onDone, beginExit]);

  // Exit only once the wordmark has settled AND the model is ready — so the splash never fades to reveal
  // the app before the AI is up.
  useEffect(() => {
    if (held && ready) beginExit();
  }, [held, ready, beginExit]);

  if (skip) return null;

  return (
    <div className={`splash${out ? " splash--out" : ""}`} aria-hidden onClick={frozen ? undefined : beginExit}>
      <div className="wordmark splash__mark">Pushin</div>
      {held && !ready && !frozen && (
        <div className="splash__loading">
          <Loader2 className="size-5 animate-spin" />
        </div>
      )}
    </div>
  );
}
