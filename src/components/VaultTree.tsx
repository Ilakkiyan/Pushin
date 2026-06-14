import { useMemo, useState } from "react";
import { ChevronRight, FileText, Plus, Trash2, CalendarHeart, Download, Loader2 } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { parseLocal } from "../lib/time";
import { importMarkdownFolder } from "../lib/import";
import type { Page } from "../lib/ipc";

/** True if `ancestorId` sits somewhere above `nodeId` in the tree — the drag-reparent cycle guard
 *  (you can't drop a page into its own subtree). Pure + exported for unit testing. */
export function isAncestor(pages: Page[], ancestorId: number, nodeId: number): boolean {
  let cur = pages.find((p) => p.id === nodeId);
  while (cur?.parentId != null) {
    if (cur.parentId === ancestorId) return true;
    const parentId: number = cur.parentId;
    cur = pages.find((p) => p.id === parentId);
  }
  return false;
}

// Build a parentId → children map once per render so the tree is O(n).
function childrenOf(pages: Page[]): Map<number | null, Page[]> {
  const map = new Map<number | null, Page[]>();
  for (const p of pages) {
    const key = p.parentId ?? null;
    const arr = map.get(key) ?? [];
    arr.push(p);
    map.set(key, arr);
  }
  return map;
}

function TreeNode({ page, byParent, depth }: { page: Page; byParent: Map<number | null, Page[]>; depth: number }) {
  const currentPageId = useStore((s) => s.currentPageId);
  const openPage = useStore((s) => s.openPage);
  const createPage = useStore((s) => s.createPage);
  const deletePage = useStore((s) => s.deletePage);
  const movePage = useStore((s) => s.movePage);
  const pages = useStore((s) => s.pages);
  const [expanded, setExpanded] = useState(false);
  const [dropHover, setDropHover] = useState(false);

  const kids = byParent.get(page.id) ?? [];
  const active = currentPageId === page.id;

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDropHover(false);
    const src = Number(e.dataTransfer.getData("text/page"));
    if (src && src !== page.id && !isAncestor(pages, src, page.id)) {
      movePage(src, page.id, 0);
      setExpanded(true);
    }
  };

  return (
    <div>
      <div
        draggable
        onDragStart={(e) => e.dataTransfer.setData("text/page", String(page.id))}
        onDragOver={(e) => {
          e.preventDefault();
          setDropHover(true);
        }}
        onDragLeave={() => setDropHover(false)}
        onDrop={onDrop}
        className={clsx(
          "group flex items-center gap-1 rounded-md pr-1 text-sm cursor-pointer",
          active ? "bg-white/10 text-white" : "text-gray-400 hover:bg-white/5 hover:text-white",
          dropHover && "ring-1 ring-indigo-400/50 bg-indigo-500/10",
        )}
        style={{ paddingLeft: depth * 12 + 4 }}
        onClick={() => openPage(page.id)}
      >
        <button
          onClick={(e) => {
            e.stopPropagation();
            setExpanded((v) => !v);
          }}
          className={clsx("p-0.5 rounded hover:bg-white/10 shrink-0", kids.length === 0 && "invisible")}
        >
          <ChevronRight className={clsx("size-3 transition-transform", expanded && "rotate-90")} />
        </button>
        <span className="shrink-0 w-4 text-center text-xs leading-none">{page.icon ?? <FileText className="size-3.5 inline text-gray-500" />}</span>
        <span className="truncate flex-1 py-1">{page.title}</span>
        <button
          onClick={(e) => {
            e.stopPropagation();
            createPage(page.id);
            setExpanded(true);
          }}
          title="Add sub-page"
          className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 shrink-0"
        >
          <Plus className="size-3.5" />
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            if (confirm(`Delete "${page.title}"?`)) deletePage(page.id);
          }}
          title="Delete page"
          className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 hover:text-rose-300 shrink-0"
        >
          <Trash2 className="size-3.5" />
        </button>
      </div>
      {expanded && kids.map((k) => <TreeNode key={k.id} page={k} byParent={byParent} depth={depth + 1} />)}
    </div>
  );
}

/** The recursive vault page tree + a Journal list of daily notes, shown under the sidebar's Vault
 *  section. Daily notes (pages with a `dailyDate`) are kept out of the manual Pages tree. */
export default function VaultTree() {
  const pages = useStore((s) => s.pages);
  const createPage = useStore((s) => s.createPage);
  const openPage = useStore((s) => s.openPage);
  const loadPages = useStore((s) => s.loadPages);
  const currentPageId = useStore((s) => s.currentPageId);
  const [importing, setImporting] = useState<{ done: number; total: number } | null>(null);

  const runImport = async () => {
    if (importing) return;
    setImporting({ done: 0, total: 0 });
    try {
      const n = await importMarkdownFolder((done, total) => setImporting({ done, total }));
      if (n) await loadPages();
    } catch {
      /* ignore — cancelled or failed */
    } finally {
      setImporting(null);
    }
  };

  // Manual pages (the tree) vs. daily notes (the Journal), kept separate.
  const manual = useMemo(() => pages.filter((p) => !p.dailyDate && !p.inbox), [pages]);
  const dailies = useMemo(
    () => pages.filter((p) => p.dailyDate).sort((a, b) => (a.dailyDate! < b.dailyDate! ? 1 : -1)).slice(0, 14),
    [pages],
  );
  const movePage = useStore((s) => s.movePage);
  const byParent = useMemo(() => childrenOf(manual), [manual]);
  const roots = byParent.get(null) ?? [];

  // Dropping a page onto the Pages container (not onto another node) moves it to the top level.
  const onRootDrop = (e: React.DragEvent) => {
    const src = Number(e.dataTransfer.getData("text/page"));
    if (src) movePage(src, null, 0);
  };

  return (
    <div className="mt-1 space-y-0.5" onDragOver={(e) => e.preventDefault()} onDrop={onRootDrop}>
      <div className="flex items-center justify-between px-3 pt-2 pb-0.5">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-gray-600">Pages</span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={runImport}
            title="Import a Markdown / Obsidian folder"
            className="p-0.5 rounded text-gray-500 hover:text-white hover:bg-white/10"
          >
            {importing ? <Loader2 className="size-3.5 animate-spin" /> : <Download className="size-3.5" />}
          </button>
          <button onClick={() => createPage(null)} title="New page" className="p-0.5 rounded text-gray-500 hover:text-white hover:bg-white/10">
            <Plus className="size-3.5" />
          </button>
        </div>
      </div>
      {importing && importing.total > 0 && (
        <p className="px-3 py-0.5 text-[10px] text-indigo-300/80">Importing… {importing.done}/{importing.total}</p>
      )}
      {roots.length === 0 ? (
        <p className="px-3 py-1 text-[11px] text-gray-600">No pages yet. Hit + to create one.</p>
      ) : (
        roots.map((p) => <TreeNode key={p.id} page={p} byParent={byParent} depth={0} />)
      )}

      {dailies.length > 0 && (
        <>
          <div className="px-3 pt-3 pb-0.5 text-[10px] font-semibold uppercase tracking-wider text-gray-600">Journal</div>
          {dailies.map((p) => (
            <div
              key={p.id}
              onClick={() => openPage(p.id)}
              className={clsx(
                "flex items-center gap-1.5 rounded-md px-2 py-1 text-sm cursor-pointer",
                currentPageId === p.id ? "bg-white/10 text-white" : "text-gray-400 hover:bg-white/5 hover:text-white",
              )}
            >
              <CalendarHeart className="size-3.5 shrink-0 text-gray-500" />
              <span className="truncate">{parseLocal(p.dailyDate!).toLocaleDateString([], { month: "short", day: "numeric" })}</span>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
