import { useEffect, useMemo, useState } from "react";
import { ArrowRight, CalendarDays, Clock, ListChecks, Loader2, Send } from "lucide-react";
import { useStore } from "../state/store";
import { api, type Briefing, type CalEvent, type PlanOutcome } from "../lib/ipc";
import { fmtTime, parseLocal, sameDay } from "../lib/time";

/** An all-day event runs midnight→midnight — kept out of the timed timeline. */
function isAllDay(e: CalEvent): boolean {
  const s = parseLocal(e.start);
  const en = parseLocal(e.end);
  return s.getHours() === 0 && s.getMinutes() === 0 && en.getHours() === 0 && en.getMinutes() === 0 && en.getTime() > s.getTime();
}

function greeting(d: Date): string {
  const h = d.getHours();
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

/** A one-line, human summary of what the planner just did (mirrors the chat/palette summaries). */
function summarize(o: PlanOutcome): string {
  const parts: string[] = [];
  if (o.createdEventTitles.length) parts.push(`added ${o.createdEventTitles.length} event${o.createdEventTitles.length === 1 ? "" : "s"}`);
  if (o.createdTaskIds.length) parts.push(`added ${o.createdTaskIds.length} task${o.createdTaskIds.length === 1 ? "" : "s"}`);
  if (o.createdHabitNames.length) parts.push(`added habit ${o.createdHabitNames.join(", ")}`);
  if (o.updatedEventTitles.length) parts.push(`updated ${[...new Set(o.updatedEventTitles)].join(", ")}`);
  if (o.removedEventTitles.length) parts.push(`removed ${o.removedEventTitles.length} event${o.removedEventTitles.length === 1 ? "" : "s"}`);
  if (o.clarifications.length && !parts.length) return o.clarifications[0];
  if (!parts.length) return "Nothing to change — try adding a bit more detail.";
  const s = parts.join(", ") + ", and re-planned your day.";
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** The calm landing surface: greeting + one plan-your-day input, then a glanceable read-only summary of
 *  today (agenda, what's due, focus booked). Everything else is one click away — this stays quiet on
 *  purpose (attention-restoration: the app opens into calm, not a wall of controls). */
export default function TodayPane() {
  const events = useStore((s) => s.events);
  const blocks = useStore((s) => s.blocks);
  const tasks = useStore((s) => s.tasks);
  const setView = useStore((s) => s.setView);
  const plan = useStore((s) => s.plan);
  const busy = useStore((s) => s.busy);
  const llm = useStore((s) => s.llm);

  const [now] = useState(() => new Date());
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  const [input, setInput] = useState("");
  const [planning, setPlanning] = useState(false);
  const [result, setResult] = useState<string | null>(null);

  const taskById = useMemo(() => new Map(tasks.map((t) => [t.id, t])), [tasks]);

  // Today's timeline = timed events + scheduled task blocks, merged and time-ordered.
  const timeline = useMemo(() => {
    const evs = events
      .filter((e) => !isAllDay(e) && sameDay(parseLocal(e.start), now))
      .map((e) => ({ key: `e${e.id}`, start: parseLocal(e.start), title: e.title, kind: e.kind === "habit" ? ("habit" as const) : ("event" as const) }));
    const blk = blocks
      .filter((b) => sameDay(parseLocal(b.start), now))
      .map((b) => ({ key: `b${b.id}`, start: parseLocal(b.start), title: taskById.get(b.taskId)?.title ?? "Task", kind: "task" as const }));
    return [...evs, ...blk].sort((a, z) => a.start.getTime() - z.start.getTime());
  }, [events, blocks, taskById, now]);

  // Refresh the briefing whenever today's data could have changed (tasks/events get a new identity
  // after every mutation). Cheap local DB read; a failure just leaves the counts hidden.
  useEffect(() => {
    let cancelled = false;
    Promise.resolve()
      .then(() => api.dailyBriefing())
      .then((b) => !cancelled && setBriefing(b))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [tasks, events]);

  const focus = briefing ? (briefing.focusMinutes >= 60 ? `${(briefing.focusMinutes / 60).toFixed(1)}h` : `${briefing.focusMinutes}m`) : null;
  const due = briefing?.dueTasks ?? [];

  const submit = async () => {
    const text = input.trim();
    if (!text || planning || busy) return;
    setPlanning(true);
    setResult(null);
    await useStore.getState().wakeAi();
    try {
      const o = await plan(text, []);
      setResult(summarize(o));
      setInput("");
    } catch {
      setResult("Couldn't plan that — is the AI set up?");
    } finally {
      setPlanning(false);
    }
  };

  const dateLabel = now.toLocaleDateString([], { weekday: "long", month: "long", day: "numeric" });
  const kindTag: Record<string, string> = { event: "event", task: "task", habit: "habit" };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="welcome-in max-w-2xl mx-auto px-6 py-14 sm:py-20">
        {/* Greeting */}
        <div className="mb-8">
          <h1 className="text-3xl font-semibold tracking-tight">{greeting(now)}</h1>
          <p className="tnum mt-1 text-sm text-[var(--ink-faint)]">{dateLabel}</p>
        </div>

        {/* Plan-your-day input — the single primary action on this surface. */}
        <form
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
          className="flex items-center gap-2 border border-white/15 bg-white/[0.03] px-3 focus-within:border-white/30"
        >
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder={llm?.reachable ? "What does your day look like?" : "Set up the AI in Settings to plan…"}
            disabled={!llm?.reachable}
            className="flex-1 bg-transparent py-3 text-sm outline-none disabled:opacity-60"
          />
          <button
            type="submit"
            disabled={!input.trim() || planning || busy}
            title="Plan it"
            className="shrink-0 grid size-8 place-items-center bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40"
          >
            {planning ? <Loader2 className="size-4 animate-spin" /> : <Send className="size-4" />}
          </button>
        </form>
        {result && <p className="mt-2 text-xs text-[var(--ink-muted)]">{result}</p>}

        {/* Today at a glance */}
        <section className="mt-10">
          <div className="flex items-center gap-3 border-b border-white/10 pb-2">
            <h2 className="text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)]">Today</h2>
            {briefing && (
              <span className="tnum flex items-center gap-3 text-[11px] text-[var(--ink-muted)]">
                <span className="flex items-center gap-1"><CalendarDays className="size-3" />{timeline.length}</span>
                <span className="flex items-center gap-1"><ListChecks className="size-3" />{due.length} due</span>
                {briefing.focusMinutes > 0 && <span className="flex items-center gap-1"><Clock className="size-3" />{focus} focus</span>}
              </span>
            )}
          </div>

          {timeline.length === 0 ? (
            <p className="py-8 text-center text-sm text-[var(--ink-faint)]">Nothing scheduled today. Plan something above, or enjoy the open space.</p>
          ) : (
            <ul className="mt-1 divide-y divide-white/5">
              {timeline.map((it) => (
                <li key={it.key} className="flex items-center gap-3 py-2.5">
                  <span className="tnum w-16 shrink-0 text-xs text-[var(--ink-muted)]">{fmtTime(it.start)}</span>
                  <span className="min-w-0 flex-1 truncate text-sm">{it.title}</span>
                  <span className="shrink-0 text-[10px] uppercase tracking-wide text-[var(--ink-faint)]">{kindTag[it.kind]}</span>
                </li>
              ))}
            </ul>
          )}

          {due.length > 0 && (
            <div className="mt-4">
              <div className="text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)] mb-1.5">Due</div>
              <div className="flex flex-wrap gap-1.5">
                {due.slice(0, 8).map((t) => (
                  <span key={t.id} className="inline-flex items-center border border-amber-500/30 bg-amber-500/10 px-2 py-0.5 text-[11px] text-amber-200">
                    {t.title}
                  </span>
                ))}
                {due.length > 8 && <span className="tnum px-1 text-[11px] text-[var(--ink-faint)]">+{due.length - 8} more</span>}
              </div>
            </div>
          )}
        </section>

        {/* Into the detail — progressive disclosure: the full calendar is one click away. */}
        <button
          onClick={() => setView("calendar")}
          className="group mt-8 inline-flex items-center gap-1.5 text-sm text-[var(--ink-muted)] hover:text-white"
        >
          Open calendar
          <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
        </button>
      </div>
    </div>
  );
}
