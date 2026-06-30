import { useEffect, useMemo, useRef, useState } from "react";
import { Tag, Plus, X, Check } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type Label, type LabelKind } from "../lib/ipc";

const PALETTE = ["#0ea5e9", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6", "#ec4899", "#14b8a6", "#f97316"];

/** A chip multiselect for an entity's labels (tasks/events/habits/pages/projects). Fetches the
 *  entity's current labels, lets you toggle existing labels or create one on the fly, and persists
 *  via `setEntityLabels`. `compact` shows just a small tag button until opened. */
export default function LabelPicker({
  kind,
  entityId,
  compact,
  revealOnHover,
}: {
  kind: LabelKind;
  entityId: number;
  compact?: boolean;
  /** When there are no labels yet, keep the "add" button hidden until the parent `.group` is hovered
   *  (and while the picker is open) — so unlabeled rows stay clean instead of showing a lone tag icon. */
  revealOnHover?: boolean;
}) {
  const labels = useStore((s) => s.labels);
  const quickLabel = useStore((s) => s.quickLabel);
  const setEntityLabels = useStore((s) => s.setEntityLabels);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [suggestions, setSuggestions] = useState<Label[]>([]);
  const ref = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let cancelled = false;
    api.labelsFor(kind, entityId).then((ls) => !cancelled && setSelected(new Set(ls.map((l) => l.id)))).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [kind, entityId]);

  // Keyword auto-label suggestions (existing labels whose name appears in the entity's text).
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    api.suggestLabels(kind, entityId).then((s) => !cancelled && setSuggestions(s)).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [open, kind, entityId]);

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open]);

  const byId = useMemo(() => new Map(labels.map((l) => [l.id, l])), [labels]);
  const chips = [...selected].map((id) => byId.get(id)).filter((l): l is NonNullable<typeof l> => !!l);

  const persist = (next: Set<number>) => {
    setSelected(next);
    setEntityLabels(kind, entityId, [...next]).catch(() => {});
  };
  const toggle = (id: number) => {
    const next = new Set(selected);
    next.has(id) ? next.delete(id) : next.add(id);
    persist(next);
  };
  const create = async () => {
    const name = query.trim();
    if (!name) return;
    const updated = await quickLabel(name, PALETTE[labels.length % PALETTE.length]);
    const made = updated.find((l) => l.name.toLowerCase() === name.toLowerCase());
    if (made) persist(new Set([...selected, made.id]));
    setQuery("");
  };

  const q = query.trim().toLowerCase();
  const matches = labels.filter((l) => !q || l.name.toLowerCase().includes(q));
  const canCreate = q.length > 0 && !labels.some((l) => l.name.toLowerCase() === q);

  return (
    <div ref={ref} className="relative inline-flex items-center gap-1 flex-wrap">
      {chips.map((l) => (
        <span key={l.id} className="inline-flex items-center gap-1 rounded-full pl-1.5 pr-1 py-0.5 text-[11px]" style={{ background: `${l.color}22`, color: l.color }}>
          <span className="size-1.5 rounded-full" style={{ background: l.color }} />
          {l.name}
          <button onClick={() => toggle(l.id)} className="opacity-60 hover:opacity-100" title="Remove">
            <X className="size-2.5" />
          </button>
        </span>
      ))}
      <button
        onClick={() => setOpen((v) => !v)}
        title="Add label"
        className={clsx(
          "inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px] text-gray-400 hover:text-white hover:bg-white/10",
          compact && chips.length === 0 && "px-1",
          revealOnHover && chips.length === 0 && !open && "opacity-0 group-hover:opacity-100 transition-opacity",
        )}
      >
        <Tag className="size-3" />
        {!compact && chips.length === 0 && "Label"}
      </button>

      {open && (
        <div className="absolute top-full left-0 mt-1 z-50 w-52 rounded-lg bg-[var(--raised)] border border-white/10 shadow-xl p-1.5">
          {(() => {
            const fresh = suggestions.filter((s) => !selected.has(s.id));
            return fresh.length > 0 ? (
              <div className="mb-1.5">
                <div className="px-1 pb-1 text-[10px] uppercase tracking-wide text-gray-500">Suggested</div>
                <div className="flex flex-wrap gap-1 px-1">
                  {fresh.map((l) => (
                    <button
                      key={l.id}
                      onClick={() => toggle(l.id)}
                      className="inline-flex items-center gap-1 rounded-full px-1.5 py-0.5 text-[11px]"
                      style={{ background: `${l.color}22`, color: l.color }}
                      title={`Add "${l.name}"`}
                    >
                      <Plus className="size-2.5" />
                      {l.name}
                    </button>
                  ))}
                </div>
              </div>
            ) : null;
          })()}
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && canCreate && create()}
            placeholder="Filter or create…"
            className="w-full mb-1 rounded-md bg-white/5 border border-white/10 px-2 py-1 text-xs outline-none focus:border-indigo-500/50"
          />
          <div className="max-h-48 overflow-y-auto">
            {matches.map((l) => (
              <button key={l.id} onClick={() => toggle(l.id)} className="w-full flex items-center gap-2 px-1.5 py-1 rounded text-xs text-gray-300 hover:bg-white/5">
                <span className="size-2 rounded-full shrink-0" style={{ background: l.color }} />
                <span className="truncate flex-1 text-left">{l.name}</span>
                {selected.has(l.id) && <Check className="size-3 text-indigo-300" />}
              </button>
            ))}
            {canCreate && (
              <button onClick={create} className="w-full flex items-center gap-2 px-1.5 py-1 rounded text-xs text-indigo-300 hover:bg-white/5">
                <Plus className="size-3" /> Create "{query.trim()}"
              </button>
            )}
            {matches.length === 0 && !canCreate && <p className="px-1.5 py-2 text-[11px] text-gray-600">No labels yet.</p>}
          </div>
        </div>
      )}
    </div>
  );
}
