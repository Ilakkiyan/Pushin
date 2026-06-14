import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Lock, Moon, Plus, X, NotebookPen } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type Block, type CalEvent, type Label } from "../lib/ipc";
import { addDays, addMinutes, fmtTime, parseLocal, sameDay, startOfWeek, toLocalIso, toLocalDate } from "../lib/time";
import ViewToggle from "../components/ViewToggle";
import CalendarLabelControls from "../components/CalendarLabelControls";

/** An all-day / multi-day event runs midnight→midnight (that's how trips are stored). */
function isAllDay(e: CalEvent): boolean {
  const s = parseLocal(e.start);
  const en = parseLocal(e.end);
  return s.getHours() === 0 && s.getMinutes() === 0 && en.getHours() === 0 && en.getMinutes() === 0 && en.getTime() > s.getTime();
}

const START_HOUR = 0;
const END_HOUR = 24;
const PX_PER_HOUR = 56;
const TOP_MIN = START_HOUR * 60;
const TOTAL_MIN = (END_HOUR - START_HOUR) * 60;

function minutesFromMidnight(d: Date) {
  return d.getHours() * 60 + d.getMinutes();
}
function snap(min: number, step = 15) {
  return Math.round(min / step) * step;
}

interface DragState {
  blockId: number;
  startClientY: number;
  origStart: Date;
  durationMin: number;
  deltaMin: number;
}

export default function CalendarPane() {
  const tasks = useStore((s) => s.tasks);
  const projects = useStore((s) => s.projects);
  const events = useStore((s) => s.events);
  const blocks = useStore((s) => s.blocks);
  const moveBlock = useStore((s) => s.moveBlock);
  const unlockBlock = useStore((s) => s.unlockBlock);
  const deleteEvent = useStore((s) => s.deleteEvent);
  const addEvent = useStore((s) => s.addEvent);
  const openDaily = useStore((s) => s.openDaily);
  const focusDateIso = useStore((s) => s.focusDateIso);
  const settings = useStore((s) => s.settings);
  const colorByLabel = useStore((s) => s.calColorByLabel);
  const labelFilterIds = useStore((s) => s.calLabelFilterIds);

  // The user's sleep window + recurring blocked time / routines, drawn as a shaded background so
  // free time reads as actual availability (not just empty space). Purely visual — the scheduler
  // already keeps these free server-side.
  const routineItems = useMemo<RoutineItem[]>(() => {
    if (!settings) return [];
    const items: RoutineItem[] = [];
    if (settings.sleepEnabled && settings.sleepStart && settings.sleepEnd)
      items.push({ name: "Sleep", start: settings.sleepStart, end: settings.sleepEnd, days: [], kind: "sleep" });
    for (const c of settings.commitments ?? [])
      if (c.start && c.end) items.push({ name: c.name || "Blocked", start: c.start, end: c.end, days: c.days ?? [], kind: c.kind || "blocked" });
    return items;
  }, [settings]);

  const [anchor, setAnchor] = useState(() => startOfWeek(focusDateIso ? parseLocal(focusDateIso) : new Date()));
  const [taskLabels, setTaskLabels] = useState<Record<number, Label[]>>({});
  const [eventLabels, setEventLabels] = useState<Record<number, Label[]>>({});

  // Month view hands off a day to open to; jump to its week when it changes.
  useEffect(() => {
    if (focusDateIso) setAnchor(startOfWeek(parseLocal(focusDateIso)));
  }, [focusDateIso]);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [modal, setModal] = useState<{ start: Date } | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Open scrolled to ~7am (or an hour before now) rather than midnight.
  useEffect(() => {
    if (!scrollRef.current) return;
    const focusHour = Math.max(0, Math.min(new Date().getHours() - 1, 7));
    scrollRef.current.scrollTop = focusHour * PX_PER_HOUR;
  }, []);

  const days = useMemo(() => Array.from({ length: 7 }, (_, i) => addMinutes(anchor, i * 1440)), [anchor]);
  const taskById = useMemo(() => new Map(tasks.map((t) => [t.id, t])), [tasks]);
  const projectById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);
  const labelFilterSet = useMemo(() => new Set(labelFilterIds), [labelFilterIds]);
  const taskIdsWithBlocks = useMemo(() => [...new Set(blocks.map((b) => b.taskId))], [blocks]);
  const eventIds = useMemo(() => events.map((e) => e.id), [events]);

  useEffect(() => {
    if (taskIdsWithBlocks.length === 0) {
      setTaskLabels({});
      return;
    }
    api.labelsForEntities("task", taskIdsWithBlocks).then(setTaskLabels).catch(() => setTaskLabels({}));
  }, [taskIdsWithBlocks]);

  useEffect(() => {
    if (eventIds.length === 0) {
      setEventLabels({});
      return;
    }
    api.labelsForEntities("event", eventIds).then(setEventLabels).catch(() => setEventLabels({}));
  }, [eventIds]);

  // Multi-day / all-day events render as horizontal bars in a row above the time grid
  // (spanning the day columns they cover), clipped to the visible week.
  const allDayBars = useMemo(() => {
    const ws = days[0];
    const dayIdx = (d: Date) =>
      Math.round((new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime() - new Date(ws.getFullYear(), ws.getMonth(), ws.getDate()).getTime()) / 86400000);
    return events
      .filter(isAllDay)
      .filter((e) => matchesLabelFilter(eventLabels[e.id], labelFilterSet))
      .map((e) => {
        const startIdx = dayIdx(parseLocal(e.start));
        const lastIdx = dayIdx(addDays(parseLocal(e.end), -1)); // end is exclusive midnight
        return { e, startIdx, lastIdx, col0: Math.max(0, startIdx), col1: Math.min(6, lastIdx), labels: eventLabels[e.id] ?? [] };
      })
      .filter((b) => b.lastIdx >= 0 && b.startIdx <= 6)
      .sort((a, b) => a.startIdx - b.startIdx || parseLocal(a.e.start).getTime() - parseLocal(b.e.start).getTime());
  }, [events, days, eventLabels, labelFilterSet]);

  // Drag lifecycle.
  useEffect(() => {
    if (!drag) return;
    const onMove = (e: PointerEvent) => {
      const deltaMin = snap(((e.clientY - drag.startClientY) / PX_PER_HOUR) * 60);
      setDrag((d) => (d ? { ...d, deltaMin } : d));
    };
    const onUp = () => {
      setDrag((d) => {
        if (d && d.deltaMin !== 0) {
          let startMin = minutesFromMidnight(d.origStart) + d.deltaMin;
          startMin = Math.max(TOP_MIN, Math.min(startMin, TOP_MIN + TOTAL_MIN - d.durationMin));
          const base = new Date(d.origStart);
          base.setHours(0, 0, 0, 0);
          const newStart = addMinutes(base, startMin);
          const newEnd = addMinutes(newStart, d.durationMin);
          moveBlock(d.blockId, toLocalIso(newStart), toLocalIso(newEnd));
        }
        return null;
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };
  }, [drag, moveBlock]);

  const top = (d: Date) => ((minutesFromMidnight(d) - TOP_MIN) / 60) * PX_PER_HOUR;
  const height = (mins: number) => Math.max(16, (mins / 60) * PX_PER_HOUR);

  const onColumnClick = (day: Date, e: React.MouseEvent) => {
    if (drag) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const mins = snap(TOP_MIN + ((e.clientY - rect.top) / PX_PER_HOUR) * 60, 30);
    const base = new Date(day);
    base.setHours(0, 0, 0, 0);
    setModal({ start: addMinutes(base, mins) });
  };

  const now = new Date();

  return (
    <div className="h-full flex flex-col">
      {/* Toolbar */}
      <div className="h-12 shrink-0 border-b border-white/10 flex items-center gap-2 px-4 min-w-0 overflow-hidden">
        <ViewToggle />
        <div className="w-px h-5 bg-white/10 mx-1 shrink-0" />
        <button onClick={() => setAnchor((a) => addMinutes(a, -7 * 1440))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronLeft className="size-4" />
        </button>
        <button onClick={() => setAnchor(startOfWeek(new Date()))} className="text-xs px-2 py-1 rounded hover:bg-white/10 shrink-0">
          Today
        </button>
        <button onClick={() => setAnchor((a) => addMinutes(a, 7 * 1440))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronRight className="size-4" />
        </button>
        <span className="text-sm text-gray-300 ml-2 whitespace-nowrap truncate">
          {anchor.toLocaleDateString([], { month: "short", day: "numeric" })} –{" "}
          {days[6].toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}
        </span>
        <CalendarLabelControls />
        {/* Legend is nice-to-have; hide it when the pane is too narrow (sidebar + chat aside squeeze it). */}
        <div className="hidden 2xl:flex items-center gap-3 text-[11px] text-gray-500 shrink-0 pl-3">
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-indigo-400" /> task block</span>
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-rose-400/70" /> fixed event</span>
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-emerald-400/70" /> habit</span>
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-slate-400/40" /> reserved</span>
          <span className="flex items-center gap-1"><Lock className="size-3" /> pinned</span>
        </div>
      </div>

      {/* Day headers */}
      <div className="shrink-0 grid border-b border-white/10" style={{ gridTemplateColumns: "56px repeat(7, 1fr)" }}>
        <div />
        {days.map((d) => (
          <div key={d.toISOString()} className={clsx("group relative py-2 text-center text-xs", sameDay(d, now) ? "text-indigo-300" : "text-gray-400")}>
            <div>{d.toLocaleDateString([], { weekday: "short" })}</div>
            <div className={clsx("text-sm", sameDay(d, now) && "font-semibold")}>{d.getDate()}</div>
            <button
              onClick={() => openDaily(toLocalDate(d))}
              title="Open this day's note"
              className="absolute top-1.5 right-1.5 opacity-0 group-hover:opacity-100 p-0.5 rounded text-gray-500 hover:text-indigo-300 hover:bg-white/10 transition"
            >
              <NotebookPen className="size-3.5" />
            </button>
          </div>
        ))}
      </div>

      {/* All-day / multi-day bar */}
      {allDayBars.length > 0 && (
        <div className="shrink-0 border-b border-white/10 grid gap-y-0.5 py-1" style={{ gridTemplateColumns: "56px repeat(7, 1fr)" }}>
          <div className="text-[10px] text-gray-600 self-center text-right pr-1" style={{ gridColumn: 1, gridRow: 1 }}>
            all-day
          </div>
          {allDayBars.map((b, i) => (
            <AllDayEventBar
              key={b.e.id}
              bar={b}
              row={i + 1}
              color={colorByLabel ? primaryLabelColor(b.labels) : null}
            />
          ))}
        </div>
      )}

      {/* Scrollable grid */}
      <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto">
        <div ref={gridRef} className="grid relative" style={{ gridTemplateColumns: "56px repeat(7, 1fr)", height: TOTAL_MIN / 60 * PX_PER_HOUR }}>
          {/* Time gutter */}
          <div className="relative">
            {Array.from({ length: END_HOUR - START_HOUR }, (_, i) => (
              <div key={i} className="absolute right-1 text-[10px] text-gray-600" style={{ top: i * PX_PER_HOUR - 6 }}>
                {((START_HOUR + i) % 12) || 12}{START_HOUR + i < 12 ? "am" : "pm"}
              </div>
            ))}
          </div>

          {/* Day columns */}
          {days.map((day) => {
            const dayEvents = events.filter((e) => !isAllDay(e) && sameDay(parseLocal(e.start), day) && matchesLabelFilter(eventLabels[e.id], labelFilterSet));
            const dayBlocks = blocks.filter((b) => sameDay(parseLocal(b.start), day) && matchesLabelFilter(taskLabels[b.taskId], labelFilterSet));
            return (
              <div
                key={day.toISOString()}
                className="relative border-l border-white/5"
                onClick={(e) => onColumnClick(day, e)}
              >
                {/* Hour lines */}
                {Array.from({ length: END_HOUR - START_HOUR }, (_, i) => (
                  <div key={i} className="absolute left-0 right-0 border-t border-white/5" style={{ top: i * PX_PER_HOUR }} />
                ))}

                {/* Reserved time (sleep + routines) — shaded, behind everything, click-through */}
                {routineSegmentsForDay(day, routineItems).map((seg) => (
                  <RoutineBlock
                    key={seg.key}
                    seg={seg}
                    top={((seg.startMin - TOP_MIN) / 60) * PX_PER_HOUR}
                    height={Math.max(12, ((seg.endMin - seg.startMin) / 60) * PX_PER_HOUR)}
                  />
                ))}

                {/* Now line */}
                {sameDay(day, now) && minutesFromMidnight(now) >= TOP_MIN && minutesFromMidnight(now) <= TOP_MIN + TOTAL_MIN && (
                  <div className="absolute left-0 right-0 z-20 pointer-events-none" style={{ top: top(now) }}>
                    <div className="h-px bg-rose-500" />
                    <div className="size-1.5 rounded-full bg-rose-500 -mt-1 -ml-0.5" />
                  </div>
                )}

                {/* Fixed events */}
                {dayEvents.map((ev) => (
                  <EventCard
                    key={`e${ev.id}`}
                    ev={ev}
                    top={top(parseLocal(ev.start))}
                    height={height(minutesBetweenEv(ev))}
                    color={colorByLabel ? primaryLabelColor(eventLabels[ev.id]) : null}
                    onDelete={() => deleteEvent(ev.id)}
                  />
                ))}

                {/* Task blocks */}
                {dayBlocks.map((b) => {
                  const t = taskById.get(b.taskId);
                  const project = t?.projectId != null ? projectById.get(t.projectId) : undefined;
                  const dur = minutesBetweenBlock(b);
                  const isDragging = drag?.blockId === b.id;
                  const dy = isDragging ? (drag!.deltaMin / 60) * PX_PER_HOUR : 0;
                  const color = (colorByLabel ? primaryLabelColor(taskLabels[b.taskId]) : null) ?? project?.color ?? "#6366f1";
                  return (
                    <div
                      key={`b${b.id}`}
                      onClick={(e) => e.stopPropagation()}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        if (b.locked) unlockBlock(b.id, b.start, b.end);
                      }}
                      onPointerDown={(e) => {
                        e.stopPropagation();
                        setDrag({ blockId: b.id, startClientY: e.clientY, origStart: parseLocal(b.start), durationMin: dur, deltaMin: 0 });
                      }}
                      className={clsx(
                        "absolute left-1 right-1 rounded-md px-1.5 py-1 text-[11px] overflow-hidden cursor-grab active:cursor-grabbing z-10 border",
                        isDragging ? "opacity-80 ring-2 ring-white/40" : "",
                      )}
                      style={{
                        top: top(parseLocal(b.start)) + dy,
                        height: height(dur),
                        background: color + "33",
                        borderColor: color + "aa",
                      }}
                      title={t?.title}
                    >
                      <div className="flex items-center gap-1 font-medium text-gray-100 truncate">
                        {b.locked && <Lock className="size-2.5 shrink-0" />}
                        {t?.title ?? "Task"}
                      </div>
                      <div className="text-gray-300/70">{fmtTime(parseLocal(b.start))}</div>
                    </div>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>

      {modal && <AddEventModal start={modal.start} onClose={() => setModal(null)} onSave={(title, end) => { addEvent(title, toLocalIso(modal.start), toLocalIso(end), "fixed"); setModal(null); }} />}
    </div>
  );
}

function primaryLabelColor(labels: Label[] | undefined): string | null {
  return labels?.[0]?.color ?? null;
}

function matchesLabelFilter(labels: Label[] | undefined, active: Set<number>): boolean {
  return active.size === 0 || (labels ?? []).some((label) => active.has(label.id));
}

function AllDayEventBar({ bar, row, color }: { bar: { e: CalEvent; startIdx: number; lastIdx: number; col0: number; col1: number }; row: number; color: string | null }) {
  return (
    <div
      onClick={(e) => e.stopPropagation()}
      className={clsx(
        "text-[11px] px-2 py-0.5 truncate border self-center mx-0.5",
        !color && (bar.e.kind === "habit" ? "bg-emerald-500/20 border-emerald-400/40 text-emerald-100" : "bg-rose-500/20 border-rose-400/40 text-rose-100"),
        bar.startIdx >= 0 ? "rounded-l-md" : "",
        bar.lastIdx <= 6 ? "rounded-r-md" : "",
      )}
      style={{
        gridColumn: `${bar.col0 + 2} / ${bar.col1 + 3}`,
        gridRow: row,
        ...(color ? { background: color + "33", borderColor: color + "aa", color: "#f9fafb" } : {}),
      }}
      title={bar.e.title}
    >
      {bar.startIdx < 0 ? "‹ " : ""}
      {bar.e.title}
      {bar.lastIdx > 6 ? " ›" : ""}
    </div>
  );
}

// --- Reserved-time (sleep + routines) overlay ---
type RoutineItem = { name: string; start: string; end: string; days: number[]; kind: string };
type RoutineSeg = { name: string; kind: string; startMin: number; endMin: number; key: string };

function minutesOfDay(hhmm: string): number | null {
  const [h, m] = hhmm.split(":").map(Number);
  return Number.isFinite(h) && Number.isFinite(m) ? h * 60 + m : null;
}
function isoWeekday(d: Date): number {
  return ((d.getDay() + 6) % 7) + 1; // 1=Mon..7=Sun
}

/** Segments of `items` that fall within calendar day `day` (0..1440 minutes). Overnight windows
 *  (end <= start, e.g. sleep 23:00→07:00) contribute an evening piece on their start day and a
 *  morning piece carried over from the previous day — each respecting the item's weekdays. */
function routineSegmentsForDay(day: Date, items: RoutineItem[]): RoutineSeg[] {
  const wd = isoWeekday(day);
  const prevWd = isoWeekday(addDays(day, -1));
  const out: RoutineSeg[] = [];
  items.forEach((it, idx) => {
    const start = minutesOfDay(it.start);
    const end = minutesOfDay(it.end);
    if (start == null || end == null) return;
    const overnight = end <= start;
    const everyDay = it.days.length === 0;
    if (everyDay || it.days.includes(wd)) {
      const endMin = overnight ? 1440 : end;
      if (endMin > start) out.push({ name: it.name, kind: it.kind, startMin: start, endMin, key: `${idx}a` });
    }
    if (overnight && end > 0 && (everyDay || it.days.includes(prevWd))) {
      out.push({ name: it.name, kind: it.kind, startMin: 0, endMin: end, key: `${idx}b` });
    }
  });
  return out;
}

function RoutineBlock({ seg, top, height }: { seg: RoutineSeg; top: number; height: number }) {
  const isSleep = seg.kind === "sleep";
  // Striped, desaturated fill so it reads as "unavailable" rather than a real event.
  const a = isSleep ? "rgba(129,140,248,0.16)" : "rgba(148,163,184,0.13)"; // indigo-400 / slate-400
  const b = isSleep ? "rgba(129,140,248,0.05)" : "rgba(148,163,184,0.04)";
  return (
    <div
      className="absolute left-0 right-0 z-0 pointer-events-none overflow-hidden border-y border-white/5"
      style={{ top, height, backgroundImage: `repeating-linear-gradient(45deg, ${a} 0 6px, ${b} 6px 12px)` }}
      title={`${seg.name} — reserved`}
    >
      {height >= 24 && (
        <div className="px-1.5 pt-0.5 text-[10px] text-gray-400/80 flex items-center gap-1 truncate">
          {isSleep && <Moon className="size-2.5 shrink-0" />}
          <span className="truncate">{seg.name}</span>
        </div>
      )}
    </div>
  );
}

function minutesBetweenEv(ev: CalEvent) {
  return Math.round((parseLocal(ev.end).getTime() - parseLocal(ev.start).getTime()) / 60000);
}
function minutesBetweenBlock(b: Block) {
  return Math.round((parseLocal(b.end).getTime() - parseLocal(b.start).getTime()) / 60000);
}

function EventCard({ ev, top, height, color, onDelete }: { ev: CalEvent; top: number; height: number; color: string | null; onDelete: () => void }) {
  const isHabit = ev.kind === "habit";
  return (
    <div
      onClick={(e) => e.stopPropagation()}
      className={clsx(
        "group absolute left-1 right-1 rounded-md px-1.5 py-1 text-[11px] overflow-hidden z-10 border",
        !color && (isHabit ? "bg-emerald-500/15 border-emerald-400/40 text-emerald-100" : "bg-rose-500/15 border-rose-400/40 text-rose-100"),
      )}
      style={{ top, height, ...(color ? { background: color + "26", borderColor: color + "99", color: "#f9fafb" } : {}) }}
      title={ev.title}
    >
      <div className="flex items-center gap-1">
        <span className="truncate flex-1">{ev.title}</span>
        <button onClick={onDelete} className="opacity-0 group-hover:opacity-100 hover:text-white">
          <X className="size-3" />
        </button>
      </div>
    </div>
  );
}

function AddEventModal({ start, onClose, onSave }: { start: Date; onClose: () => void; onSave: (title: string, end: Date) => void }) {
  const [title, setTitle] = useState("");
  const [duration, setDuration] = useState(60);
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50" onClick={onClose}>
      <div className="w-80 rounded-xl border border-white/10 bg-[#12151c] p-4 space-y-3" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium flex items-center gap-2"><Plus className="size-4" /> Add busy time</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-white"><X className="size-4" /></button>
        </div>
        <p className="text-xs text-gray-500">
          {start.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })} — the scheduler will plan around it.
        </p>
        <input
          autoFocus
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && title.trim() && onSave(title.trim(), addMinutes(start, duration))}
          placeholder="e.g. Team standup"
          className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50"
        />
        <div className="flex items-center gap-2">
          <input type="number" min={15} step={15} value={duration} onChange={(e) => setDuration(Number(e.target.value))} className="w-20 rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none" />
          <span className="text-xs text-gray-500">minutes</span>
          <button
            disabled={!title.trim()}
            onClick={() => onSave(title.trim(), addMinutes(start, duration))}
            className="ml-auto text-xs px-3 py-1.5 rounded-md bg-indigo-500 hover:bg-indigo-400 disabled:opacity-40"
          >
            Add
          </button>
        </div>
      </div>
    </div>
  );
}
