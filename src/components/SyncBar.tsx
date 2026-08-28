import { RefreshCw } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { SyncProgress } from "../lib/ipc";

/**
 * The sidebar's sync bar — a strip above the AI status tag that appears only while something is
 * actually syncing, and says what and how far.
 *
 * Both engines feed the same `sync-progress` event (device mesh: rows then file bytes; Google:
 * pull/push/mirror), so this renders whichever is running without knowing which. It shows a real
 * percentage only when the backend knows the size of the work; a phase whose length isn't known yet
 * gets a moving indeterminate bar and no number, because a made-up percentage that parks at 90% is
 * worse than none — it teaches you to stop believing the bar.
 */

/** Human text for a progress tick. Exported so the test asserts on the same mapping the UI uses. */
export function describeSync(p: SyncProgress): string {
  if (p.source === "google") {
    if (p.phase === "pull") return "Pulling from Google";
    if (p.phase === "push") return "Pushing to Google";
    if (p.phase === "mirror") return "Mirroring blocks";
    return p.label || "Google Calendar";
  }
  if (p.phase === "files") return "Syncing vault files";
  return p.label ? `Syncing with ${p.label}` : "Syncing devices";
}

/** Whole-percent progress, or null when the phase is indeterminate. */
export function syncPercent(p: SyncProgress): number | null {
  if (p.total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((p.done / p.total) * 100)));
}

export default function SyncBar({ collapsed }: { collapsed: boolean }) {
  const progress = useStore((s) => s.syncProgress);
  if (!progress || !progress.active) return null;

  const pct = syncPercent(progress);
  const label = describeSync(progress);

  // Collapsed rail: no room for words, so the bar itself is the whole signal.
  if (collapsed) {
    return (
      <div
        className="px-1 py-1.5"
        title={pct === null ? label : `${label} — ${pct}%`}
        aria-label={label}
        role="progressbar"
        aria-valuenow={pct ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <Track pct={pct} />
      </div>
    );
  }

  return (
    <div
      className="rounded-lg border border-indigo-500/30 bg-indigo-500/10 px-3 py-1.5 space-y-1.5"
      role="progressbar"
      aria-valuenow={pct ?? undefined}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-label={label}
    >
      <div className="flex items-center gap-2 text-xs text-indigo-200">
        <RefreshCw className="size-3 animate-spin shrink-0" />
        <span className="truncate flex-1">{label}</span>
        {pct !== null && <span className="font-mono tabular-nums text-[11px] shrink-0">{pct}%</span>}
      </div>
      <Track pct={pct} />
    </div>
  );
}

/**
 * The bar itself. A known percentage fills from the left; an unknown one sweeps a short segment
 * across the track so it still reads as "working", not "stuck at zero".
 */
function Track({ pct }: { pct: number | null }) {
  return (
    <div className="h-1 w-full overflow-hidden rounded-full bg-white/10">
      <div
        className={clsx(
          "h-full bg-indigo-400",
          pct === null ? "w-1/3 sync-sweep" : "transition-[width] duration-300 ease-out",
        )}
        style={pct === null ? undefined : { width: `${pct}%` }}
      />
    </div>
  );
}
