import { useEffect, useState } from "react";
import { Download, Loader2, X } from "lucide-react";
import type { Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, installUpdate } from "../lib/updates";

/**
 * Top-of-window banner shown when a newer Pushin release is available on GitHub. Checks once on
 * mount (desktop only — mount this only in the desktop layout). Clicking "Update & restart"
 * downloads + installs the new version and relaunches; user data is untouched by the install.
 */
export default function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [pct, setPct] = useState<number | null>(null);
  const [showNotes, setShowNotes] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    checkForUpdate().then((u) => {
      if (active) setUpdate(u);
    });
    return () => {
      active = false;
    };
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
            className="shrink-0 rounded-md bg-indigo-500 hover:bg-indigo-400 text-white px-3 py-1 text-xs font-medium"
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
