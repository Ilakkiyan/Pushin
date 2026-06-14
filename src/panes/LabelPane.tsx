import { useEffect, useMemo, useState } from "react";
import { CheckSquare, CalendarDays, Flame, FileText, FolderKanban, Pencil, Trash2, Clock } from "lucide-react";
import { useStore } from "../state/store";
import { api, type EntityRef, type LabelInput } from "../lib/ipc";

/** The cross-cutting filtered view for one label: everything tagged it (tasks/events/habits/pages/
 *  projects), plus an inline editor (rename/recolor/group + scheduling prefs/delete). */
export default function LabelPane() {
  const labelId = useStore((s) => s.currentLabelId);
  const labels = useStore((s) => s.labels);
  const tasks = useStore((s) => s.tasks);
  const events = useStore((s) => s.events);
  const habits = useStore((s) => s.habits);
  const pages = useStore((s) => s.pages);
  const projects = useStore((s) => s.projects);
  const updateLabel = useStore((s) => s.updateLabel);
  const deleteLabel = useStore((s) => s.deleteLabel);
  const setView = useStore((s) => s.setView);
  const openPage = useStore((s) => s.openPage);

  const [refs, setRefs] = useState<EntityRef[]>([]);
  const [editing, setEditing] = useState(false);

  const label = useMemo(() => labels.find((l) => l.id === labelId), [labels, labelId]);

  useEffect(() => {
    if (labelId == null) return;
    api.entitiesForLabel(labelId).then(setRefs).catch(() => setRefs([]));
  }, [labelId]);

  if (!label) {
    return <div className="h-full grid place-items-center text-gray-500 text-sm">Select a label.</div>;
  }

  const resolve = (r: EntityRef): { title: string; icon: React.ReactNode; onClick: () => void } | null => {
    switch (r.kind) {
      case "task": {
        const t = tasks.find((x) => x.id === r.id);
        return t ? { title: t.title, icon: <CheckSquare className="size-3.5 text-emerald-400/70" />, onClick: () => setView("calendar") } : null;
      }
      case "event": {
        const e = events.find((x) => x.id === r.id);
        return e ? { title: e.title, icon: <CalendarDays className="size-3.5 text-rose-400/70" />, onClick: () => setView("calendar") } : null;
      }
      case "habit": {
        const h = habits.find((x) => x.id === r.id);
        return h ? { title: h.name, icon: <Flame className="size-3.5 text-orange-400/70" />, onClick: () => setView("habits") } : null;
      }
      case "page": {
        const p = pages.find((x) => x.id === r.id);
        return p ? { title: p.title, icon: <FileText className="size-3.5 text-gray-400" />, onClick: () => openPage(p.id) } : null;
      }
      case "project": {
        const p = projects.find((x) => x.id === r.id);
        return p ? { title: p.name, icon: <FolderKanban className="size-3.5 text-indigo-400/70" />, onClick: () => setView("projects") } : null;
      }
      default:
        return null;
    }
  };
  const items = refs.map(resolve).filter((x): x is NonNullable<typeof x> => !!x);
  const actionable = label.prefWindowStart || label.prefMinChunk || label.prefBatch;

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-2xl mx-auto p-6 space-y-5">
        <div className="flex items-center gap-2">
          <span className="size-3 rounded-full" style={{ background: label.color }} />
          <h1 className="text-lg font-semibold flex items-center gap-2">
            {label.icon && <span>{label.icon}</span>}
            {label.name}
            {label.groupName && <span className="text-xs font-normal text-gray-500">· {label.groupName}</span>}
          </h1>
          <span className="text-sm text-gray-600">{items.length} item{items.length === 1 ? "" : "s"}</span>
          {actionable && (
            <span className="text-[11px] flex items-center gap-1 text-indigo-300/80" title="This label biases scheduling">
              <Clock className="size-3" /> scheduling
            </span>
          )}
          <button onClick={() => setEditing((v) => !v)} className="ml-auto text-gray-500 hover:text-white" title="Edit label">
            <Pencil className="size-4" />
          </button>
        </div>

        {editing && <LabelEditor key={label.id} initial={label} onSave={(input) => { updateLabel(label.id, input); setEditing(false); }} onDelete={() => { deleteLabel(label.id); setView("calendar"); }} />}

        {items.length === 0 ? (
          <p className="text-sm text-gray-500 py-8 text-center">Nothing tagged <span className="text-gray-300">{label.name}</span> yet. Add it from any task, event, habit, page, or project.</p>
        ) : (
          <div className="space-y-1">
            {items.map((it, i) => (
              <button key={i} onClick={it.onClick} className="w-full flex items-center gap-2 text-left text-sm px-3 py-2 rounded-lg text-gray-300 hover:bg-white/5 hover:text-white">
                <span className="shrink-0">{it.icon}</span>
                <span className="truncate">{it.title}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function LabelEditor({ initial, onSave, onDelete }: { initial: { name: string; color: string; icon?: string; groupName?: string; prefWindowStart?: string; prefWindowEnd?: string; prefMinChunk?: number; prefMaxChunk?: number; prefBatch: boolean }; onSave: (i: LabelInput) => void; onDelete: () => void }) {
  const [f, setF] = useState<LabelInput>({
    name: initial.name,
    color: initial.color,
    icon: initial.icon ?? null,
    groupName: initial.groupName ?? null,
    prefWindowStart: initial.prefWindowStart ?? null,
    prefWindowEnd: initial.prefWindowEnd ?? null,
    prefMinChunk: initial.prefMinChunk ?? null,
    prefMaxChunk: initial.prefMaxChunk ?? null,
    prefBatch: initial.prefBatch,
  });
  const inp = "rounded-md bg-white/5 border border-white/10 px-2 py-1 text-sm outline-none focus:border-indigo-500/50";

  return (
    <div className="rounded-lg border border-white/10 bg-white/[0.02] p-3 space-y-3 text-sm">
      <div className="flex items-center gap-2">
        <input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} placeholder="Name" className={`${inp} flex-1`} />
        <input type="color" value={f.color} onChange={(e) => setF({ ...f, color: e.target.value })} className="size-8 rounded bg-transparent border border-white/10" />
        <input value={f.groupName ?? ""} onChange={(e) => setF({ ...f, groupName: e.target.value || null })} placeholder="Group (Context/Area/Energy)" className={`${inp} w-44`} />
      </div>
      <div className="space-y-2 border-t border-white/10 pt-2">
        <div className="text-[11px] uppercase tracking-wider text-gray-500 flex items-center gap-1"><Clock className="size-3" /> Scheduling (optional — biases the planner)</div>
        <div className="flex items-center gap-2 flex-wrap text-xs text-gray-400">
          <span>Prefer</span>
          <input type="time" value={f.prefWindowStart ?? ""} onChange={(e) => setF({ ...f, prefWindowStart: e.target.value || null })} className={inp} />
          <span>to</span>
          <input type="time" value={f.prefWindowEnd ?? ""} onChange={(e) => setF({ ...f, prefWindowEnd: e.target.value || null })} className={inp} />
          <span className="ml-2">block</span>
          <input type="number" min={0} step={15} value={f.prefMinChunk ?? ""} onChange={(e) => setF({ ...f, prefMinChunk: e.target.value ? Number(e.target.value) : null })} placeholder="min" className={`${inp} w-16`} />
          <span>–</span>
          <input type="number" min={0} step={15} value={f.prefMaxChunk ?? ""} onChange={(e) => setF({ ...f, prefMaxChunk: e.target.value ? Number(e.target.value) : null })} placeholder="max" className={`${inp} w-16`} />
          <label className="flex items-center gap-1 ml-2 cursor-pointer">
            <input type="checkbox" checked={f.prefBatch} onChange={(e) => setF({ ...f, prefBatch: e.target.checked })} /> batch
          </label>
        </div>
      </div>
      <div className="flex items-center gap-2">
        <button onClick={() => onSave(f)} className="text-xs px-3 py-1.5 rounded-md bg-indigo-500 hover:bg-indigo-400 text-white">Save</button>
        <button onClick={() => { if (confirm(`Delete label "${initial.name}"?`)) onDelete(); }} className="ml-auto text-xs px-2 py-1.5 rounded-md text-gray-400 hover:text-rose-300 flex items-center gap-1">
          <Trash2 className="size-3.5" /> Delete
        </button>
      </div>
    </div>
  );
}
