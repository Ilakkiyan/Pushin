import { useEffect, useRef, useState } from "react";
import { Info, Lock } from "lucide-react";

/** The calendar color legend, tucked behind a small info icon so the toolbar stays uncluttered.
 *  (Replaces the always-on inline legend row that crowded the calendar header.) */
export default function CalendarLegend() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  return (
    <div ref={ref} className="relative shrink-0">
      <button
        onClick={() => setOpen((v) => !v)}
        title="What the colors mean"
        aria-label="Legend"
        className="p-1.5 rounded-md text-gray-500 hover:text-white hover:bg-white/10 transition"
      >
        <Info className="size-3.5" />
      </button>
      {open && (
        <div className="pop-in absolute right-0 top-full z-50 mt-1 w-44 border border-white/10 bg-[var(--raised)] p-2.5 shadow-xl">
          <div className="mb-2 text-[10px] uppercase tracking-wide text-gray-500">Legend</div>
          <div className="flex flex-col gap-1.5 text-[11px] text-gray-300">
            <span className="flex items-center gap-2"><span className="size-2 rounded-sm bg-indigo-400" /> task block</span>
            <span className="flex items-center gap-2"><span className="size-2 rounded-sm bg-rose-400/70" /> fixed event</span>
            <span className="flex items-center gap-2"><span className="size-2 rounded-sm bg-emerald-400/70" /> habit</span>
            <span className="flex items-center gap-2"><span className="size-2 rounded-sm bg-slate-400/40" /> reserved</span>
            <span className="flex items-center gap-2"><Lock className="size-3" /> pinned</span>
          </div>
        </div>
      )}
    </div>
  );
}
