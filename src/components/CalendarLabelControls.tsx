import { Palette, Tag, X } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";

/** Compact calendar toolbar controls for label-based coloring and filtering. */
export default function CalendarLabelControls() {
  const labels = useStore((s) => s.labels);
  const colorByLabel = useStore((s) => s.calColorByLabel);
  const setColorByLabel = useStore((s) => s.setCalColorByLabel);
  const activeIds = useStore((s) => s.calLabelFilterIds);
  const toggleFilter = useStore((s) => s.toggleCalLabelFilter);
  const clearFilters = useStore((s) => s.clearCalLabelFilters);

  if (labels.length === 0) return null;

  return (
    <div className="ml-auto flex items-center gap-1 min-w-0 overflow-hidden">
      <button
        onClick={() => setColorByLabel(!colorByLabel)}
        title="Color calendar items by primary label"
        aria-pressed={colorByLabel}
        className={clsx(
          "p-1.5 rounded-md border shrink-0 transition",
          colorByLabel ? "bg-white/10 border-white/15 text-white" : "border-transparent text-gray-500 hover:text-white hover:bg-white/10",
        )}
      >
        <Palette className="size-3.5" />
      </button>

      <div className="hidden xl:flex items-center gap-1 min-w-0 overflow-x-auto">
        <Tag className="size-3.5 text-gray-600 shrink-0" />
        {labels.map((label) => {
          const active = activeIds.includes(label.id);
          return (
            <button
              key={label.id}
              onClick={() => toggleFilter(label.id)}
              title={`Filter by ${label.name}`}
              className={clsx(
                "flex items-center gap-1 max-w-28 rounded-md border px-1.5 py-1 text-[11px] leading-none transition",
                active ? "border-white/20 bg-white/10 text-white" : "border-white/10 text-gray-400 hover:text-white hover:bg-white/5",
              )}
            >
              <span className="size-1.5 rounded-full shrink-0" style={{ background: label.color }} />
              <span className="truncate">{label.name}</span>
            </button>
          );
        })}
        {activeIds.length > 0 && (
          <button onClick={clearFilters} title="Clear label filters" className="p-1 rounded text-gray-500 hover:text-white hover:bg-white/10 shrink-0">
            <X className="size-3.5" />
          </button>
        )}
      </div>
    </div>
  );
}
