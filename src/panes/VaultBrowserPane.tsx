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
  Grid2x2,
  Library,
  List,
  Loader2,
  Pencil,
  Plus,
  Search,
  Trash2,
  X,
} from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { parseLocal, toLocalDate } from "../lib/time";
import { importMarkdownFolder } from "../lib/import";
import {
  JOURNAL_ID,
  browsablePages,
  folderCount,
  folderPath,
  isAncestor,
  journalEntries,
  sortEntries,
} from "../lib/pageTree";
import ContextMenu, { type MenuAnchor, type MenuItem } from "../components/ContextMenu";
import { dragSource, setPageDrag } from "../lib/pageDrag";
import type { Page } from "../lib/ipc";

type Layout = "grid" | "list";

/** Which breadcrumb a drag is hovering: a folder id, the root crumb, or nothing. */
type Crumb = number | "root" | null;

/** "3 items" / "1 item" — a folder's weight at a glance, the way Drive shows it. */
function itemCount(n: number): string {
  return `${n} ${n === 1 ? "item" : "items"}`;
}

/** A short, human date for the "Modified" column. Machine-readable, so it wears the mono face. */
function modified(iso: string): string {
  if (!iso) return "—";
  const d = new Date(iso.includes("T") ? iso : iso.replace(" ", "T"));
  if (Number.isNaN(d.getTime())) return "—";
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString([], { month: "short", day: "numeric", ...(sameYear ? {} : { year: "numeric" }) });
}

/** The icon a row/card wears: a folder, a journal day, or a document. */
function EntryIcon({ page, className }: { page: Page; className?: string }) {
  if (page.isFolder) return <Folder className={clsx(className, "text-amber-300/80")} />;
  if (page.dailyDate) return <CalendarHeart className={clsx(className, "text-indigo-300/80")} />;
  return <FileText className={clsx(className, "text-[var(--ink-faint)]")} />;
}

/** The Drive-style vault browser: the folder you're standing in, its contents as cards or rows, and
 *  a breadcrumb back to the root. Folders are real page rows (`isFolder`); the Journal is a virtual
 *  folder gathering the daily notes, which have no parent of their own.
 *
 *  Opening a document hands off to the editor (`vault` view); opening a folder just moves the
 *  browser. Both entry points — the sidebar's Vault button and this pane — land here first, so the
 *  vault always opens on a place rather than on whatever document happened to be open last. */
export default function VaultBrowserPane() {
  const pages = useStore((s) => s.pages);
  const folderId = useStore((s) => s.vaultFolderId);
  const openFolder = useStore((s) => s.openFolder);
  const openPage = useStore((s) => s.openPage);
  const openDaily = useStore((s) => s.openDaily);
  const createPage = useStore((s) => s.createPage);
  const createFolder = useStore((s) => s.createFolder);
  const renamePage = useStore((s) => s.renamePage);
  const deletePage = useStore((s) => s.deletePage);
  const movePage = useStore((s) => s.movePage);
  const loadPages = useStore((s) => s.loadPages);

  const [layout, setLayout] = useState<Layout>("grid");
  const [query, setQuery] = useState("");
  // The row being renamed in place, and the draft name. Inline beats a modal here: renaming is a
  // one-field edit and the surrounding folder is the context you need while doing it.
  const [renaming, setRenaming] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [dropTarget, setDropTarget] = useState<number | null>(null);
  const [dropCrumb, setDropCrumb] = useState<Crumb>(null);
  const [menu, setMenu] = useState<MenuAnchor | null>(null);
  const [importing, setImporting] = useState<{ done: number; total: number } | null>(null);
  const renameRef = useRef<HTMLInputElement | null>(null);

  const inJournal = folderId === JOURNAL_ID;
  const trail = useMemo(() => folderPath(pages, folderId), [pages, folderId]);

  // What this folder holds. The vault ROOT also shows the virtual Journal folder, so daily notes are
  // reachable as a place rather than only as a sidebar list.
  const entries = useMemo(() => {
    if (inJournal) return journalEntries(pages);
    const here = browsablePages(pages).filter((p) => (p.parentId ?? null) === folderId);
    const sorted = sortEntries(here);
    if (folderId != null) return sorted;
    // The Journal is part of the vault's fixed shape, so it's there before the first daily note is —
    // a folder that only materializes once you've already used the thing it holds is a folder nobody
    // discovers. Always first, ahead of the user's own folders.
    const journal = journalEntries(pages);
    const virtualFolder = { id: JOURNAL_ID, title: "Journal", isFolder: true, updatedAt: journal[0]?.updatedAt ?? "" } as Page;
    return [virtualFolder, ...sorted];
  }, [pages, folderId, inJournal]);

  // The root always holds at least the Journal, so "nothing here" can never be an empty grid there.
  // A first-run vault still needs a nudge, though — it rides ALONGSIDE the Journal card instead of
  // replacing the listing.
  const vaultUntouched = folderId == null && browsablePages(pages).length === 0;

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((p) => p.title.toLowerCase().includes(q));
  }, [entries, query]);

  const runImport = async () => {
    if (importing) return;
    setImporting({ done: 0, total: 0 });
    try {
      const n = await importMarkdownFolder((done, total) => setImporting({ done, total }));
      if (n) await loadPages();
    } catch {
      /* cancelled or failed — the browser just stays as it was */
    } finally {
      setImporting(null);
    }
  };

  const open = (page: Page) => {
    if (page.isFolder) openFolder(page.id);
    else openPage(page.id);
  };

  const startRename = (page: Page) => {
    setRenaming(page.id);
    setDraft(page.title);
    // Focus after the input mounts; selecting the text makes "type over it" the default gesture.
    requestAnimationFrame(() => renameRef.current?.select());
  };

  const commitRename = async () => {
    const id = renaming;
    const name = draft.trim();
    setRenaming(null);
    if (id == null || !name) return;
    const before = pages.find((p) => p.id === id);
    if (before && before.title === name) return;
    await renamePage(id, name);
  };

  const remove = async (page: Page) => {
    const kids = page.isFolder ? folderCount(pages, page.id) : 0;
    const warning = kids > 0 ? `\n\nIts ${itemCount(kids)} move to the top level. Nothing is deleted with it.` : "";
    if (!confirm(`Delete "${page.title}"?${warning}`)) return;
    await deletePage(page.id);
  };

  // A folder made from a menu appears where you asked for it, so the browser follows it in. A new
  // folder you cannot see is a folder you cannot name, and naming it is the next thing you want.
  const newFolderIn = async (parentId: number | null) => {
    const folder = await createFolder("New folder", parentId);
    if (parentId !== folderId) openFolder(parentId);
    startRename(folder);
  };

  // Drag & drop. Only folders accept a drop, and a folder can't swallow its own ancestor — that
  // would orphan the whole subtree from the root. The Journal is virtual, so it never takes one, and
  // a daily note can't be filed anywhere: the Journal gathers it by date rather than by parent, so
  // the move would read as the note simply vanishing.
  const canDropInto = (destId: number | null, srcId: number) => {
    const src = pages.find((p) => p.id === srcId);
    if (!src || src.dailyDate) return false;
    if (destId === JOURNAL_ID) return false;
    if (destId != null && (srcId === destId || isAncestor(pages, srcId, destId))) return false;
    return (src.parentId ?? null) !== destId; // already filed there
  };

  const canDrop = (target: Page, srcId: number) => !!target.isFolder && canDropInto(target.id, srcId);

  const srcOf = (e: React.DragEvent): number | null => dragSource(e);

  const endDrag = () => {
    setPageDrag(null);
    setDropTarget(null);
    setDropCrumb(null);
  };

  const onDropInto = async (target: Page, e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const src = srcOf(e);
    endDrag();
    if (src && canDrop(target, src)) await movePage(src, target.id, 0);
  };

  // Dropping on the pane background (not on a card) moves the page into the folder you're browsing.
  const onDropHere = async (e: React.DragEvent) => {
    const src = srcOf(e);
    endDrag();
    if (!src || inJournal) return;
    if (folderId != null && (src === folderId || isAncestor(pages, src, folderId))) return;
    if (!canDropInto(folderId, src)) return;
    await movePage(src, folderId, 0);
  };

  const isDraggable = (page: Page) => page.id !== JOURNAL_ID && !page.dailyDate;

  const dragProps = (page: Page) =>
    !isDraggable(page)
      ? {}
      : {
          draggable: true,
          onDragStart: (e: React.DragEvent) => {
            setPageDrag(page.id);
            e.dataTransfer.setData("text/page", String(page.id));
            e.dataTransfer.effectAllowed = "move";
          },
          onDragEnd: endDrag,
        };

  const dropProps = (page: Page) => ({
    onDragOver: (e: React.DragEvent) => {
      const src = srcOf(e);
      // Not a target: leave the event alone so it bubbles to the pane, which files the drop into the
      // folder you are standing in. The cursor reads "no drop here" in the meantime.
      if (src == null || !canDrop(page, src)) {
        if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      e.dataTransfer.dropEffect = "move";
      setDropTarget(page.id);
    },
    onDragLeave: () => setDropTarget((t) => (t === page.id ? null : t)),
    onDrop: (e: React.DragEvent) => void onDropInto(page, e),
  });

  // Breadcrumbs take drops too. Dragging onto an ancestor is how you move something back OUT of a
  // folder without navigating there first, which is the one direction card targets cannot express.
  const crumbDropProps = (id: number | null) => {
    const key: Crumb = id ?? "root";
    return {
      onDragOver: (e: React.DragEvent) => {
        const src = srcOf(e);
        if (src == null || !canDropInto(id, src)) {
          if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
          return;
        }
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = "move";
        setDropCrumb(key);
      },
      onDragLeave: () => setDropCrumb((c) => (c === key ? null : c)),
      onDrop: (e: React.DragEvent) => {
        e.preventDefault();
        e.stopPropagation();
        const src = srcOf(e);
        endDrag();
        if (src && canDropInto(id, src)) void movePage(src, id, 0);
      },
    };
  };

  const renameField = () => (
    <input
      ref={renameRef}
      value={draft}
      aria-label="Rename"
      autoFocus
      onChange={(e) => setDraft(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onBlur={() => void commitRename()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") void commitRename();
        if (e.key === "Escape") setRenaming(null);
      }}
      className="w-full bg-[var(--raised)] border border-white/20 px-1.5 py-0.5 text-sm text-white outline-none"
    />
  );

  const rowActions = (page: Page) =>
    page.id === JOURNAL_ID ? null : (
      <div className="flex items-center gap-0.5 opacity-0 group-hover:opacity-100 focus-within:opacity-100">
        <button
          onClick={(e) => {
            e.stopPropagation();
            startRename(page);
          }}
          title="Rename"
          className="p-1 hoverable text-[var(--ink-faint)] hover:text-white"
        >
          <Pencil className="size-3.5" />
        </button>
        <button
          onClick={(e) => {
            e.stopPropagation();
            void remove(page);
          }}
          title="Delete"
          className="p-1 hoverable text-[var(--ink-faint)] hover:text-rose-300"
        >
          <Trash2 className="size-3.5" />
        </button>
      </div>
    );

  const showMenu = (e: React.MouseEvent, items: MenuItem[]) => {
    // A text field keeps its own menu: cut/copy/paste on the name you are typing is more use than
    // "New folder", and taking it away is the kind of small theft that makes an app feel wrong.
    if ((e.target as HTMLElement).closest("input, textarea, [contenteditable='true']")) return;
    e.preventDefault();
    e.stopPropagation();
    setMenu({ x: e.clientX, y: e.clientY, items });
  };

  /** The right-click menu for one entry: everything the hover buttons do, plus what there was never
   *  room for on a card: filing it back out of a folder, and creating inside the folder you clicked. */
  const entryMenu = (page: Page): MenuItem[] => {
    if (page.id === JOURNAL_ID) {
      return [{ label: "Open", icon: FolderOpen, onSelect: () => openFolder(JOURNAL_ID) }];
    }
    if (page.dailyDate) {
      // A journal entry is filed by its date, so it has no parent to change and no name to rename.
      return [
        { label: "Open", icon: FileText, onSelect: () => openPage(page.id) },
        { separator: true },
        { label: "Delete", icon: Trash2, danger: true, onSelect: () => void remove(page) },
      ];
    }
    const items: MenuItem[] = [
      { label: "Open", icon: page.isFolder ? FolderOpen : FileText, onSelect: () => open(page) },
      { separator: true },
    ];
    if (page.isFolder) {
      items.push(
        { label: "New page inside", icon: Plus, onSelect: () => void createPage(page.id) },
        { label: "New folder inside", icon: FolderPlus, onSelect: () => void newFolderIn(page.id) },
        { separator: true },
      );
    }
    items.push(
      { label: "Rename", icon: Pencil, onSelect: () => startRename(page) },
      {
        label: "Move to Vault",
        icon: FolderUp,
        disabled: (page.parentId ?? null) === null,
        onSelect: () => void movePage(page.id, null, 0),
      },
      { separator: true },
      { label: "Delete", icon: Trash2, danger: true, onSelect: () => void remove(page) },
    );
    return items;
  };

  /** Right-clicking the empty space acts on the folder you are standing in. */
  const hereMenu = (): MenuItem[] => {
    if (inJournal) {
      return [{ label: "Today's note", icon: CalendarHeart, onSelect: () => void openDaily(toLocalDate(new Date())) }];
    }
    const items: MenuItem[] = [
      { label: "New page", icon: Plus, onSelect: () => void createPage(folderId) },
      { label: "New folder", icon: FolderPlus, onSelect: () => void newFolderIn(folderId) },
    ];
    if (folderId == null) {
      items.push({ separator: true }, { label: "Import Markdown", icon: Download, onSelect: () => void runImport() });
    }
    items.push(
      { separator: true },
      layout === "grid"
        ? { label: "List view", icon: List, onSelect: () => setLayout("list") }
        : { label: "Grid view", icon: Grid2x2, onSelect: () => setLayout("grid") },
    );
    return items;
  };

  /** The line under a name: what the thing is, in the fewest words that are actually informative. */
  const subtitle = (page: Page) => {
    if (page.id === JOURNAL_ID) return itemCount(journalEntries(pages).length);
    if (page.isFolder) return itemCount(folderCount(pages, page.id));
    if (page.dailyDate) return parseLocal(page.dailyDate).toLocaleDateString([], { weekday: "long" });
    return modified(page.updatedAt);
  };

  return (
    <div
      className="h-full w-full overflow-y-auto"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => void onDropHere(e)}
      onContextMenu={(e) => showMenu(e, hereMenu())}
    >
      <div className="max-w-5xl mx-auto p-6 space-y-4">
        {/* Breadcrumb + actions */}
        <div className="flex items-center justify-between gap-4 flex-wrap">
          <div className="flex items-center gap-1 text-sm min-w-0">
            <button
              onClick={() => openFolder(null)}
              {...crumbDropProps(null)}
              className={clsx(
                "flex items-center gap-1.5 px-2 py-1 hoverable",
                folderId == null ? "text-white font-medium" : "text-[var(--ink-muted)] hover:text-white",
                dropCrumb === "root" && "ring-1 ring-white/40 bg-white/[0.08]",
              )}
            >
              <Library className="size-4" /> Vault
            </button>
            {trail.map((f, i) => (
              <span key={f.id} className="flex items-center gap-1 min-w-0">
                <ChevronRight className="size-3.5 shrink-0 text-[var(--ink-faint)]" />
                <button
                  onClick={() => openFolder(f.id)}
                  {...crumbDropProps(f.id)}
                  className={clsx(
                    "truncate px-2 py-1 hoverable",
                    i === trail.length - 1 ? "text-white font-medium" : "text-[var(--ink-muted)] hover:text-white",
                    dropCrumb === f.id && "ring-1 ring-white/40 bg-white/[0.08]",
                  )}
                >
                  {f.title}
                </button>
              </span>
            ))}
          </div>

          <div className="flex items-center gap-2">
            <div className="flex items-center gap-1.5 border border-white/10 px-2 py-1">
              <Search className="size-3.5 shrink-0 text-[var(--ink-faint)]" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Filter this folder"
                className="w-36 bg-transparent text-xs outline-none placeholder:text-[var(--ink-faint)]"
              />
              {query && (
                <button onClick={() => setQuery("")} title="Clear filter" className="text-[var(--ink-faint)] hover:text-white">
                  <X className="size-3.5" />
                </button>
              )}
            </div>
            <div className="flex items-center border border-white/10">
              <button
                onClick={() => setLayout("grid")}
                title="Grid view"
                className={clsx("p-1.5", layout === "grid" ? "is-selected text-white" : "hoverable text-[var(--ink-faint)] hover:text-white")}
              >
                <Grid2x2 className="size-3.5" />
              </button>
              <button
                onClick={() => setLayout("list")}
                title="List view"
                className={clsx("p-1.5", layout === "list" ? "is-selected text-white" : "hoverable text-[var(--ink-faint)] hover:text-white")}
              >
                <List className="size-3.5" />
              </button>
            </div>
            {!inJournal && (
              <button
                onClick={() => void newFolderIn(inJournal ? null : folderId)}
                title="New folder"
                className="flex items-center gap-1.5 text-xs px-2.5 py-1.5 hoverable border border-white/10"
              >
                <FolderPlus className="size-3.5" /> New folder
              </button>
            )}
            {inJournal ? (
              <button
                onClick={() => void openDaily(toLocalDate(new Date()))}
                title="Open today's note"
                className="flex items-center gap-1.5 text-xs px-2.5 py-1.5 bg-white/90 hover:bg-white text-gray-900 font-medium"
              >
                <CalendarHeart className="size-3.5" /> Today's note
              </button>
            ) : (
              <button
                onClick={() => void createPage(folderId)}
                title="New page"
                className="flex items-center gap-1.5 text-xs px-2.5 py-1.5 bg-white/90 hover:bg-white text-gray-900 font-medium"
              >
                <Plus className="size-3.5" /> New page
              </button>
            )}
          </div>
        </div>

        {importing && importing.total > 0 && (
          <p className="tnum text-[11px] text-[var(--ink-muted)]">Importing… {importing.done}/{importing.total}</p>
        )}

        {/* Contents */}
        {visible.length === 0 ? (
          <div className="py-20 grid place-items-center text-center">
            <div className="flex flex-col items-center gap-3">
              <Folder className="size-8 text-gray-600" />
              <p className="text-sm text-[var(--ink-muted)]">
                {query
                  ? `Nothing here matches "${query.trim()}".`
                  : inJournal
                    ? "No journal entries yet. Open Today's note to start one."
                    : "This folder is empty."}
              </p>
              {!query && (
                <div className="flex items-center gap-2">
                  {inJournal ? (
                    // In an empty Journal the only sensible "new" is today's entry — a blank untitled
                    // page here would just be a document that isn't a journal entry.
                    <button
                      onClick={() => void openDaily(toLocalDate(new Date()))}
                      className="flex items-center gap-2 text-sm px-4 py-2 bg-white/90 hover:bg-white text-gray-900 font-medium"
                    >
                      <CalendarHeart className="size-4" /> Open today's note
                    </button>
                  ) : (
                    <button
                      onClick={() => void createPage(folderId)}
                      className="flex items-center gap-2 text-sm px-4 py-2 bg-white/90 hover:bg-white text-gray-900 font-medium"
                    >
                      <Plus className="size-4" /> New page
                    </button>
                  )}
                  {folderId == null && (
                    <button
                      onClick={() => void runImport()}
                      className="flex items-center gap-2 text-sm px-4 py-2 hoverable border border-white/10"
                    >
                      {importing ? <Loader2 className="size-4 animate-spin" /> : null} Import Markdown
                    </button>
                  )}
                </div>
              )}
            </div>
          </div>
        ) : layout === "grid" ? (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3">
            {visible.map((page) => (
              <div
                key={page.id}
                data-entry-id={page.id}
                {...dragProps(page)}
                {...dropProps(page)}
                onClick={() => renaming !== page.id && open(page)}
                onContextMenu={(e) => showMenu(e, entryMenu(page))}
                className={clsx(
                  "group border border-white/10 bg-white/[0.02] p-3 cursor-pointer hoverable",
                  dropTarget === page.id && "ring-1 ring-white/40 bg-white/[0.08]",
                )}
              >
                <div className="flex items-start justify-between gap-2">
                  <EntryIcon page={page} className="size-5 shrink-0" />
                  {rowActions(page)}
                </div>
                <div className="mt-3 min-w-0">
                  {renaming === page.id ? (
                    renameField()
                  ) : (
                    <div className="truncate text-sm text-gray-200" title={page.title}>
                      {page.title}
                    </div>
                  )}
                  <div className="tnum mt-1 truncate text-[11px] text-[var(--ink-faint)]">{subtitle(page)}</div>
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className="border border-white/10">
            <div className="flex items-center gap-3 px-3 py-2 border-b border-white/10 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)]">
              <span className="flex-1">Name</span>
              <span className="w-28 shrink-0">Modified</span>
              <span className="w-16 shrink-0" />
            </div>
            {visible.map((page) => (
              <div
                key={page.id}
                data-entry-id={page.id}
                {...dragProps(page)}
                {...dropProps(page)}
                onClick={() => renaming !== page.id && open(page)}
                onContextMenu={(e) => showMenu(e, entryMenu(page))}
                className={clsx(
                  "group flex items-center gap-3 px-3 py-2 cursor-pointer hoverable border-b border-white/5 last:border-b-0",
                  dropTarget === page.id && "ring-1 ring-white/40 bg-white/[0.08]",
                )}
              >
                <EntryIcon page={page} className="size-4 shrink-0" />
                <div className="flex-1 min-w-0">
                  {renaming === page.id ? (
                    renameField()
                  ) : (
                    <div className="truncate text-sm text-gray-200" title={page.title}>
                      {page.title}
                    </div>
                  )}
                </div>
                <span className="tnum w-28 shrink-0 text-[11px] text-[var(--ink-faint)]">
                  {page.isFolder ? itemCount(folderCount(pages, page.id)) : modified(page.updatedAt)}
                </span>
                <div className="w-16 shrink-0 flex justify-end">
                  {rowActions(page)}
                </div>
              </div>
            ))}
          </div>
        )}

        {vaultUntouched && !query && (
          <p className="text-sm text-[var(--ink-muted)] pt-2">
            Your vault is empty. Make a folder to organize things, or start a page. Your daily notes
            file themselves under Journal.
          </p>
        )}
      </div>

      {menu && <ContextMenu anchor={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}
