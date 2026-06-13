import { useEffect, useState } from "react";
import { CalendarPlus, Check, Flame, Plus, Trash2, TrendingUp, Trophy } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { HabitStats } from "../lib/ipc";
import { humanMinutes, mondayIndex, parseLocal } from "../lib/time";

const COLORS = ["#22c55e", "#0ea5e9", "#a855f7", "#f59e0b", "#ef4444", "#ec4899", "#14b8a6", "#6366f1"];
const WEEKDAYS = [
  { n: 1, l: "M" },
  { n: 2, l: "T" },
  { n: 3, l: "W" },
  { n: 4, l: "T" },
  { n: 5, l: "F" },
  { n: 6, l: "S" },
  { n: 7, l: "S" },
];
const WD_FULL = ["", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

type Cadence = "daily" | "weekly" | "interval";

/** Human label for a habit's recurrence. */
function cadenceLabel(h: { cadence: string; days: number[]; intervalDays: number }): string {
  if (h.cadence === "weekly" && h.days.length) {
    const s = [...h.days].sort((a, b) => a - b);
    if (s.length === 7) return "Daily";
    if (s.join() === "1,2,3,4,5") return "Weekdays";
    if (s.join() === "6,7") return "Weekends";
    return s.map((d) => WD_FULL[d]).join(", ");
  }
  if (h.cadence === "interval" && h.intervalDays > 1) {
    return h.intervalDays === 2 ? "Every other day" : `Every ${h.intervalDays} days`;
  }
  return "Daily";
}

export default function HabitsPane() {
  const habits = useStore((s) => s.habits);
  const loadHabits = useStore((s) => s.loadHabits);
  const createHabit = useStore((s) => s.createHabit);

  const [name, setName] = useState("");
  const [color, setColor] = useState(COLORS[0]);
  const [duration, setDuration] = useState(30);
  const [cadence, setCadence] = useState<Cadence>("daily");
  const [selDays, setSelDays] = useState<number[]>([]);
  const [intervalDays, setIntervalDays] = useState(2);

  useEffect(() => {
    loadHabits();
  }, [loadHabits]);

  const toggleDay = (n: number) => setSelDays((d) => (d.includes(n) ? d.filter((x) => x !== n) : [...d, n].sort((a, b) => a - b)));

  const add = () => {
    const n = name.trim();
    if (!n) return;
    // Weekly with no days picked falls back to daily; interval coerces to ≥2.
    const cad: Cadence = cadence === "weekly" && selDays.length === 0 ? "daily" : cadence;
    const days = cad === "weekly" ? selDays : [];
    const iv = cad === "interval" ? Math.max(2, intervalDays) : 1;
    createHabit(n, color, cad, days, iv, duration);
    setName("");
    setColor(COLORS[(COLORS.indexOf(color) + 1) % COLORS.length]);
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-3xl mx-auto p-6 space-y-6">
        <header>
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <TrendingUp className="size-5 text-emerald-400" /> Habits
          </h1>
          <p className="text-sm text-gray-500 mt-1">Build streaks, track consistency, and drop habits onto your calendar — all on-device.</p>
        </header>

        {/* Add habit */}
        <div className="rounded-xl border border-white/10 bg-white/[0.02] p-3 space-y-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex items-center gap-1.5">
              {COLORS.map((c) => (
                <button
                  key={c}
                  onClick={() => setColor(c)}
                  className={clsx("size-5 rounded-full transition", color === c ? "ring-2 ring-white/70 ring-offset-2 ring-offset-[#0e1117]" : "opacity-70 hover:opacity-100")}
                  style={{ background: c }}
                  aria-label={`color ${c}`}
                />
              ))}
            </div>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && add()}
              placeholder="New habit, e.g. Read"
              className="flex-1 min-w-[160px] rounded-md bg-white/5 border border-white/10 px-3 py-1.5 text-sm outline-none focus:border-indigo-500/50"
            />
            <div className="flex items-center gap-1.5 rounded-md bg-white/5 border border-white/10 px-2 py-1">
              <input
                type="number"
                min={5}
                step={5}
                value={duration}
                onChange={(e) => setDuration(Math.max(5, Number(e.target.value) || 0))}
                className="w-14 bg-transparent text-sm outline-none text-right"
              />
              <span className="text-xs text-gray-500">min</span>
            </div>
            <button
              onClick={add}
              disabled={!name.trim()}
              className="flex items-center gap-1 text-sm px-3 py-1.5 rounded-md bg-indigo-500 hover:bg-indigo-400 disabled:opacity-40"
            >
              <Plus className="size-4" /> Add
            </button>
          </div>

          {/* Cadence */}
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <div className="flex rounded-md border border-white/10 overflow-hidden">
              {(["daily", "weekly", "interval"] as Cadence[]).map((c) => (
                <button
                  key={c}
                  onClick={() => setCadence(c)}
                  className={clsx("px-2.5 py-1 capitalize", cadence === c ? "bg-indigo-500/30 text-indigo-100" : "text-gray-400 hover:bg-white/5")}
                >
                  {c === "interval" ? "Every N days" : c === "weekly" ? "Days of week" : "Daily"}
                </button>
              ))}
            </div>
            {cadence === "weekly" && (
              <div className="flex gap-1">
                {WEEKDAYS.map((d) => (
                  <button
                    key={d.n}
                    onClick={() => toggleDay(d.n)}
                    className={clsx(
                      "size-6 rounded text-[11px]",
                      selDays.includes(d.n) ? "bg-indigo-500/30 text-indigo-100 border border-indigo-400/40" : "bg-white/5 text-gray-500 border border-white/10",
                    )}
                  >
                    {d.l}
                  </button>
                ))}
              </div>
            )}
            {cadence === "interval" && (
              <div className="flex items-center gap-1.5 text-gray-400">
                every
                <input
                  type="number"
                  min={2}
                  max={30}
                  value={intervalDays}
                  onChange={(e) => setIntervalDays(Math.min(30, Math.max(2, Number(e.target.value) || 2)))}
                  className="w-12 rounded-md bg-white/5 border border-white/10 px-2 py-0.5 text-right outline-none"
                />
                days
              </div>
            )}
          </div>
        </div>

        {habits.length === 0 ? (
          <div className="text-center text-sm text-gray-500 py-16 border border-dashed border-white/10 rounded-xl">
            No habits yet. Add one above to start a streak.
          </div>
        ) : (
          <div className="space-y-3">
            {habits.map((h) => (
              <HabitCard key={h.id} habit={h} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function HabitCard({ habit }: { habit: HabitStats }) {
  const toggleHabit = useStore((s) => s.toggleHabit);
  const deleteHabit = useStore((s) => s.deleteHabit);
  const updateHabit = useStore((s) => s.updateHabit);
  const setHabitScheduled = useStore((s) => s.setHabitScheduled);

  const [dur, setDur] = useState(habit.durationMinutes);
  const [pending, setPending] = useState(false);
  const onCalendar = habit.scheduledDays > 0;

  // Keep the local input in sync if the habit changes underneath us.
  useEffect(() => setDur(habit.durationMinutes), [habit.durationMinutes]);

  const commitDuration = () => {
    const d = Math.max(5, dur || 0);
    // Preserve the habit's cadence; only the duration changes here.
    if (d !== habit.durationMinutes) updateHabit(habit.id, habit.name, habit.color, habit.cadence, habit.days, habit.intervalDays, d);
    else setDur(habit.durationMinutes);
  };

  const toggleCalendar = async () => {
    setPending(true);
    try {
      await setHabitScheduled(habit.id, !onCalendar);
    } finally {
      setPending(false);
    }
  };

  return (
    <div className="rounded-xl border border-white/10 bg-white/[0.02] p-4">
      <div className="flex items-center gap-3">
        {/* Today toggle */}
        <button
          onClick={() => toggleHabit(habit.id)}
          title={habit.doneToday ? "Done today — click to undo" : "Mark done for today"}
          className={clsx(
            "size-9 shrink-0 rounded-full grid place-items-center border-2 transition",
            habit.doneToday ? "text-white" : "text-transparent hover:text-white/40",
          )}
          style={{ borderColor: habit.color, background: habit.doneToday ? habit.color : "transparent" }}
        >
          <Check className="size-5" />
        </button>

        <div className="min-w-0 flex-1">
          <div className="font-medium truncate flex items-center gap-2">
            {habit.name}
            <span className="text-[10px] font-normal px-1.5 py-0.5 rounded bg-white/8 text-gray-400 border border-white/10 shrink-0">{cadenceLabel(habit)}</span>
          </div>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-400 mt-0.5">
            <span className="flex items-center gap-1" title="Current streak">
              <Flame className={clsx("size-3.5", habit.currentStreak > 0 ? "text-orange-400" : "text-gray-600")} />
              {habit.currentStreak} {habit.currentStreak === 1 ? "time" : "times"}
            </span>
            <span className="flex items-center gap-1" title="Longest streak">
              <Trophy className="size-3.5 text-amber-400" /> {habit.longestStreak}
            </span>
            <span title="Completed (all time)">{habit.totalDone} total</span>
            <span title="Consistency over the last 30 days (scheduled days only)">{Math.round(habit.completionRate * 100)}% consistent</span>
          </div>
        </div>

        {/* Duration */}
        <div className="flex items-center gap-1.5 rounded-md bg-white/5 border border-white/10 px-2 py-1 shrink-0" title="Time per session, used when adding to the calendar">
          <input
            type="number"
            min={5}
            step={5}
            value={dur}
            onChange={(e) => setDur(Math.max(0, Number(e.target.value) || 0))}
            onBlur={commitDuration}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
            className="w-12 bg-transparent text-sm outline-none text-right"
          />
          <span className="text-xs text-gray-500">min</span>
        </div>

        {/* Calendar toggle — slots the habit into free space on each of its due days */}
        <button
          onClick={toggleCalendar}
          disabled={pending}
          title={
            onCalendar
              ? `On your calendar for ${habit.scheduledDays} day${habit.scheduledDays === 1 ? "" : "s"} — click to remove`
              : "Slot this habit into a free space on each of its scheduled days"
          }
          className={clsx(
            "flex items-center gap-1 text-xs px-2.5 py-1.5 rounded-md border transition shrink-0 disabled:opacity-50",
            onCalendar ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-300" : "border-white/10 text-gray-300 hover:bg-white/10",
          )}
        >
          {onCalendar ? <Check className="size-3.5" /> : <CalendarPlus className="size-3.5" />}
          {onCalendar ? `On calendar · ${habit.scheduledDays}d` : "Add to calendar"}
        </button>

        <button onClick={() => deleteHabit(habit.id)} className="text-gray-600 hover:text-rose-400 transition shrink-0" title="Delete habit">
          <Trash2 className="size-4" />
        </button>
      </div>

      <div className="mt-1 text-[11px] text-gray-600 pl-12">{humanMinutes(habit.durationMinutes)} per session · {cadenceLabel(habit).toLowerCase()}</div>

      <Heatmap habit={habit} />
    </div>
  );
}

/** GitHub-style consistency grid: 7 rows (Mon→Sun) flowing in week columns. Non-due days are dimmed. */
function Heatmap({ habit }: { habit: HabitStats }) {
  const toggleHabit = useStore((s) => s.toggleHabit);
  const days = habit.history;
  if (days.length === 0) return null;

  // Pad the first column so day 0 lands on its real weekday row.
  const lead = mondayIndex(parseLocal(days[0].day));

  return (
    <div className="mt-3 overflow-x-auto">
      {/* 7 rows (Mon→Sun) is not a default Tailwind class, so set the template inline. */}
      <div className="grid gap-[3px] w-max" style={{ gridTemplateRows: "repeat(7, 10px)", gridAutoFlow: "column", gridAutoColumns: "10px" }}>
        {Array.from({ length: lead }, (_, i) => (
          <div key={`pad${i}`} className="size-[10px]" />
        ))}
        {days.map((d) => (
          <button
            key={d.day}
            onClick={() => toggleHabit(habit.id, d.day)}
            title={`${d.day}${d.done ? " · done" : d.due ? "" : " · not scheduled"}`}
            className="size-[10px] rounded-[2px] transition hover:ring-1 hover:ring-white/50"
            style={{ background: d.done ? habit.color : d.due ? "rgba(255,255,255,0.10)" : "rgba(255,255,255,0.02)" }}
          />
        ))}
      </div>
    </div>
  );
}
