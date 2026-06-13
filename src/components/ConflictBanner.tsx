import { AlertTriangle } from "lucide-react";
import { useStore } from "../state/store";
import type { Conflict } from "../lib/ipc";

function describe(c: Conflict): string {
  switch (c.kind) {
    case "deadlineMiss":
      return `“${c.title}” can’t be fully scheduled before its deadline`;
    case "unschedulable":
      return `“${c.title}” doesn’t fit in your free time (${c.remainingMinutes}m over)`;
    case "dependencyCycle":
      return `Dependency cycle detected among ${c.taskIds.length} tasks`;
  }
}

export default function ConflictBanner() {
  const conflicts = useStore((s) => s.conflicts);
  if (!conflicts.length) return null;
  return (
    <div className="shrink-0 bg-amber-500/10 border-b border-amber-500/30 px-4 py-2 text-sm text-amber-200 flex items-start gap-2">
      <AlertTriangle className="size-4 mt-0.5 shrink-0" />
      <div className="flex flex-wrap gap-x-5 gap-y-1">
        {conflicts.map((c, i) => (
          <span key={i}>• {describe(c)}</span>
        ))}
      </div>
    </div>
  );
}
