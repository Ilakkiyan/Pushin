import { useEffect, useState } from "react";
import { Check, ChevronRight, Plus, Trash2, NotebookPen, Play, Square, RotateCcw } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type FocusSession, type Task } from "../lib/ipc";
import { humanMinutes, parseLocal } from "../lib/time";
import LabelPicker from "../components/LabelPicker";

/** mm:ss for an elapsed-seconds count. */
function fmtElapsed(sec: number): string {
  const m = Math.floor(sec / 60);
  return `${m}:${String(sec % 60).padStart(2, "0")}`;
}

/** Remembers whether the done bin is open. Same `pushin:` prefix as the other UI preferences. */
const DONE_OPEN_KEY = "pushin:taskListDoneOpen";

const PRIORITY: Record<number, { label: string; cls: string }> = {
  1: { label: "Low", cls: "text-gray-400 bg-gray-400/10" },
  2: { label: "Med", cls: "text-sky-300 bg-sky-400/10" },
  3: { label: "High", cls: "text-orange-300 bg-orange-400/10" },
  4: { label: "Urgent", cls: "text-rose-300 bg-rose-400/10" },
};

function TaskRow({ task, active, now, onStart, onStop }: { task: Task; active: FocusSession | null; now: number; onStart: (id: number) => void; onStop: () => void }) {
  const projects = useStore((s) => s.projects);
  const setTaskStatus = useStore((s) => s.setTaskStatus);
  const deleteTask = useStore((s) => s.deleteTask);
  const openEntityNote = useStore((s) => s.openEntityNote);
  const project = projects.find((p) => p.id === task.projectId);
  const done = task.status === "done";
  const pr = PRIORITY[task.priority] ?? PRIORITY[2];
  const focusing = active?.taskId === task.id;
  const elapsed = focusing ? Math.max(0, Math.floor((now - parseLocal(active!.start).getTime()) / 1000)) : 0;

  return (
    <div className="group flex items-start gap-2.5 px-3 py-2.5 hoverable">
      <button
        aria-label={done ? "Mark not done" : "Mark done"}
        onClick={() => setTaskStatus(task.id, done ? "todo" : "done")}
        className={clsx(
          "mt-0.5 size-4 shrink-0 rounded border grid place-items-center transition",
          done ? "bg-emerald-500 border-emerald-500" : "border-white/25 hover:border-white/50",
        )}
      >
        {done && <Check className="size-3 text-white" />}
      </button>

      <div className="min-w-0 flex-1">
        {/* line 1: title · priority */}
        <div className="flex items-center gap-2">
          {project && <span className="size-2 rounded-full shrink-0" style={{ background: project.color }} title={project.name} />}
          <span className={clsx("min-w-0 flex-1 truncate text-sm", done && "line-through text-gray-500")}>{task.title}</span>
          <span className={clsx("shrink-0 rounded px-1.5 py-0.5 text-[10px]", pr.cls)}>{pr.label}</span>
        </div>
        {/* line 2: meta · labels */}
        <div className="tnum mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px] text-[var(--ink-faint)]">
          <span>{humanMinutes(task.estimatedMinutes)}</span>
          {task.deadline && <span>· due {parseLocal(task.deadline).toLocaleDateString([], { month: "short", day: "numeric" })}</span>}
          {task.dependsOn.length > 0 && <span>· {task.dependsOn.length} dep</span>}
          {!done && task.missedCount > 0 && (
            <span
              title={`Its planned time passed unfinished ${task.missedCount} time${task.missedCount === 1 ? "" : "s"}. Each one moved it to the next free slot`}
              className="inline-flex shrink-0 items-center gap-1 rounded bg-amber-400/10 px-1.5 py-0.5 text-[10px] text-amber-300"
            >
              <RotateCcw className="size-2.5" />
              {task.missedCount}×
            </span>
          )}
          <LabelPicker kind="task" entityId={task.id} compact revealOnHover />
        </div>
      </div>

      {/* right rail: focus timer (always when running) + hover actions */}
      <div className="flex shrink-0 items-center gap-1.5 self-center text-gray-500">
        {focusing ? (
          <button onClick={onStop} title="Stop focus" className="tnum flex items-center gap-1 text-[11px] text-emerald-300">
            <Square className="size-3 fill-current" />
            {fmtElapsed(elapsed)}
          </button>
        ) : (
          !done && (
            <button onClick={() => onStart(task.id)} title="Start a focus session" className="opacity-0 group-hover:opacity-100 hover:text-emerald-300 transition-opacity">
              <Play className="size-3.5" />
            </button>
          )
        )}
        <button
          onClick={() => openEntityNote("task", task.id, task.title)}
          title="Open notes for this task"
          className="opacity-0 group-hover:opacity-100 hover:text-indigo-300 transition-opacity"
        >
          <NotebookPen className="size-3.5" />
        </button>
        <button
          aria-label="Delete task"
          onClick={() => deleteTask(task.id)}
          className="opacity-0 group-hover:opacity-100 hover:text-rose-400 transition-opacity"
        >
          <Trash2 className="size-3.5" />
        </button>
      </div>
    </div>
  );
}

export default function TaskListPane() {
  const tasks = useStore((s) => s.tasks);
  const createTask = useStore((s) => s.createTask);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [minutes, setMinutes] = useState(60);
  const [focus, setFocus] = useState<FocusSession | null>(null);
  const [now, setNow] = useState(Date.now());
  // The done bin only grows, so it starts collapsed and remembers the choice — an archive you have
  // to scroll past is worse than one you open when you want it. Read lazily and guarded: a private
  // window or blocked site data throws on access, and this is not worth crashing the task list over.
  const [doneOpen, setDoneOpen] = useState(() => {
    try {
      return localStorage.getItem(DONE_OPEN_KEY) === "1";
    } catch {
      return false;
    }
  });
  useEffect(() => {
    try {
      localStorage.setItem(DONE_OPEN_KEY, doneOpen ? "1" : "0");
    } catch {
      /* not worth surfacing — the bin just forgets between sessions */
    }
  }, [doneOpen]);

  // Load any in-progress focus session on mount (e.g. after a navigation). Defensive against a
  // missing api method (older test mocks) so the task list never crashes.
  useEffect(() => {
    let cancelled = false;
    Promise.resolve()
      .then(() => api.activeFocus())
      .then((s) => !cancelled && setFocus(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // Tick the elapsed clock while a session is running.
  useEffect(() => {
    if (!focus) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [focus]);

  const startFocus = async (taskId: number) => {
    setNow(Date.now());
    setFocus(await api.startFocus(taskId).catch(() => null));
  };
  const stopFocus = async () => {
    if (focus) await api.stopFocus(focus.id).catch(() => {});
    setFocus(null);
  };

  const active = tasks.filter((t) => t.status !== "done");
  const done = tasks.filter((t) => t.status === "done");

  const add = async () => {
    if (!title.trim()) return;
    await createTask(title.trim(), minutes, null, 2);
    setTitle("");
    setMinutes(60);
    setAdding(false);
  };

  return (
    <div className="h-full flex flex-col">
      <div className="px-4 py-3 border-b border-white/10 flex items-center justify-between shrink-0">
        <span className="text-sm font-medium">Tasks <span className="tnum text-[var(--ink-faint)]">· {active.length}</span></span>
        <button onClick={() => setAdding((v) => !v)} title="Add a task" className="p-1 hoverable text-[var(--ink-muted)] hover:text-white">
          <Plus className="size-4" />
        </button>
      </div>

      {adding && (
        <div className="p-3 border-b border-white/10 space-y-2 shrink-0">
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && add()}
            placeholder="Task title"
            className="w-full bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-white/30"
          />
          <div className="flex items-center gap-2">
            <input
              type="number"
              value={minutes}
              min={15}
              step={15}
              onChange={(e) => setMinutes(Number(e.target.value))}
              className="tnum w-20 bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-white/30"
            />
            <span className="text-xs text-[var(--ink-faint)]">minutes</span>
            <button onClick={add} className="ml-auto text-xs px-3 py-1.5 bg-white/90 hover:bg-white text-gray-900 font-medium">
              Add
            </button>
          </div>
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto py-1">
        {active.length === 0 && !adding && (
          <p className="text-xs text-gray-500 px-4 py-6 text-center">No tasks yet. Plan something with the AI above.</p>
        )}
        {active.map((t) => (
          <TaskRow key={t.id} task={t} active={focus} now={now} onStart={startFocus} onStop={stopFocus} />
        ))}
        {done.length > 0 && (
          <div className="mt-2 pt-2 border-t border-white/5">
            <button
              onClick={() => setDoneOpen((o) => !o)}
              aria-expanded={doneOpen}
              className="flex w-full items-center gap-1.5 px-4 py-1 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)] hover:text-[var(--ink-muted)]"
            >
              <ChevronRight className={clsx("size-3 shrink-0 transition-transform", doneOpen && "rotate-90")} />
              <span>Done</span>
              <span className="tnum font-normal tracking-normal">· {done.length}</span>
            </button>
            {doneOpen &&
              done.map((t) => (
                <TaskRow key={t.id} task={t} active={focus} now={now} onStart={startFocus} onStop={stopFocus} />
              ))}
          </div>
        )}
      </div>
    </div>
  );
}
