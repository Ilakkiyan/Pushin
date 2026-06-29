import { useMemo, useState } from "react";
import { Check, CheckCircle2, ChevronDown, FolderKanban, Plane, Plus, RotateCcw, Trash2 } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { CalEvent, Project, Task } from "../lib/ipc";
import { humanMinutes, parseLocal } from "../lib/time";
import LabelPicker from "../components/LabelPicker";

const PRIORITY: Record<number, { label: string; cls: string }> = {
  1: { label: "Low", cls: "text-gray-400 bg-gray-400/10" },
  2: { label: "Med", cls: "text-sky-300 bg-sky-400/10" },
  3: { label: "High", cls: "text-orange-300 bg-orange-400/10" },
  4: { label: "Urgent", cls: "text-rose-300 bg-rose-400/10" },
};

/** All-day events spanning 2+ calendar days (trips / multi-day commitments). */
function isMultiDay(e: CalEvent): boolean {
  const s = parseLocal(e.start);
  const en = parseLocal(e.end);
  const midnight = s.getHours() === 0 && s.getMinutes() === 0 && en.getHours() === 0 && en.getMinutes() === 0;
  return midnight && (en.getTime() - s.getTime()) / 86_400_000 >= 2;
}

const fmtDate = (d: Date) => d.toLocaleDateString([], { month: "short", day: "numeric" });

/** Active tasks first (priority desc, then deadline), done tasks last. */
function sortTasks(tasks: Task[]): Task[] {
  return [...tasks].sort((a, b) => {
    const ad = a.status === "done" ? 1 : 0;
    const bd = b.status === "done" ? 1 : 0;
    if (ad !== bd) return ad - bd;
    if (a.priority !== b.priority) return b.priority - a.priority;
    const at = a.deadline ? parseLocal(a.deadline).getTime() : Infinity;
    const bt = b.deadline ? parseLocal(b.deadline).getTime() : Infinity;
    return at - bt;
  });
}

export default function ProjectsPane() {
  const projects = useStore((s) => s.projects);
  const tasks = useStore((s) => s.tasks);
  const events = useStore((s) => s.events);
  const [showCompleted, setShowCompleted] = useState(false);

  const activeProjects = useMemo(() => projects.filter((p) => !p.archivedAt), [projects]);
  const completedProjects = useMemo(() => projects.filter((p) => p.archivedAt), [projects]);

  const trips = useMemo(
    () => events.filter(isMultiDay).sort((a, b) => parseLocal(a.start).getTime() - parseLocal(b.start).getTime()),
    [events],
  );

  const tasksByProject = useMemo(() => {
    const m = new Map<number | null, Task[]>();
    for (const t of tasks) {
      const k = t.projectId ?? null;
      const list = m.get(k) ?? [];
      list.push(t);
      m.set(k, list);
    }
    return m;
  }, [tasks]);

  const unassigned = tasksByProject.get(null) ?? [];
  const hasAnything = trips.length > 0 || projects.length > 0 || tasks.length > 0;

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-3xl mx-auto p-4 sm:p-6 space-y-6">
        <header>
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <FolderKanban className="size-5 text-indigo-400" /> Projects
          </h1>
          <p className="text-sm text-gray-500 mt-1">Your multi-day plans and long-running projects, broken into subtasks.</p>
        </header>

        {!hasAnything && (
          <div className="text-center text-sm text-gray-500 py-16 border border-dashed border-white/10 rounded-xl">
            Nothing here yet. Describe a project in the chat (e.g. “launch a blog: pick a platform, write 3 posts”)
            and it’ll show up here with its subtasks.
          </div>
        )}

        {/* Multi-day events / trips */}
        {trips.length > 0 && (
          <section className="space-y-2">
            <h2 className="text-xs uppercase tracking-wide text-gray-500 flex items-center gap-1.5">
              <Plane className="size-3.5" /> Trips & multi-day
            </h2>
            <div className="space-y-2">
              {trips.map((e) => {
                const s = parseLocal(e.start);
                const last = new Date(parseLocal(e.end).getTime() - 86_400_000); // inclusive last day
                const days = Math.round((parseLocal(e.end).getTime() - s.getTime()) / 86_400_000);
                return (
                  <div key={e.id} className="rounded-xl px-4 py-3 flex items-center gap-3 transition-colors hover:bg-white/[0.025]">
                    <span className="size-2 rounded-full bg-rose-400 shrink-0" />
                    <div className="min-w-0 flex-1">
                      <div className="text-sm font-medium truncate">{e.title}</div>
                      <div className="text-xs text-gray-500">
                        {fmtDate(s)} – {fmtDate(last)} · {days} days
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </section>
        )}

        {/* Active projects with their subtasks */}
        {activeProjects.map((p) => (
          <ProjectCard key={p.id} project={p} tasks={tasksByProject.get(p.id) ?? []} />
        ))}

        {/* Tasks not attached to a project */}
        {unassigned.length > 0 && <ProjectCard project={null} tasks={unassigned} />}

        {/* Completed bin — collapsed by default, holds finished projects */}
        {completedProjects.length > 0 && (
          <section className="space-y-2">
            <button
              onClick={() => setShowCompleted((v) => !v)}
              className="text-xs uppercase tracking-wide text-gray-500 hover:text-gray-300 flex items-center gap-1.5"
            >
              <CheckCircle2 className="size-3.5" /> Completed
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-white/10 normal-case tracking-normal">
                {completedProjects.length}
              </span>
              <ChevronDown className={clsx("size-3.5 transition-transform", !showCompleted && "-rotate-90")} />
            </button>
            {showCompleted && (
              <div className="space-y-2">
                {completedProjects.map((p) => (
                  <ProjectCard key={p.id} project={p} tasks={tasksByProject.get(p.id) ?? []} archived />
                ))}
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}

function ProjectCard({ project, tasks, archived = false }: { project: Project | null; tasks: Task[]; archived?: boolean }) {
  const createTask = useStore((s) => s.createTask);
  const deleteProject = useStore((s) => s.deleteProject);
  const setProjectArchived = useStore((s) => s.setProjectArchived);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [confirmDel, setConfirmDel] = useState(false);
  const isReal = project !== null;

  const sorted = useMemo(() => sortTasks(tasks), [tasks]);
  const total = tasks.length;
  const done = tasks.filter((t) => t.status === "done").length;
  const pct = total ? Math.round((done / total) * 100) : 0;
  const remainingMin = tasks.filter((t) => t.status !== "done").reduce((s, t) => s + t.estimatedMinutes, 0);
  const nextDue = tasks
    .filter((t) => t.status !== "done" && t.deadline)
    .map((t) => parseLocal(t.deadline!))
    .sort((a, b) => a.getTime() - b.getTime())[0];

  const color = project?.color ?? "#64748b";

  const add = async () => {
    const n = title.trim();
    if (!n) return;
    await createTask(n, 60, null, 2, project?.id ?? null);
    setTitle("");
  };

  const onDelete = () => {
    if (!isReal) return;
    if (total === 0) deleteProject(project.id);
    else setConfirmDel(true);
  };

  return (
    <section className={clsx("group rounded-xl overflow-hidden transition-colors hover:bg-white/[0.025]", archived && "opacity-60")}>
      <div className="px-4 pt-3 pb-2">
        <div className="flex items-center gap-2">
          <span className="size-2.5 rounded-full shrink-0" style={{ background: color }} />
          <h2 className="font-medium truncate flex-1">{project ? project.name.trim() || "Untitled project" : "No project"}</h2>
          <span className="text-xs text-gray-500 shrink-0">
            {done}/{total} done
          </span>
          {/* Action buttons stay hidden until the card is hovered (or a button is focused). */}
          <div className="flex items-center gap-1 shrink-0 opacity-0 transition-opacity duration-150 group-hover:opacity-100 focus-within:opacity-100">
            {(!isReal || !archived) && (
              <button onClick={() => setAdding((v) => !v)} title="Add subtask" className="text-gray-500 hover:text-white">
                <Plus className="size-4" />
              </button>
            )}
            {isReal && !archived && (
              <button onClick={() => setProjectArchived(project.id, true)} title="Mark project complete" className="text-gray-500 hover:text-emerald-400">
                <Check className="size-4" />
              </button>
            )}
            {isReal && archived && (
              <button onClick={() => setProjectArchived(project.id, false)} title="Restore project" className="text-gray-500 hover:text-white">
                <RotateCcw className="size-4" />
              </button>
            )}
            {isReal && (
              <button onClick={onDelete} title="Delete project" className="text-gray-500 hover:text-rose-400">
                <Trash2 className="size-4" />
              </button>
            )}
          </div>
        </div>
        {isReal && project && (
          <div className="mt-1.5">
            <LabelPicker kind="project" entityId={project.id} compact />
          </div>
        )}
        {/* Progress bar */}
        <div className="mt-2 h-1.5 rounded-full bg-white/10 overflow-hidden">
          <div className="h-full rounded-full transition-all" style={{ width: `${pct}%`, background: color }} />
        </div>
        <div className="mt-1.5 text-[11px] text-gray-500 flex items-center gap-3 flex-wrap">
          {remainingMin > 0 && <span>{humanMinutes(remainingMin)} of work left</span>}
          {nextDue && <span>next due {fmtDate(nextDue)}</span>}
          {remainingMin === 0 && total > 0 && <span className="text-emerald-400">all done 🎉</span>}
        </div>
      </div>

      {confirmDel && isReal && (
        <div className="px-4 pb-2 flex items-center gap-2 text-xs text-gray-400 flex-wrap">
          <span>
            Delete “{project.name.trim() || "Untitled project"}”? Its {total} task{total === 1 ? "" : "s"} will move to No project.
          </span>
          <button onClick={() => deleteProject(project.id)} className="px-2 py-1 rounded-md bg-rose-500/90 hover:bg-rose-500 text-white">
            Delete
          </button>
          <button onClick={() => setConfirmDel(false)} className="px-2 py-1 rounded-md bg-white/10 hover:bg-white/20">
            Cancel
          </button>
        </div>
      )}

      {adding && (
        <div className="px-4 pb-2 flex items-center gap-2">
          <input
            autoFocus
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") add();
              if (e.key === "Escape") setAdding(false);
            }}
            placeholder="New subtask…"
            className="flex-1 rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50"
          />
          <button onClick={add} disabled={!title.trim()} className="text-xs px-3 py-1.5 rounded-md bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40">
            Add
          </button>
        </div>
      )}

      <div className="border-t border-white/5 py-1">
        {sorted.map((t) => (
          <SubtaskRow key={t.id} task={t} />
        ))}
      </div>
    </section>
  );
}

function SubtaskRow({ task }: { task: Task }) {
  const setTaskStatus = useStore((s) => s.setTaskStatus);
  const deleteTask = useStore((s) => s.deleteTask);
  const done = task.status === "done";
  const pr = PRIORITY[task.priority] ?? PRIORITY[2];

  return (
    <div className="group flex items-center gap-2 px-4 py-2 hover:bg-white/[0.03]">
      <button
        onClick={() => setTaskStatus(task.id, done ? "todo" : "done")}
        className={clsx(
          "size-4 shrink-0 rounded border grid place-items-center",
          done ? "bg-emerald-500 border-emerald-500" : "border-white/25 hover:border-white/50",
        )}
      >
        {done && <Check className="size-3 text-white" />}
      </button>

      <div className="min-w-0 flex-1">
        <div className={clsx("text-sm truncate", done && "line-through text-gray-500")}>{task.title}</div>
        <div className="flex items-center gap-2 text-[11px] text-gray-500">
          <span>{humanMinutes(task.estimatedMinutes)}</span>
          {task.status === "scheduled" && <span className="text-indigo-300/80">· scheduled</span>}
          {task.deadline && <span>· due {fmtDate(parseLocal(task.deadline))}</span>}
          {task.dependsOn.length > 0 && <span>· {task.dependsOn.length} dep</span>}
        </div>
      </div>

      <span className={clsx("text-[10px] px-1.5 py-0.5 rounded shrink-0", pr.cls)}>{pr.label}</span>
      <button onClick={() => deleteTask(task.id)} className="opacity-0 group-hover:opacity-100 text-gray-500 hover:text-rose-400 shrink-0">
        <Trash2 className="size-3.5" />
      </button>
    </div>
  );
}
