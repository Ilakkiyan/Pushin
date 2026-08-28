import { useState } from "react";
import { Archive, ChevronRight } from "lucide-react";
import clsx from "clsx";
import { type Task } from "../lib/ipc";
import { useStore } from "../state/store";
import { parseLocal } from "../lib/time";

/**
 * The briefing's "stale" group: tasks overdue by 30+ days (`briefing::STALE_AFTER_DAYS`).
 *
 * These used to sit at the top of the due list forever — a deadline missed in June still led the
 * August briefing and buried what was actually due today. They aren't hidden, though: a task you
 * still owe shouldn't vanish because you ignored it. So they collapse into one line you can open,
 * with an archive action that makes letting go an explicit choice rather than a silent one.
 *
 * Archiving is reversible (a status change, not a delete) — the rows stay in the task list.
 */
export default function StaleTasks({ tasks, compact = false }: { tasks: Task[]; compact?: boolean }) {
  const [open, setOpen] = useState(false);
  const archiveTasks = useStore((s) => s.archiveTasks);
  const busy = useStore((s) => s.busy);

  if (tasks.length === 0) return null;

  const daysLate = (t: Task) => {
    if (!t.deadline) return 0;
    const d = parseLocal(t.deadline);
    return Math.max(0, Math.round((Date.now() - d.getTime()) / 86_400_000));
  };
  // The oldest item carries the group's headline age — it's the one that makes the point.
  const oldest = tasks.reduce((a, t) => Math.max(a, daysLate(t)), 0);

  return (
    <div className={clsx("border-t border-white/5", compact ? "mt-1.5 pt-1.5 pl-5" : "mt-4 pt-3")}>
      <div className="flex items-center gap-2">
        <button
          onClick={() => setOpen((o) => !o)}
          className="flex items-center gap-1.5 text-[11px] text-[var(--ink-faint)] hover:text-[var(--ink-muted)]"
        >
          <ChevronRight className={clsx("size-3 transition-transform", open && "rotate-90")} />
          <span className="tnum">
            Stale: {tasks.length} item{tasks.length === 1 ? "" : "s"}, {oldest}+ days late
          </span>
        </button>
        <button
          onClick={() => archiveTasks(tasks.map((t) => t.id))}
          disabled={busy}
          title="Archive all. They leave the briefing and stop being scheduled, but stay in your task list"
          className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[11px] text-[var(--ink-faint)] hoverable hover:text-white disabled:opacity-50"
        >
          <Archive className="size-3" /> Archive all
        </button>
      </div>

      {open && (
        <ul className="mt-1.5 space-y-1">
          {tasks.map((t) => (
            <li key={t.id} className="flex items-center gap-2 text-[11px] text-[var(--ink-muted)]">
              <span className="truncate flex-1">{t.title}</span>
              <span className="tnum shrink-0 text-[var(--ink-faint)]">{daysLate(t)}d late</span>
              <button
                onClick={() => archiveTasks([t.id])}
                disabled={busy}
                title="Archive this one"
                className="shrink-0 p-0.5 hoverable text-[var(--ink-faint)] hover:text-white disabled:opacity-50"
              >
                <Archive className="size-3" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
