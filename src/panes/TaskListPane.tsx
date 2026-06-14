import { useState } from "react";
import { Check, Plus, Trash2, NotebookPen } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { Task } from "../lib/ipc";
import { humanMinutes, parseLocal } from "../lib/time";
import LabelPicker from "../components/LabelPicker";

const PRIORITY: Record<number, { label: string; cls: string }> = {
  1: { label: "Low", cls: "text-gray-400 bg-gray-400/10" },
  2: { label: "Med", cls: "text-sky-300 bg-sky-400/10" },
  3: { label: "High", cls: "text-orange-300 bg-orange-400/10" },
  4: { label: "Urgent", cls: "text-rose-300 bg-rose-400/10" },
};

function TaskRow({ task }: { task: Task }) {
  const projects = useStore((s) => s.projects);
  const setTaskStatus = useStore((s) => s.setTaskStatus);
  const deleteTask = useStore((s) => s.deleteTask);
  const openEntityNote = useStore((s) => s.openEntityNote);
  const project = projects.find((p) => p.id === task.projectId);
  const done = task.status === "done";
  const pr = PRIORITY[task.priority] ?? PRIORITY[2];

  return (
    <div className="group flex items-center gap-2 px-3 py-2 hover:bg-white/[0.03] rounded-lg">
      <button
        aria-label={done ? "Mark not done" : "Mark done"}
        onClick={() => setTaskStatus(task.id, done ? "todo" : "done")}
        className={clsx(
          "size-4 shrink-0 rounded border grid place-items-center",
          done ? "bg-emerald-500 border-emerald-500" : "border-white/25 hover:border-white/50",
        )}
      >
        {done && <Check className="size-3 text-white" />}
      </button>

      {project && <span className="size-2 rounded-full shrink-0" style={{ background: project.color }} />}

      <div className="min-w-0 flex-1">
        <div className={clsx("text-sm truncate", done && "line-through text-gray-500")}>{task.title}</div>
        <div className="flex items-center gap-2 text-[11px] text-gray-500">
          <span>{humanMinutes(task.estimatedMinutes)}</span>
          {task.deadline && <span>· due {parseLocal(task.deadline).toLocaleDateString([], { month: "short", day: "numeric" })}</span>}
          {task.dependsOn.length > 0 && <span>· {task.dependsOn.length} dep</span>}
        </div>
        <div className="mt-0.5">
          <LabelPicker kind="task" entityId={task.id} compact />
        </div>
      </div>

      <span className={clsx("text-[10px] px-1.5 py-0.5 rounded shrink-0", pr.cls)}>{pr.label}</span>
      <button
        onClick={() => openEntityNote("task", task.id, task.title)}
        title="Open notes for this task"
        className="opacity-0 group-hover:opacity-100 text-gray-500 hover:text-indigo-300 shrink-0"
      >
        <NotebookPen className="size-3.5" />
      </button>
      <button
        aria-label="Delete task"
        onClick={() => deleteTask(task.id)}
        className="opacity-0 group-hover:opacity-100 text-gray-500 hover:text-rose-400 shrink-0"
      >
        <Trash2 className="size-3.5" />
      </button>
    </div>
  );
}

export default function TaskListPane() {
  const tasks = useStore((s) => s.tasks);
  const createTask = useStore((s) => s.createTask);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [minutes, setMinutes] = useState(60);

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
        <span className="text-sm font-medium">Tasks <span className="text-gray-500">· {active.length}</span></span>
        <button onClick={() => setAdding((v) => !v)} className="text-gray-400 hover:text-white">
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
            className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50"
          />
          <div className="flex items-center gap-2">
            <input
              type="number"
              value={minutes}
              min={15}
              step={15}
              onChange={(e) => setMinutes(Number(e.target.value))}
              className="w-20 rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none"
            />
            <span className="text-xs text-gray-500">minutes</span>
            <button onClick={add} className="ml-auto text-xs px-3 py-1.5 rounded-md bg-indigo-500 hover:bg-indigo-400">
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
          <TaskRow key={t.id} task={t} />
        ))}
        {done.length > 0 && (
          <div className="mt-2 pt-2 border-t border-white/5">
            <div className="px-4 py-1 text-[11px] uppercase tracking-wide text-gray-600">Done</div>
            {done.map((t) => (
              <TaskRow key={t.id} task={t} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
