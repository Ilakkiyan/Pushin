import { useEffect, useRef, useState } from "react";
import { Info, Lock } from "lucide-react";

/** The calendar color legend, tucked behind a small info icon so the toolbar stays uncluttered.
 *  The popover is positioned `fixed` off the button so it escapes the toolbar's `overflow-hidden`
 *  (which otherwise clips it behind the calendar grid). */
export default function CalendarLegend() {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const popRef = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState<{ top: number; right: number }>({ top: 0, right: 0 });

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!btnRef.current?.contains(t) && !popRef.current?.contains(t)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const toggle = () => {
    if (!open && btnRef.current) {
      const r = btnRef.current.getBoundingClientRect();
      setPos({ top: r.bottom + 6, right: Math.max(8, window.innerWidth - r.right) });
    }
    setOpen((v) => !v);
  };

  return (
    <>
      <button
        ref={btnRef}
        onClick={toggle}
        title="What the colors mean"
        aria-label="Legend"
        className="shrink-0 rounded-md p-1.5 text-gray-500 transition hover:bg-white/10 hover:text-white"
      >
        <Info className="size-3.5" />
      </button>
      {open && (
        <div
          ref={popRef}
          style={{ top: pos.top, right: pos.right }}
          className="pop-in fixed z-[100] w-44 border border-white/10 bg-[var(--raised)] p-2.5 shadow-xl"
        >
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
    </>
  );
}
