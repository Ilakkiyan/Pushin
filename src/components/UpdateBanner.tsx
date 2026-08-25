import { useEffect, useState } from "react";
import { Download, Loader2, X } from "lucide-react";
import type { Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, installUpdate } from "../lib/updates";

/**
 * Top-of-window banner shown when a newer Pushin release is available on GitHub. Desktop only —
 * mount this only in the desktop layout. Clicking "Update & restart" downloads + installs the new
 * version and relaunches; user data is untouched by the install.
 *
 * It checks on mount AND re-checks periodically, because a once-on-mount check misses the common
 * case: Pushin is an app people leave open for days, and a release's per-OS installers land minutes
 * to tens of minutes apart (see .github/workflows/release.yml), so the manifest often gains your
 * platform only AFTER you last launched. Without a re-check you would have to restart the app to be
 * told an update exists. Also re-checks when the window regains focus, debounced by the same
 * interval so tabbing back and forth can't hammer GitHub.
 */
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
export default function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [pct, setPct] = useState<number | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let last = 0;
    const run = () => {
      last = Date.now();
      // Never swap the banner out from under an in-flight install.
      checkForUpdate().then((u) => {
        if (active && !installing) setUpdate(u);
      });
    };
    const onFocus = () => {
      if (Date.now() - last >= CHECK_EVERY_MS) run();
    };
    run();
    const id = setInterval(run, CHECK_EVERY_MS);
    window.addEventListener("focus", onFocus);
    return () => {
      active = false;
      clearInterval(id);
      window.removeEventListener("focus", onFocus);
    };
    // `installing` is read inside the callback only as a guard; re-subscribing on it would restart
    // the interval on every install attempt, so it is deliberately not a dependency.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!update || dismissed) return null;

  const onInstall = async () => {
    setInstalling(true);
    setError(null);
    try {
      await installUpdate(update, (p) => setPct(p.pct));
      // installUpdate relaunches on success — nothing after this runs.
    } catch (e) {
      setInstalling(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="shrink-0 bg-indigo-500/10 border-b border-indigo-500/30 px-4 py-2 text-sm text-indigo-100 flex items-start gap-3">
      <Download className="size-4 mt-0.5 shrink-0" />
      <div className="flex-1 min-w-0">
        <span>
          Pushin <strong>{update.version}</strong> is available.
        </span>{" "}
        <span className="text-indigo-200/70">Your tasks, notes, and settings are kept.</span>
        {update.body && (
          <button onClick={() => setShowNotes((s) => !s)} className="ml-2 underline text-indigo-200/80 hover:text-white">
            {showNotes ? "Hide notes" : "What's new"}
          </button>
        )}
        {showNotes && update.body && (
          <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap text-xs text-indigo-200/80">{update.body}</pre>
        )}
        {error && <div className="mt-1 text-xs text-red-300">Update failed: {error}</div>}
      </div>
      {installing ? (
        <span className="shrink-0 flex items-center gap-1.5 text-indigo-200">
          <Loader2 className="size-4 animate-spin" />
          {pct !== null ? `${pct}%` : "Installing…"}
        </span>
      ) : (
        <>
          <button
            onClick={onInstall}
            className="shrink-0 rounded-md bg-white/90 hover:bg-white text-gray-900 px-3 py-1 text-xs font-medium"
          >
            Update &amp; restart
          </button>
          <button onClick={() => setDismissed(true)} title="Later" className="shrink-0 text-indigo-200/70 hover:text-white">
            <X className="size-4" />
          </button>
        </>
      )}
    </div>
  );
}
