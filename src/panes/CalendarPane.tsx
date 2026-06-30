import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Lock, Moon, Plus, X, NotebookPen } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type Block, type CalEvent, type Label, type MeetingBrief } from "../lib/ipc";
import { addDays, addMinutes, fmtTime, parseLocal, sameDay, startOfWeek, toLocalIso, toLocalDate } from "../lib/time";
import ViewToggle from "../components/ViewToggle";
import CalendarLabelControls from "../components/CalendarLabelControls";
import CalendarLegend from "../components/CalendarLegend";
import LabelPicker from "../components/LabelPicker";
import BriefingCard from "../components/BriefingCard";

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

export default function CalendarPane({ days: dayCount = 7 }: { days?: number }) {
  const tasks = useStore((s) => s.tasks);
  const projects = useStore((s) => s.projects);
  const events = useStore((s) => s.events);
  const blocks = useStore((s) => s.blocks);
  const moveBlock = useStore((s) => s.moveBlock);
  const unlockBlock = useStore((s) => s.unlockBlock);
  const deleteEvent = useStore((s) => s.deleteEvent);
  const addEvent = useStore((s) => s.addEvent);
  const openDaily = useStore((s) => s.openDaily);
  const openEntityNote = useStore((s) => s.openEntityNote);
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

  // How many day-columns to render: 7 = the week grid (desktop), 1 = a single-day view (phones).
  // The anchor is the week-start for the week grid, or the day-start for the day view.
  const anchorStart = (date: Date) => {
    if (dayCount >= 7) return startOfWeek(date);
    const d = new Date(date);
    d.setHours(0, 0, 0, 0);
    return d;
  };
  const gridCols = `56px repeat(${dayCount}, 1fr)`;
  const lastCol = dayCount - 1;

  const [anchor, setAnchor] = useState(() => anchorStart(focusDateIso ? parseLocal(focusDateIso) : new Date()));
  const [taskLabels, setTaskLabels] = useState<Record<number, Label[]>>({});
  const [eventLabels, setEventLabels] = useState<Record<number, Label[]>>({});

  // Month view hands off a day to open to; jump to its week when it changes.
  useEffect(() => {
    if (focusDateIso) setAnchor(anchorStart(parseLocal(focusDateIso)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusDateIso, dayCount]);
  const [drag, setDrag] = useState<DragState | null>(null);
  const [modal, setModal] = useState<{ start: Date } | null>(null);
  // The event whose detail/label popover is open, plus a counter to refresh event labels (for
  // color-by-label + filters) after the popover edits them.
  const [detail, setDetail] = useState<CalEvent | null>(null);
  const [labelRefresh, setLabelRefresh] = useState(0);
  // Selected time slot (mouse click or keyboard arrows) — a {column, minutes-from-midnight} cursor.
  const [cursor, setCursor] = useState<{ dayIdx: number; minutes: number } | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Open scrolled to ~7am (or an hour before now) rather than midnight.
  useEffect(() => {
    if (!scrollRef.current) return;
    const focusHour = Math.max(0, Math.min(new Date().getHours() - 1, 7));
    scrollRef.current.scrollTop = focusHour * PX_PER_HOUR;
  }, []);

  const days = useMemo(() => Array.from({ length: dayCount }, (_, i) => addMinutes(anchor, i * 1440)), [anchor, dayCount]);
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
  }, [eventIds, labelRefresh]);

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
        return { e, startIdx, lastIdx, col0: Math.max(0, startIdx), col1: Math.min(lastCol, lastIdx), labels: eventLabels[e.id] ?? [] };
      })
      .filter((b) => b.lastIdx >= 0 && b.startIdx <= lastCol)
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

  const SNAP = 30; // minutes per arrow step / click snap
  const topMin = (minutes: number) => ((minutes - TOP_MIN) / 60) * PX_PER_HOUR;
  const slotMinutes = (e: React.MouseEvent) => {
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return snap(TOP_MIN + ((e.clientY - rect.top) / PX_PER_HOUR) * 60, SNAP);
  };
  // Single click SELECTS the slot (keyboard takes over from there); double-click / Enter CREATES.
  const onColumnClick = (dayIdx: number, e: React.MouseEvent) => {
    if (drag) return;
    setCursor({ dayIdx, minutes: slotMinutes(e) });
    gridRef.current?.focus();
  };
  const createAt = (dayIdx: number, minutes: number) => {
    const base = new Date(days[dayIdx]);
    base.setHours(0, 0, 0, 0);
    setModal({ start: addMinutes(base, minutes) });
  };
  const onColumnDblClick = (dayIdx: number, e: React.MouseEvent) => {
    if (drag) return;
    createAt(dayIdx, slotMinutes(e));
  };
  const onGridKey = (e: React.KeyboardEvent) => {
    const maxMin = TOP_MIN + TOTAL_MIN - SNAP;
    if (e.key === "ArrowUp" || e.key === "ArrowDown" || e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      setCursor((c) => {
        const seed = c ?? {
          dayIdx: Math.max(0, days.findIndex((d) => sameDay(d, new Date()))),
          minutes: snap(Math.min(maxMin, Math.max(TOP_MIN, minutesFromMidnight(new Date()))), SNAP),
        };
        let { dayIdx, minutes } = seed;
        if (e.key === "ArrowLeft") dayIdx = Math.max(0, dayIdx - 1);
        if (e.key === "ArrowRight") dayIdx = Math.min(dayCount - 1, dayIdx + 1);
        if (e.key === "ArrowUp") minutes = Math.max(TOP_MIN, minutes - SNAP);
        if (e.key === "ArrowDown") minutes = Math.min(maxMin, minutes + SNAP);
        return { dayIdx, minutes };
      });
    } else if (e.key === "Enter" && cursor) {
      e.preventDefault();
      createAt(cursor.dayIdx, cursor.minutes);
    } else if (e.key === "Escape") {
      setCursor(null);
    }
  };
  // Keep the keyboard cursor in view as it moves.
  useEffect(() => {
    if (!cursor || !scrollRef.current) return;
    const el = scrollRef.current;
    const y = topMin(cursor.minutes);
    if (y < el.scrollTop) el.scrollTop = Math.max(0, y - 60);
    else if (y > el.scrollTop + el.clientHeight - 40) el.scrollTop = y - el.clientHeight + 80;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cursor]);

  // Open (creating + linking on first use) the vault note for an event, auto-titled "Title — Mon D".
  const makeEventNote = (ev: CalEvent) => {
    const when = parseLocal(ev.start).toLocaleDateString([], { month: "short", day: "numeric" });
    openEntityNote("event", ev.id, `${ev.title} — ${when}`);
    setDetail(null);
  };
  // Ctrl/⌘+T → note for the event whose detail is open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "t" && detail) {
        e.preventDefault();
        makeEventNote(detail);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail]);

  const now = new Date();

  return (
    <div className="h-full flex flex-col">
      {/* Toolbar */}
      <div className="h-12 shrink-0 border-b border-white/10 flex items-center gap-2 px-4 min-w-0 overflow-hidden">
        <ViewToggle />
        <div className="w-px h-5 bg-white/10 mx-1 shrink-0" />
        <button onClick={() => setAnchor((a) => addDays(a, -dayCount))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronLeft className="size-4" />
        </button>
        <button onClick={() => setAnchor(anchorStart(new Date()))} className="text-xs px-2 py-1 rounded hover:bg-white/10 shrink-0">
          Today
        </button>
        <button onClick={() => setAnchor((a) => addDays(a, dayCount))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronRight className="size-4" />
        </button>
        <span className="text-sm text-gray-300 ml-2 whitespace-nowrap truncate">
          {dayCount === 1
            ? anchor.toLocaleDateString([], { weekday: "long", month: "short", day: "numeric" })
            : `${anchor.toLocaleDateString([], { month: "short", day: "numeric" })} – ${days[lastCol].toLocaleDateString([], { month: "short", day: "numeric", year: "numeric" })}`}
        </span>
        <CalendarLabelControls />
        <CalendarLegend />
      </div>

      <BriefingCard />

      {/* Day headers */}
      <div className="shrink-0 grid border-b border-white/10" style={{ gridTemplateColumns: gridCols }}>
        <div />
        {days.map((d) => (
          <div key={d.toISOString()} className={clsx("group relative py-2 text-center text-xs", sameDay(d, now) ? "text-indigo-300" : "text-gray-400")}>
            <div>{d.toLocaleDateString([], { weekday: "short" })}</div>
            <div className={clsx("text-sm", sameDay(d, now) && "font-semibold")}>{d.getDate()}</div>
            <button
              onClick={() => openDaily(toLocalDate(d))}
              title="Open this day's note"
              className={clsx(
                "absolute top-1.5 right-1.5 p-0.5 rounded text-gray-500 hover:text-indigo-300 hover:bg-white/10 transition",
                dayCount === 1 ? "opacity-100" : "opacity-0 group-hover:opacity-100",
              )}
            >
              <NotebookPen className="size-3.5" />
            </button>
          </div>
        ))}
      </div>

      {/* All-day / multi-day bar */}
      {allDayBars.length > 0 && (
        <div className="shrink-0 border-b border-white/10 grid gap-y-0.5 py-1" style={{ gridTemplateColumns: gridCols }}>
          <div className="text-[10px] text-gray-600 self-center text-right pr-1" style={{ gridColumn: 1, gridRow: 1 }}>
            all-day
          </div>
          {allDayBars.map((b, i) => (
            <AllDayEventBar
              key={b.e.id}
              bar={b}
              row={i + 1}
              lastCol={lastCol}
              color={colorByLabel ? primaryLabelColor(b.labels) : null}
            />
          ))}
        </div>
      )}

      {/* Scrollable grid */}
      <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto">
        <div
          ref={gridRef}
          tabIndex={0}
          onKeyDown={onGridKey}
          className="grid relative outline-none"
          style={{ gridTemplateColumns: gridCols, height: (TOTAL_MIN / 60) * PX_PER_HOUR }}
        >
          {/* Time gutter */}
          <div className="relative">
            {Array.from({ length: END_HOUR - START_HOUR }, (_, i) => (
              <div key={i} className="absolute right-1 text-[10px] text-gray-600" style={{ top: i * PX_PER_HOUR - 6 }}>
                {((START_HOUR + i) % 12) || 12}{START_HOUR + i < 12 ? "am" : "pm"}
              </div>
            ))}
          </div>

          {/* Day columns */}
          {days.map((day, dayIdx) => {
            const dayEvents = events.filter((e) => !isAllDay(e) && sameDay(parseLocal(e.start), day) && matchesLabelFilter(eventLabels[e.id], labelFilterSet));
            const dayBlocks = blocks.filter((b) => sameDay(parseLocal(b.start), day) && matchesLabelFilter(taskLabels[b.taskId], labelFilterSet));
            return (
              <div
                key={day.toISOString()}
                className="relative border-l border-white/5"
                onClick={(e) => onColumnClick(dayIdx, e)}
                onDoubleClick={(e) => onColumnDblClick(dayIdx, e)}
              >
                {/* Selected-slot cursor (mouse/keyboard) */}
                {cursor?.dayIdx === dayIdx && (
                  <div
                    className="absolute left-0.5 right-0.5 z-30 pointer-events-none rounded-md ring-1 ring-white/45 bg-white/[0.07]"
                    style={{ top: topMin(cursor.minutes), height: height(SNAP) }}
                  />
                )}
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
                    onOpen={() => setDetail(ev)}
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
                        // Non-destructive: open the task's note. (Unpinning is the lock button below —
                        // double-click used to unpin, which silently re-flowed the block to 9am.)
                        e.stopPropagation();
                        if (t) openEntityNote("task", t.id, t.title);
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
                        {b.locked && (
                          <button
                            onPointerDown={(e) => e.stopPropagation()}
                            onClick={(e) => {
                              e.stopPropagation();
                              unlockBlock(b.id, b.start, b.end);
                            }}
                            title="Pinned to this time — click to unpin and let Pushin reschedule it"
                            className="shrink-0 text-gray-300 hover:text-white"
                          >
                            <Lock className="size-2.5" />
                          </button>
                        )}
                        <span className="truncate">{t?.title ?? "Task"}</span>
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
      {detail && (
        <EventDetailModal
          ev={detail}
          onNotes={() => makeEventNote(detail)}
          onClose={() => { setDetail(null); setLabelRefresh((n) => n + 1); }}
          onDelete={() => { deleteEvent(detail.id); setDetail(null); setLabelRefresh((n) => n + 1); }}
        />
      )}
    </div>
  );
}

function primaryLabelColor(labels: Label[] | undefined): string | null {
  return labels?.[0]?.color ?? null;
}

function matchesLabelFilter(labels: Label[] | undefined, active: Set<number>): boolean {
  return active.size === 0 || (labels ?? []).some((label) => active.has(label.id));
}

function AllDayEventBar({ bar, row, color, lastCol }: { bar: { e: CalEvent; startIdx: number; lastIdx: number; col0: number; col1: number }; row: number; color: string | null; lastCol: number }) {
  return (
    <div
      onClick={(e) => e.stopPropagation()}
      className={clsx(
        "text-[11px] px-2 py-0.5 truncate border self-center mx-0.5",
        !color && (bar.e.kind === "habit" ? "bg-emerald-500/20 border-emerald-400/40 text-emerald-100" : "bg-rose-500/20 border-rose-400/40 text-rose-100"),
        bar.startIdx >= 0 ? "rounded-l-md" : "",
        bar.lastIdx <= lastCol ? "rounded-r-md" : "",
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
      {bar.lastIdx > lastCol ? " ›" : ""}
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

function EventCard({ ev, top, height, color, onDelete, onOpen }: { ev: CalEvent; top: number; height: number; color: string | null; onDelete: () => void; onOpen: () => void }) {
  const isHabit = ev.kind === "habit";
  return (
    <div
      onClick={(e) => {
        e.stopPropagation();
        if (!isHabit) onOpen(); // open the detail/label popover for real events (habits → HabitsPane)
      }}
      className={clsx(
        "group absolute left-1 right-1 rounded-md px-1.5 py-1 text-[11px] overflow-hidden z-10 border",
        !isHabit && "cursor-pointer",
        !color && (isHabit ? "bg-emerald-500/15 border-emerald-400/40 text-emerald-100" : "bg-rose-500/15 border-rose-400/40 text-rose-100"),
      )}
      style={{ top, height, ...(color ? { background: color + "26", borderColor: color + "99", color: "#f9fafb" } : {}) }}
      title={ev.title}
    >
      <div className="flex items-center gap-1">
        <span className="truncate flex-1">{ev.title}</span>
        <button onClick={(e) => { e.stopPropagation(); onDelete(); }} className="opacity-0 group-hover:opacity-100 hover:text-white">
          <X className="size-3" />
        </button>
      </div>
    </div>
  );
}

/** Click an event → a small popover to (re)label it and delete it. Events have no inline editor, and
 *  the calendar block itself is `overflow-hidden`, so labels live here rather than on the block. */
function EventDetailModal({ ev, onClose, onDelete, onNotes }: { ev: CalEvent; onClose: () => void; onDelete: () => void; onNotes: () => void }) {
  const createTask = useStore((s) => s.createTask);
  const [brief, setBrief] = useState<MeetingBrief | null>(null);
  const [notes, setNotes] = useState("");
  const [items, setItems] = useState<string[]>([]);
  const [extracting, setExtracting] = useState(false);

  const extract = async () => {
    setExtracting(true);
    try {
      setItems(await api.extractActionItems(notes));
    } catch {
      setItems([]);
    } finally {
      setExtracting(false);
    }
  };
  const addItem = async (i: number) => {
    await createTask(items[i], 30, null, 2);
    setItems((prev) => prev.filter((_, j) => j !== i));
  };
  useEffect(() => {
    let cancelled = false;
    Promise.resolve()
      .then(() => api.meetingBrief(ev.id))
      .then((b) => !cancelled && setBrief(b))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [ev.id]);
  const hasBrief = !!brief && (brief.attendees.length > 0 || brief.linkedPages.length > 0);

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50" onClick={onClose}>
      <div className="w-80 rounded-xl border border-white/10 bg-[var(--raised)] p-4 space-y-3" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-start justify-between gap-2">
          <h3 className="text-sm font-medium leading-snug">{ev.title}</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-white shrink-0"><X className="size-4" /></button>
        </div>
        <p className="text-xs text-gray-500">
          {parseLocal(ev.start).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })} – {fmtTime(parseLocal(ev.end))}
        </p>
        {hasBrief && (
          <div className="space-y-2 border-t border-white/10 pt-3">
            {brief!.attendees.length > 0 && (
              <div>
                <div className="text-xs text-gray-400 mb-1">Attendees</div>
                {brief!.attendees.map((a) => (
                  <div key={a.person.email ?? a.person.name} className="mb-1">
                    <div className="text-sm text-gray-200">
                      {a.person.name}
                      <span className="text-xs text-gray-500"> · {a.totalMeetings} meeting{a.totalMeetings === 1 ? "" : "s"}</span>
                    </div>
                    {a.person.notes && <div className="text-xs text-gray-500 line-clamp-2">{a.person.notes}</div>}
                  </div>
                ))}
              </div>
            )}
            {brief!.linkedPages.length > 0 && (
              <div>
                <div className="text-xs text-gray-400 mb-1">Linked notes</div>
                {brief!.linkedPages.map((p) => (
                  <div key={p.id} className="truncate text-sm text-indigo-300">{p.title}</div>
                ))}
              </div>
            )}
          </div>
        )}
        <div>
          <div className="text-xs text-gray-400 mb-1">Labels</div>
          <LabelPicker kind="event" entityId={ev.id} />
        </div>

        <div className="space-y-1.5 border-t border-white/10 pt-3">
          <div className="text-xs text-gray-400">Action items from notes</div>
          <textarea
            value={notes}
            onChange={(e) => setNotes(e.target.value)}
            rows={3}
            placeholder="Paste meeting notes — Pushin suggests follow-up tasks you confirm."
            className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-xs outline-none focus:border-indigo-500/50 resize-y"
          />
          <button
            onClick={extract}
            disabled={extracting || !notes.trim()}
            className="text-xs px-2.5 py-1 rounded-md bg-white/5 border border-white/10 hover:bg-white/10 disabled:opacity-40"
          >
            {extracting ? "Extracting…" : "Extract action items"}
          </button>
          {items.length > 0 && (
            <div className="flex flex-wrap gap-1.5 pt-1">
              {items.map((it, i) => (
                <button
                  key={i}
                  onClick={() => addItem(i)}
                  title="Add as a task"
                  className="inline-flex items-center gap-1 rounded-full border border-indigo-500/30 bg-indigo-500/10 px-2 py-0.5 text-[11px] text-indigo-200 hover:bg-indigo-500/20"
                >
                  <Plus className="size-2.5" /> {it}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex items-center justify-between pt-1">
          <button
            onClick={onNotes}
            title="Open the linked note (creates one on first use)"
            className="text-xs px-3 py-1.5 rounded-md bg-white/5 border border-white/10 hover:bg-white/10 inline-flex items-center gap-1.5"
          >
            <NotebookPen className="size-3.5" /> Notes
            <kbd className="text-[10px] text-gray-500 border border-white/10 rounded px-1">⌘T</kbd>
          </button>
          <button onClick={onDelete} className="text-xs px-3 py-1.5 rounded-md border border-rose-500/40 text-rose-300 hover:bg-rose-500/10">
            Delete event
          </button>
        </div>
      </div>
    </div>
  );
}

function AddEventModal({ start, onClose, onSave }: { start: Date; onClose: () => void; onSave: (title: string, end: Date) => void }) {
  const [title, setTitle] = useState("");
  const [duration, setDuration] = useState(60);
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50" onClick={onClose}>
      <div className="w-80 rounded-xl border border-white/10 bg-[var(--raised)] p-4 space-y-3" onClick={(e) => e.stopPropagation()}>
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
            className="ml-auto text-xs px-3 py-1.5 rounded-md bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40"
          >
            Add
          </button>
        </div>
      </div>
    </div>
  );
}
