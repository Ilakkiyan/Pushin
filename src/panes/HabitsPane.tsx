import { useEffect, useRef, useState } from "react";
import { CalendarPlus, Check, Flame, Plus, Trash2, TrendingUp, Trophy } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { HabitStats } from "../lib/ipc";
import { humanMinutes, mondayIndex, parseLocal } from "../lib/time";

const COLORS = ["#22c55e", "#0ea5e9", "#a855f7", "#f59e0b", "#ef4444", "#ec4899", "#14b8a6", "#6366f1"];

export default function HabitsPane() {
  const habits = useStore((s) => s.habits);
  const loadHabits = useStore((s) => s.loadHabits);
  const createHabit = useStore((s) => s.createHabit);

  const [name, setName] = useState("");
  const [color, setColor] = useState(COLORS[0]);
  const [duration, setDuration] = useState(30);

  useEffect(() => {
    loadHabits();
  }, [loadHabits]);

  const add = () => {
    const n = name.trim();
    if (!n) return;
    createHabit(n, color, duration);
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
        <div className="rounded-xl border border-white/10 bg-white/[0.02] p-3 flex flex-wrap items-center gap-2">
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
  const scheduleHabit = useStore((s) => s.scheduleHabit);

  const [dur, setDur] = useState(habit.durationMinutes);
  const [scheduled, setScheduled] = useState(false);
  const timer = useRef<number | null>(null);

  // Keep the local input in sync if the habit changes underneath us.
  useEffect(() => setDur(habit.durationMinutes), [habit.durationMinutes]);
  useEffect(() => () => { if (timer.current) window.clearTimeout(timer.current); }, []);

  const commitDuration = () => {
    const d = Math.max(5, dur || 0);
    if (d !== habit.durationMinutes) updateHabit(habit.id, habit.name, habit.color, d);
    else setDur(habit.durationMinutes);
  };

  const addToCalendar = async () => {
    await scheduleHabit(habit.id);
    setScheduled(true);
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setScheduled(false), 2500);
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
          <div className="font-medium truncate">{habit.name}</div>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-gray-400 mt-0.5">
            <span className="flex items-center gap-1" title="Current streak">
              <Flame className={clsx("size-3.5", habit.currentStreak > 0 ? "text-orange-400" : "text-gray-600")} />
              {habit.currentStreak} day{habit.currentStreak === 1 ? "" : "s"}
            </span>
            <span className="flex items-center gap-1" title="Longest streak">
              <Trophy className="size-3.5 text-amber-400" /> {habit.longestStreak}
            </span>
            <span title="Completed days (all time)">{habit.totalDone} total</span>
            <span title="Consistency over the last 30 days">{Math.round(habit.completionRate * 100)}% consistent</span>
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

        {/* Add to calendar (today) */}
        <button
          onClick={addToCalendar}
          title="Slot this habit into a free space on today's calendar"
          className={clsx(
            "flex items-center gap-1 text-xs px-2.5 py-1.5 rounded-md border transition shrink-0",
            scheduled
              ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-300"
              : "border-white/10 text-gray-300 hover:bg-white/10",
          )}
        >
          {scheduled ? <Check className="size-3.5" /> : <CalendarPlus className="size-3.5" />}
          {scheduled ? "Added" : "Add to today"}
        </button>

        <button onClick={() => deleteHabit(habit.id)} className="text-gray-600 hover:text-rose-400 transition shrink-0" title="Delete habit">
          <Trash2 className="size-4" />
        </button>
      </div>

      <div className="mt-1 text-[11px] text-gray-600 pl-12">{humanMinutes(habit.durationMinutes)} per session</div>

      <Heatmap habit={habit} />
    </div>
  );
}

/** GitHub-style consistency grid: 7 rows (Mon→Sun) flowing in week columns. */
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
            title={`${d.day}${d.done ? " · done" : ""}`}
            className="size-[10px] rounded-[2px] transition hover:ring-1 hover:ring-white/50"
            style={{ background: d.done ? habit.color : "rgba(255,255,255,0.06)" }}
          />
        ))}
      </div>
    </div>
  );
}
