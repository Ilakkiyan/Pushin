import { useMemo, useRef, useState } from "react";
import {
  CalendarHeart,
  ChevronRight,
  Download,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  FolderUp,
  Loader2,
  Pencil,
  Plus,
  Trash2,
  X,
} from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { parseLocal } from "../lib/time";
import { importMarkdownFolder } from "../lib/import";
import { JOURNAL_ID, browsablePages, childrenOf, isAncestor, journalEntries } from "../lib/pageTree";
import { dragSource, setPageDrag } from "../lib/pageDrag";
import ContextMenu, { type MenuAnchor, type MenuItem } from "./ContextMenu";
import type { Page } from "../lib/ipc";

// Re-exported from its new home in `lib/pageTree` — the vault browser needs the same cycle guard,
// so the logic can't live in a component file any more.
export { isAncestor } from "../lib/pageTree";

function TreeNode({ page, byParent, depth }: { page: Page; byParent: Map<number | null, Page[]>; depth: number }) {
  const currentPageId = useStore((s) => s.currentPageId);
  const openPage = useStore((s) => s.openPage);
  const openFolder = useStore((s) => s.openFolder);
  const createPage = useStore((s) => s.createPage);
  const createFolder = useStore((s) => s.createFolder);
  const deletePage = useStore((s) => s.deletePage);
  const movePage = useStore((s) => s.movePage);
  const renamePage = useStore((s) => s.renamePage);
  const pages = useStore((s) => s.pages);
  const [expanded, setExpanded] = useState(false);
  const [dropHover, setDropHover] = useState(false);
  const [menu, setMenu] = useState<MenuAnchor | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(page.title);
  const renameRef = useRef<HTMLInputElement | null>(null);

  const kids = byParent.get(page.id) ?? [];
  const active = currentPageId === page.id;
  const isFolder = !!page.isFolder;

  // A node can hold a drop unless it would swallow itself or one of its own ancestors, which would
  // cut the whole subtree off from the root.
  const accepts = (src: number | null) => src != null && src !== page.id && !isAncestor(pages, src, page.id);

  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDropHover(false);
    const src = dragSource(e);
    setPageDrag(null);
    if (accepts(src)) {
      movePage(src!, page.id, 0);
      setExpanded(true);
    }
  };

  const startRename = () => {
    setDraft(page.title);
    setRenaming(true);
    requestAnimationFrame(() => renameRef.current?.select());
  };

  const commitRename = () => {
    const name = draft.trim();
    setRenaming(false);
    if (name && name !== page.title) void renamePage(page.id, name);
  };

  const confirmDelete = () => {
    // Deleting a folder never deletes what's inside it: the children reparent to the top level
    // (the schema's ON DELETE SET NULL). Say so, so nobody hesitates over the button.
    const warning = isFolder && kids.length > 0 ? ` Its ${kids.length} item(s) move to the top level.` : "";
    if (confirm(`Delete "${page.title}"?${warning}`)) deletePage(page.id);
  };

  const items: MenuItem[] = [
    {
      label: "Open",
      icon: isFolder ? FolderOpen : FileText,
      onSelect: () => (isFolder ? openFolder(page.id) : openPage(page.id)),
    },
    { separator: true },
    {
      label: isFolder ? "New page inside" : "Add sub-page",
      icon: Plus,
      onSelect: () => {
        void createPage(page.id);
        setExpanded(true);
      },
    },
    ...(isFolder
      ? [
          {
            label: "New folder inside",
            icon: FolderPlus,
            onSelect: () => {
              void createFolder("New folder", page.id);
              setExpanded(true);
            },
          } as MenuItem,
        ]
      : []),
    { separator: true },
    { label: "Rename", icon: Pencil, onSelect: startRename },
    {
      label: "Move to Vault",
      icon: FolderUp,
      disabled: (page.parentId ?? null) === null,
      onSelect: () => void movePage(page.id, null, 0),
    },
    { separator: true },
    { label: "Delete", icon: Trash2, danger: true, onSelect: confirmDelete },
  ];

  return (
    <div>
      <div
        draggable={!renaming}
        onDragStart={(e) => {
          setPageDrag(page.id);
          e.dataTransfer.setData("text/page", String(page.id));
          e.dataTransfer.effectAllowed = "move";
        }}
        onDragEnd={() => {
          setPageDrag(null);
          setDropHover(false);
        }}
        onDragOver={(e) => {
          const src = dragSource(e);
          // Refusing here (no preventDefault) lets the drag fall through to the Pages container,
          // which files it at the top level, and keeps the cursor honest about what will happen.
          if (!accepts(src)) {
            if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
            return;
          }
          e.preventDefault();
          e.stopPropagation();
          e.dataTransfer.dropEffect = "move";
          setDropHover(true);
        }}
        onDragLeave={() => setDropHover(false)}
        onDrop={onDrop}
        onContextMenu={(e) => {
          // The rename field keeps its own cut/copy/paste menu.
          if ((e.target as HTMLElement).closest("input")) return;
          e.preventDefault();
          e.stopPropagation();
          setMenu({ x: e.clientX, y: e.clientY, items });
        }}
        className={clsx(
          "group flex items-center gap-1 pr-1 text-sm cursor-pointer",
          active ? "is-selected text-white" : "hoverable text-[var(--ink-muted)] hover:text-white",
          dropHover && "ring-1 ring-white/40 bg-white/[0.08]",
        )}
        style={{ paddingLeft: depth * 12 + 4 }}
        // A folder isn't a document — clicking it browses TO it (the Drive view), it never opens an editor.
        onClick={() => !renaming && (isFolder ? openFolder(page.id) : openPage(page.id))}
      >
        <button
          onClick={(e) => {
            e.stopPropagation();
            setExpanded((v) => !v);
          }}
          // A folder always keeps its twisty, even while empty — that's the affordance that says
          // "things go in here"; a page only earns one once it has children.
          className={clsx("p-0.5 rounded hover:bg-white/10 shrink-0", kids.length === 0 && !isFolder && "invisible")}
        >
          <ChevronRight className={clsx("size-3 transition-transform", expanded && "rotate-90")} />
        </button>
        <span className="shrink-0 w-4 text-center text-xs leading-none">
          {page.icon ??
            (isFolder ? (
              expanded ? (
                <FolderOpen className="size-3.5 inline text-amber-300/80" />
              ) : (
                <Folder className="size-3.5 inline text-amber-300/80" />
              )
            ) : (
              <FileText className="size-3.5 inline text-gray-500" />
            ))}
        </span>
        {renaming ? (
          <input
            ref={renameRef}
            value={draft}
            aria-label="Rename"
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onClick={(e) => e.stopPropagation()}
            onBlur={commitRename}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") commitRename();
              if (e.key === "Escape") setRenaming(false);
            }}
            className="flex-1 min-w-0 my-0.5 bg-[var(--raised)] border border-white/20 px-1 py-0.5 text-sm text-white outline-none"
          />
        ) : (
          <span className="truncate flex-1 py-1">{page.title}</span>
        )}
        {isFolder && (
          <button
            onClick={(e) => {
              e.stopPropagation();
              void createFolder("New folder", page.id);
              setExpanded(true);
            }}
            title="New folder inside"
            className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 shrink-0"
          >
            <FolderPlus className="size-3.5" />
          </button>
        )}
        <button
          onClick={(e) => {
            e.stopPropagation();
            createPage(page.id);
            setExpanded(true);
          }}
          title={isFolder ? "New page inside" : "Add sub-page"}
          className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 shrink-0"
        >
          <Plus className="size-3.5" />
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            confirmDelete();
          }}
          title={isFolder ? "Delete folder" : "Delete page"}
          className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 hover:text-rose-300 shrink-0"
        >
          <Trash2 className="size-3.5" />
        </button>
      </div>
      {expanded && kids.map((k) => <TreeNode key={k.id} page={k} byParent={byParent} depth={depth + 1} />)}
      {menu && <ContextMenu anchor={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}

/** The pages you currently have open, most recent last — a switcher so moving between two documents
 *  you're working across doesn't mean hunting them down in the tree each time. Session-only; closing
 *  one only closes the tab, it never touches the page. */
function OpenPages() {
  const openPageIds = useStore((s) => s.openPageIds);
  const pages = useStore((s) => s.pages);
  const currentPageId = useStore((s) => s.currentPageId);
  const openPage = useStore((s) => s.openPage);
  const closePage = useStore((s) => s.closePage);

  // A page can be closed from under us (deleted elsewhere, or an id that no longer resolves) — read
  // through the live tree rather than trusting the id list to still describe real pages.
  const open = openPageIds.map((id) => pages.find((p) => p.id === id)).filter((p): p is Page => !!p);
  if (open.length === 0) return null;

  return (
    <div className="mt-1">
      <div className="px-3 pt-2 pb-0.5 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)]">Open</div>
      {open.map((p) => (
        <div
          key={p.id}
          onClick={() => openPage(p.id)}
          className={clsx(
            "group flex items-center gap-1.5 px-2 py-1 text-sm cursor-pointer",
            currentPageId === p.id ? "is-selected text-white" : "hoverable text-[var(--ink-muted)] hover:text-white",
          )}
        >
          {p.dailyDate ? (
            <CalendarHeart className="size-3.5 shrink-0 text-indigo-300/80" />
          ) : (
            <FileText className="size-3.5 shrink-0 text-gray-500" />
          )}
          <span className="truncate flex-1">{p.title}</span>
          <button
            onClick={(e) => {
              e.stopPropagation();
              closePage(p.id);
            }}
            title="Close"
            className="opacity-0 group-hover:opacity-100 p-0.5 rounded hover:bg-white/10 shrink-0"
          >
            <X className="size-3" />
          </button>
        </div>
      ))}
    </div>
  );
}

/** The recursive vault page tree + a Journal list of daily notes, shown under the sidebar's Vault
 *  section. Daily notes (pages with a `dailyDate`) are kept out of the manual Pages tree. */
export default function VaultTree() {
  const pages = useStore((s) => s.pages);
  const createPage = useStore((s) => s.createPage);
  const createFolder = useStore((s) => s.createFolder);
  const openPage = useStore((s) => s.openPage);
  const deletePage = useStore((s) => s.deletePage);
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

  // Manual pages + folders (the tree) vs. daily notes (the Journal), kept separate.
  const manual = useMemo(() => browsablePages(pages), [pages]);
  const dailies = useMemo(() => journalEntries(pages).slice(0, 14), [pages]);
  const movePage = useStore((s) => s.movePage);
  const openFolder = useStore((s) => s.openFolder);
  const byParent = useMemo(() => childrenOf(manual), [manual]);
  const roots = byParent.get(null) ?? [];
  const [menu, setMenu] = useState<MenuAnchor | null>(null);

  // Dropping a page onto the Pages container (not onto another node) moves it to the top level.
  // A page already sitting at the top level is left alone rather than written back unchanged.
  const onRootDrop = (e: React.DragEvent) => {
    const src = dragSource(e);
    setPageDrag(null);
    if (src == null) return;
    const page = pages.find((p) => p.id === src);
    if (!page || (page.parentId ?? null) === null) return;
    movePage(src, null, 0);
  };

  const containerMenu: MenuItem[] = [
    { label: "New page", icon: Plus, onSelect: () => void createPage(null) },
    { label: "New folder", icon: FolderPlus, onSelect: () => void createFolder("New folder", null) },
    { separator: true },
    { label: "Import Markdown", icon: Download, onSelect: () => void runImport() },
  ];

  return (
    <div
      className="mt-1 space-y-0.5"
      onDragOver={(e) => e.preventDefault()}
      onDrop={onRootDrop}
      onContextMenu={(e) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY, items: containerMenu });
      }}
    >
      <OpenPages />

      <div className="flex items-center justify-between px-3 pt-2 pb-0.5">
        <span className="text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)]">Pages</span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={runImport}
            title="Import a Markdown / Obsidian folder"
            className="p-0.5 rounded text-gray-500 hover:text-white hover:bg-white/10"
          >
            {importing ? <Loader2 className="size-3.5 animate-spin" /> : <Download className="size-3.5" />}
          </button>
          <button
            onClick={() => void createFolder("New folder", null)}
            title="New folder"
            className="p-0.5 rounded text-gray-500 hover:text-white hover:bg-white/10"
          >
            <FolderPlus className="size-3.5" />
          </button>
          <button onClick={() => createPage(null)} title="New page" className="p-0.5 rounded text-gray-500 hover:text-white hover:bg-white/10">
            <Plus className="size-3.5" />
          </button>
        </div>
      </div>
      {importing && importing.total > 0 && (
        <p className="tnum px-3 py-0.5 text-[10px] text-[var(--ink-muted)]">Importing… {importing.done}/{importing.total}</p>
      )}
      {roots.length === 0 ? (
        <p className="px-3 py-1 text-[11px] text-[var(--ink-faint)]">No pages yet. Hit + to create one.</p>
      ) : (
        roots.map((p) => <TreeNode key={p.id} page={p} byParent={byParent} depth={0} />)
      )}

      {dailies.length > 0 && (
        <>
          {/* The Journal header is itself the way into the Journal folder in the browser — the list
              below is just the most recent few, so there has to be a door to the rest. */}
          <button
            onClick={() => openFolder(JOURNAL_ID)}
            title="Open the Journal folder"
            className="group w-full flex items-center gap-1 px-3 pt-3 pb-0.5 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)] hover:text-white"
          >
            <span>Journal</span>
            <ChevronRight className="size-3 opacity-0 group-hover:opacity-100" />
          </button>
          {dailies.map((p) => (
            <div
              key={p.id}
              onClick={() => openPage(p.id)}
              // A journal entry is filed by its date, so it has no parent to change and no name to
              // rename. Its menu is the two things that ARE true of it.
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setMenu({
                  x: e.clientX,
                  y: e.clientY,
                  items: [
                    { label: "Open", icon: FileText, onSelect: () => openPage(p.id) },
                    { separator: true },
                    {
                      label: "Delete",
                      icon: Trash2,
                      danger: true,
                      onSelect: () => {
                        if (confirm(`Delete "${p.title}"?`)) void deletePage(p.id);
                      },
                    },
                  ],
                });
              }}
              className={clsx(
                "flex items-center gap-1.5 px-2 py-1 text-sm cursor-pointer",
                currentPageId === p.id ? "is-selected text-white" : "hoverable text-[var(--ink-muted)] hover:text-white",
              )}
            >
              <CalendarHeart className="size-3.5 shrink-0 text-gray-500" />
              <span className="tnum truncate">{parseLocal(p.dailyDate!).toLocaleDateString([], { month: "short", day: "numeric" })}</span>
            </div>
          ))}
        </>
      )}

      {menu && <ContextMenu anchor={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}
