import { useEffect, useState } from "react";
import { Sparkles, CalendarDays, ListChecks, Clock, X } from "lucide-react";
import { api, type Briefing } from "../lib/ipc";
import { useStore } from "../state/store";

/** The morning Daily Briefing as a slim, dismissible banner above the calendar — today's event
 *  count, what's due, and how much focus time is already blocked. Renders nothing on a clear day. */
export default function BriefingCard() {
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  const [dismissed, setDismissed] = useState(false);
  // The briefing is a snapshot of today's agenda, so it must be re-derived whenever the underlying
  // tasks/events change — otherwise completing a task leaves it listed as still due. `tasks`/`events`
  // are replaced (new array identity) by the store's refreshData after every mutation, so depending on
  // them re-fetches exactly when something could have changed. daily_briefing is a cheap local DB read.
  const tasks = useStore((s) => s.tasks);
  const events = useStore((s) => s.events);

  useEffect(() => {
    let cancelled = false;
    // Wrap in Promise.resolve so a missing/throwing api method (older test mocks) can't crash the
    // calendar — a failed briefing just shows nothing.
    Promise.resolve()
      .then(() => api.dailyBriefing())
      .then((b) => !cancelled && setBriefing(b))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [tasks, events]);

  if (dismissed || !briefing) return null;
  if (briefing.events.length === 0 && briefing.dueTasks.length === 0) return null;

  const focus = briefing.focusMinutes >= 60 ? `${(briefing.focusMinutes / 60).toFixed(1)}h` : `${briefing.focusMinutes}m`;
  const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

  return (
    <div className="shrink-0 border-b border-white/10 bg-white/[0.02] px-4 py-2.5 text-xs">
      <div className="flex items-center gap-2">
        <Sparkles className="size-3.5 text-indigo-300 shrink-0" />
        <span className="font-medium text-gray-200">Here's your {briefing.weekday}</span>
        <span className="tnum ml-1 flex items-center gap-3 text-[var(--ink-muted)]">
          <span className="flex items-center gap-1"><CalendarDays className="size-3" />{plural(briefing.events.length, "event")}</span>
          <span className="flex items-center gap-1"><ListChecks className="size-3" />{briefing.dueTasks.length} due</span>
          {briefing.focusMinutes > 0 && <span className="flex items-center gap-1"><Clock className="size-3" />{focus} focus</span>}
        </span>
        <button onClick={() => setDismissed(true)} title="Dismiss" className="ml-auto p-0.5 hoverable text-[var(--ink-faint)] hover:text-white">
          <X className="size-3.5" />
        </button>
      </div>
      {briefing.dueTasks.length > 0 && (
        <div className="mt-1.5 flex flex-wrap gap-1.5 pl-5">
          {briefing.dueTasks.slice(0, 6).map((t) => (
            <span key={t.id} className="inline-flex items-center border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-amber-200">
              {t.title}
            </span>
          ))}
          {briefing.dueTasks.length > 6 && <span className="tnum px-1 text-[var(--ink-faint)]">+{briefing.dueTasks.length - 6} more</span>}
        </div>
      )}
    </div>
  );
}
