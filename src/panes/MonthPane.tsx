import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type Label } from "../lib/ipc";
import { addDays, fmtTime, parseLocal, sameDay, sameMonth, startOfMonth, startOfWeek } from "../lib/time";
import ViewToggle from "../components/ViewToggle";
import CalendarLabelControls from "../components/CalendarLabelControls";

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MAX_CHIPS = 3; // chips shown per day before the "+N more" roll-up

interface DayItem {
  start: Date;
  title: string;
  color: string; // base hex
  kind: "event" | "block";
}

export default function MonthPane() {
  const events = useStore((s) => s.events);
  const blocks = useStore((s) => s.blocks);
  const tasks = useStore((s) => s.tasks);
  const projects = useStore((s) => s.projects);
  const setCalMode = useStore((s) => s.setCalMode);
  const setFocusDate = useStore((s) => s.setFocusDate);
  const colorByLabel = useStore((s) => s.calColorByLabel);
  const labelFilterIds = useStore((s) => s.calLabelFilterIds);

  const [anchor, setAnchor] = useState(() => startOfMonth(new Date()));
  const [taskLabels, setTaskLabels] = useState<Record<number, Label[]>>({});
  const [eventLabels, setEventLabels] = useState<Record<number, Label[]>>({});

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

  // 6 weeks × 7 days, starting on the Monday on/before the 1st.
  const gridStart = useMemo(() => startOfWeek(anchor), [anchor]);
  const cells = useMemo(() => Array.from({ length: 42 }, (_, i) => addDays(gridStart, i)), [gridStart]);

  // Group all items (fixed events + task blocks) by calendar day, sorted by start time.
  const itemsByDay = useMemo(() => {
    const map = new Map<string, DayItem[]>();
    const key = (d: Date) => `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
    const addToDay = (dayDate: Date, item: DayItem) => {
      const k = key(dayDate);
      (map.get(k) ?? map.set(k, []).get(k)!).push(item);
    };
    const gridStart = cells[0].getTime();
    const gridEnd = cells[41].getTime();
    for (const e of events) {
      const labels = eventLabels[e.id];
      if (!matchesLabelFilter(labels, labelFilterSet)) continue;
      const s = parseLocal(e.start);
      const en = parseLocal(e.end);
      // Habit events (kind "habit") get the green habit accent; other fixed events are rose.
      const item: DayItem = { start: s, title: e.title, color: (colorByLabel ? primaryLabelColor(labels) : null) ?? (e.kind === "habit" ? "#22c55e" : "#f43f5e"), kind: "event" };
      const allDay = s.getHours() === 0 && s.getMinutes() === 0 && en.getHours() === 0 && en.getMinutes() === 0 && en.getTime() > s.getTime();
      if (allDay) {
        // A multi-day trip appears on each day it covers (clipped to the visible grid).
        for (let d = new Date(s.getFullYear(), s.getMonth(), s.getDate()); d.getTime() < en.getTime(); d = addDays(d, 1)) {
          if (d.getTime() >= gridStart && d.getTime() <= gridEnd) addToDay(d, item);
        }
      } else {
        addToDay(s, item);
      }
    }
    for (const b of blocks) {
      const t = taskById.get(b.taskId);
      const project = t?.projectId != null ? projectById.get(t.projectId) : undefined;
      const labels = taskLabels[b.taskId];
      if (!matchesLabelFilter(labels, labelFilterSet)) continue;
      addToDay(parseLocal(b.start), { start: parseLocal(b.start), title: t?.title ?? "Task", color: (colorByLabel ? primaryLabelColor(labels) : null) ?? project?.color ?? "#6366f1", kind: "block" });
    }
    for (const list of map.values()) list.sort((a, z) => a.start.getTime() - z.start.getTime());
    return map;
  }, [events, blocks, taskById, projectById, cells, eventLabels, taskLabels, colorByLabel, labelFilterSet]);

  const openWeek = (day: Date) => {
    setFocusDate(`${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, "0")}-${String(day.getDate()).padStart(2, "0")}T00:00:00`);
    setCalMode("week");
  };

  const now = new Date();

  return (
    <div className="h-full flex flex-col">
      {/* Toolbar */}
      <div className="h-12 shrink-0 border-b border-white/10 flex items-center gap-2 px-4 min-w-0 overflow-hidden">
        <ViewToggle />
        <div className="w-px h-5 bg-white/10 mx-1 shrink-0" />
        <button onClick={() => setAnchor((a) => new Date(a.getFullYear(), a.getMonth() - 1, 1))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronLeft className="size-4" />
        </button>
        <button onClick={() => setAnchor(startOfMonth(new Date()))} className="text-xs px-2 py-1 rounded hover:bg-white/10 shrink-0">
          Today
        </button>
        <button onClick={() => setAnchor((a) => new Date(a.getFullYear(), a.getMonth() + 1, 1))} className="p-1 rounded hover:bg-white/10 shrink-0">
          <ChevronRight className="size-4" />
        </button>
        <span className="text-sm text-gray-300 ml-2 whitespace-nowrap truncate">{anchor.toLocaleDateString([], { month: "long", year: "numeric" })}</span>
        <CalendarLabelControls />
        <div className="hidden 2xl:flex items-center gap-3 text-[11px] text-gray-500 shrink-0 pl-3">
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-indigo-400" /> task block</span>
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-rose-400/70" /> fixed event</span>
          <span className="flex items-center gap-1"><span className="size-2 rounded-sm bg-emerald-400/70" /> habit</span>
        </div>
      </div>

      {/* Weekday header */}
      <div className="shrink-0 grid grid-cols-7 border-b border-white/10">
        {WEEKDAYS.map((w) => (
          <div key={w} className="py-1.5 text-center text-[11px] text-gray-500">{w}</div>
        ))}
      </div>

      {/* Month grid */}
      <div className="flex-1 min-h-0 grid grid-cols-7 grid-rows-6">
        {cells.map((day) => {
          const inMonth = sameMonth(day, anchor);
          const isToday = sameDay(day, now);
          const items = itemsByDay.get(`${day.getFullYear()}-${day.getMonth()}-${day.getDate()}`) ?? [];
          return (
            <button
              key={day.toISOString()}
              onClick={() => openWeek(day)}
              className={clsx(
                "text-left border-b border-r border-white/5 p-1 min-h-0 overflow-hidden flex flex-col gap-0.5 transition hover:bg-white/[0.03] focus:outline-none focus:bg-white/5",
                !inMonth && "bg-black/20",
              )}
            >
              <div className="flex items-center justify-between px-0.5">
                <span
                  className={clsx(
                    "text-xs grid place-items-center size-5 rounded-full",
                    isToday ? "bg-indigo-500 text-white font-semibold" : inMonth ? "text-gray-300" : "text-gray-600",
                  )}
                >
                  {day.getDate()}
                </span>
              </div>
              <div className="flex-1 min-h-0 flex flex-col gap-0.5 overflow-hidden">
                {items.slice(0, MAX_CHIPS).map((it, i) => (
                  <div
                    key={i}
                    className="flex items-center gap-1 rounded px-1 py-px text-[10px] leading-tight truncate"
                    style={{ background: it.color + "26", color: "#e5e7eb" }}
                    title={`${fmtTime(it.start)} ${it.title}`}
                  >
                    <span className="size-1.5 rounded-full shrink-0" style={{ background: it.color }} />
                    <span className="truncate">{it.title}</span>
                  </div>
                ))}
                {items.length > MAX_CHIPS && (
                  <span className="text-[10px] text-gray-500 px-1">+{items.length - MAX_CHIPS} more</span>
                )}
              </div>
            </button>
          );
        })}
      </div>
    </div>
  );
}

function primaryLabelColor(labels: Label[] | undefined): string | null {
  return labels?.[0]?.color ?? null;
}

function matchesLabelFilter(labels: Label[] | undefined, active: Set<number>): boolean {
  return active.size === 0 || (labels ?? []).some((label) => active.has(label.id));
}
