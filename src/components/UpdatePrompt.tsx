import { useEffect, useRef, useState } from "react";
import { DownloadCloud, Loader2 } from "lucide-react";
import type { Update } from "@tauri-apps/plugin-updater";
import { checkForUpdate, downloadUpdate, installDownloaded, installUpdate } from "../lib/updates";

/**
 * Unattended updates, phone-style: Pushin finds a new release on its own, downloads it quietly in
 * the background with no UI at all, and only then asks, once, in a popup: install now, or later.
 * Nobody has to visit Settings and press "Check for updates" to get the version they are missing.
 *
 * Why a background download rather than the old banner's download-on-click: the banner asked first
 * and downloaded after, so saying yes meant sitting through a multi-hundred-megabyte transfer with
 * the app half-abandoned. Downloading first makes the answer cheap: by the time the question is
 * asked, "Install now" is a few seconds and a relaunch.
 *
 * Checks run on mount, every {@link CHECK_EVERY_MS}, and when the window regains focus (debounced by
 * the same interval so tabbing back and forth can't hammer GitHub). The repeat matters: Pushin is an
 * app people leave open for days, and a release's per-OS installers land minutes to tens of minutes
 * apart (see .github/workflows/release.yml), so the manifest often gains your platform only after
 * you last launched.
 *
 * Desktop only. Mount it in the desktop layout; the underlying plugins aren't built on mobile.
 */
export const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
/** "Later" means later, not never: the popup comes back after this long, and on the next launch. */
export const SNOOZE_MS = 8 * 60 * 60 * 1000;
/** Consecutive failed background downloads before we stop being quiet about it and ask by hand. */
const QUIET_FAILURES = 2;

const AUTO_KEY = "pushin:autoUpdate";
const SNOOZE_KEY = "pushin:updateSnoozedUntil";

/** Whether new versions download by themselves. On unless the user turned it off in Settings. */
export function autoUpdateEnabled(): boolean {
  try {
    return localStorage.getItem(AUTO_KEY) !== "0";
  } catch {
    return true; // storage blocked: the default wins rather than the feature breaking
  }
}

export function setAutoUpdateEnabled(on: boolean): void {
  try {
    localStorage.setItem(AUTO_KEY, on ? "1" : "0");
  } catch {
    /* storage blocked, so the setting just doesn't persist */
  }
}

/** Epoch ms until which this version's popup stays hidden, or 0. Per version, so a release newer
 *  than the one you postponed still gets to ask. */
function snoozedUntil(version: string): number {
  try {
    return Number(localStorage.getItem(`${SNOOZE_KEY}:${version}`)) || 0;
  } catch {
    return 0;
  }
}

function snooze(version: string): number {
  const until = Date.now() + SNOOZE_MS;
  try {
    localStorage.setItem(`${SNOOZE_KEY}:${version}`, String(until));
  } catch {
    /* storage blocked, so the postponement lasts for this session only */
  }
  return until;
}

/**
 * Preview hook: `?updatePreview=1` shows the popup against a stand-in release. The updater plugin is
 * inert outside a signed, bundled app pointed at a real GitHub release, so this is the only way to
 * look at this popup while working on it.
 */
function previewUpdate(): Update | null {
  try {
    if (new URLSearchParams(window.location.search).get("updatePreview") !== "1") return null;
    return { version: "0.0.0-preview", body: "A stand-in release, for looking at this popup." } as unknown as Update;
  } catch {
    return null;
  }
}

export default function UpdatePrompt({ hold = false }: { hold?: boolean }) {
  const [preview] = useState(previewUpdate);
  const [pending, setPending] = useState<Update | null>(preview);
  /** The bytes are down and staged in the backend, so installing is now near-instant. */
  const [staged, setStaged] = useState(!!preview);
  const [busy, setBusy] = useState<"download" | "install" | null>(null);
  const [pct, setPct] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [failures, setFailures] = useState(0);
  const [snoozedTo, setSnoozedTo] = useState(0);
  const [showNotes, setShowNotes] = useState(false);
  // Read inside the polling callback as a guard. Kept in a ref so the interval doesn't restart (and
  // re-fire a check) every time the phase changes.
  const busyRef = useRef<"download" | "install" | null>(null);
  busyRef.current = busy;

  useEffect(() => {
    if (previewUpdate()) return; // previewing the popup, not looking for a real release
    let active = true;
    let last = 0;
    // The version whose bytes are already staged, so a re-check doesn't re-download them.
    let stagedVersion: string | null = null;

    const run = async () => {
      last = Date.now();
      if (busyRef.current) return; // never swap the update out from under a transfer or an install
      const u = await checkForUpdate();
      if (!active || !u || u.version === stagedVersion) return;
      setPending(u);
      setStaged(false);
      setSnoozedTo(snoozedUntil(u.version));
      if (!autoUpdateEnabled()) return; // ask first and download on click, as before
      setBusy("download");
      try {
        await downloadUpdate(u, (p) => {
          if (active) setPct(p.pct);
        });
        if (!active) return;
        stagedVersion = u.version;
        setStaged(true);
        setFailures(0);
      } catch {
        // A background download nobody asked for must not raise an error the user can't act on: go
        // quiet and retry on the next check. Only once that keeps happening do we surface the
        // update as a manual one, so a machine that can't auto-download still gets offered it.
        if (active) setFailures((n) => n + 1);
      } finally {
        if (active) {
          setBusy(null);
          setPct(null);
        }
      }
    };

    const onFocus = () => {
      if (Date.now() - last >= CHECK_EVERY_MS) void run();
    };
    void run();
    const id = setInterval(() => void run(), CHECK_EVERY_MS);
    window.addEventListener("focus", onFocus);
    return () => {
      active = false;
      clearInterval(id);
      window.removeEventListener("focus", onFocus);
    };
  }, []);

  // Bring the popup back when the postponement runs out, without polling for it.
  useEffect(() => {
    if (!snoozedTo) return;
    const ms = snoozedTo - Date.now();
    if (ms <= 0) {
      setSnoozedTo(0);
      return;
    }
    const id = setTimeout(() => setSnoozedTo(0), ms);
    return () => clearTimeout(id);
  }, [snoozedTo]);

  // Silent until there is nothing left to wait for: staged and ready, or a manual case (auto-update
  // switched off, or the background download failing often enough to be worth mentioning).
  // `hold` is the opening sequence saying "not now": the splash, the new-user guide, the welcome-back
  // landing and the what's-new intro all own the whole window, and an update popup over the top of a
  // first run is the wrong first impression. The download still runs underneath, so the question is
  // simply asked once the user is actually in the app.
  const manual = !autoUpdateEnabled() || failures >= QUIET_FAILURES;
  const ask = !hold && !!pending && !snoozedTo && (staged || manual || busy === "install");
  if (!ask || !pending) return null;

  const later = () => setSnoozedTo(snooze(pending.version));

  const install = async () => {
    setBusy("install");
    setError(null);
    try {
      if (staged) await installDownloaded(pending);
      else await installUpdate(pending, (p) => setPct(p.pct));
      // Both relaunch on success, so nothing after this runs.
    } catch (e) {
      setBusy(null);
      setPct(null);
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const working = busy === "install";
  return (
    <div
      className="fade-in fixed inset-0 z-50 grid place-items-center bg-black/50 p-4"
      onClick={working ? undefined : later}
    >
      <div
        role="dialog"
        aria-modal="true"
        className="pop-in w-full max-w-sm overflow-hidden rounded-xl border border-white/10 bg-[var(--raised)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-5 pt-5 pb-4">
          <div className="grid size-10 place-items-center rounded-lg bg-white/[0.06] text-indigo-300">
            <DownloadCloud className="size-5" />
          </div>
          <h2 className="mt-3.5 text-base font-medium text-gray-100">
            Pushin {pending.version} is {staged ? "ready to install" : "available"}
          </h2>
          <p className="mt-1.5 text-xs leading-relaxed text-gray-500">
            {staged
              ? "It's already downloaded. Installing takes a few seconds and reopens Pushin."
              : "Pushin will download it, then reopen on the new version."}{" "}
            Your tasks, notes and settings are kept.
          </p>
          {pending.body && (
            <button
              onClick={() => setShowNotes((s) => !s)}
              className="mt-2 text-xs text-indigo-300 underline underline-offset-2 hover:text-indigo-200"
            >
              {showNotes ? "Hide notes" : "What's new"}
            </button>
          )}
          {showNotes && pending.body && (
            <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap text-xs leading-relaxed text-gray-500">
              {pending.body}
            </pre>
          )}
          {error && <p className="mt-2 text-xs text-red-300">Update failed: {error}</p>}
        </div>
        <div className="flex items-center justify-end gap-2 border-t border-white/10 px-5 py-3">
          <button
            onClick={later}
            disabled={working}
            className="rounded-md px-3 py-1.5 text-xs text-gray-400 hover:bg-white/5 hover:text-gray-200 disabled:opacity-40"
          >
            Later
          </button>
          <button
            onClick={install}
            disabled={working}
            className="inline-flex items-center gap-1.5 rounded-md bg-white/90 px-3 py-1.5 text-xs font-medium text-gray-900 hover:bg-white disabled:opacity-60"
          >
            {working && <Loader2 className="size-3.5 animate-spin" />}
            {working ? (pct !== null ? `Installing ${pct}%` : "Installing…") : staged ? "Install now" : "Update now"}
          </button>
        </div>
      </div>
    </div>
  );
}
