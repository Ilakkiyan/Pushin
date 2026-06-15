import { useEffect, useRef, useState } from "react";
import { Search, FileText, Plus, CalendarDays, FolderKanban, Flame, CalendarClock, Network, Settings as SettingsIcon, Sparkles, Zap, ArrowLeft, Loader2 } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type Page, type PlanOutcome, type VaultAnswer } from "../lib/ipc";

/** A one-line summary of what a natural-language command did, for the action-bar result. */
function summarizeOutcome(o: PlanOutcome): string {
  const parts: string[] = [];
  if (o.createdEventTitles.length) parts.push(`added ${o.createdEventTitles.length} event${o.createdEventTitles.length === 1 ? "" : "s"} (${o.createdEventTitles.join(", ")})`);
  if (o.createdTaskIds.length) parts.push(`added ${o.createdTaskIds.length} task${o.createdTaskIds.length === 1 ? "" : "s"}`);
  if (o.updatedEventTitles.length) parts.push(`updated ${o.updatedEventTitles.join(", ")}`);
  if (o.removedEventTitles.length) parts.push(`removed ${o.removedEventTitles.join(", ")}`);
  if (o.createdHabitNames.length) parts.push(`added habit ${o.createdHabitNames.join(", ")}`);
  if (parts.length === 0) return o.clarifications[0] ?? "Nothing to change.";
  const s = parts.join(", ");
  return s.charAt(0).toUpperCase() + s.slice(1) + ".";
}

type Item = { key: string; label: string; icon: React.ReactNode; hint?: string; keepOpen?: boolean; run: () => void };

/** Cmd/Ctrl-K palette: jump to any page (full-text search) or any view, or create a page. */
export default function CommandPalette() {
  const setView = useStore((s) => s.setView);
  const openPage = useStore((s) => s.openPage);
  const createPage = useStore((s) => s.createPage);
  const allPages = useStore((s) => s.pages);
  const labels = useStore((s) => s.labels);
  const openLabel = useStore((s) => s.openLabel);
  const askVault = useStore((s) => s.askVault);
  const plan = useStore((s) => s.plan);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [pages, setPages] = useState<Page[]>([]);
  const [mode, setMode] = useState<"semantic" | "keyword" | null>(null);
  const [sel, setSel] = useState(0);
  const [answer, setAnswer] = useState<VaultAnswer | null>(null);
  const [asking, setAsking] = useState(false);
  const [running, setRunning] = useState(false);
  const [planMsg, setPlanMsg] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Global hotkey to toggle the palette.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((v) => !v);
      } else if (e.key === "Escape") {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Reset + focus on open.
  useEffect(() => {
    if (open) {
      setQuery("");
      setPages([]);
      setSel(0);
      setAnswer(null);
      setAsking(false);
      setRunning(false);
      setPlanMsg(null);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  // Debounced page search. Prefers on-device semantic recall (find by meaning) when the embedding
  // engine is up; falls back to keyword title/body search otherwise.
  useEffect(() => {
    if (!open) return;
    const q = query.trim();
    if (!q) {
      setPages([]);
      setMode(null);
      return;
    }
    const t = setTimeout(async () => {
      try {
        const r = await api.hermesRecall(q, 8);
        if (r.mode === "semantic" && r.notes.length) {
          const mapped = r.notes.map((n) => allPages.find((p) => p.id === n.id)).filter((p): p is Page => !!p);
          if (mapped.length) {
            setPages(mapped);
            setMode("semantic");
            return;
          }
        }
      } catch {
        /* fall through to keyword */
      }
      setPages(await api.searchPages(q).catch(() => []));
      setMode("keyword");
    }, 150);
    return () => clearTimeout(t);
  }, [query, open, allPages]);

  if (!open) return null;

  const close = () => setOpen(false);
  const q = query.trim().toLowerCase();

  const views: Item[] = [
    { key: "v:calendar", label: "Calendar", icon: <CalendarDays className="size-4" />, run: () => setView("calendar") },
    { key: "v:projects", label: "Projects", icon: <FolderKanban className="size-4" />, run: () => setView("projects") },
    { key: "v:habits", label: "Habits", icon: <Flame className="size-4" />, run: () => setView("habits") },
    { key: "v:booking", label: "Booking", icon: <CalendarClock className="size-4" />, run: () => setView("booking") },
    { key: "v:graph", label: "Graph", icon: <Network className="size-4" />, run: () => setView("graph") },
    { key: "v:settings", label: "Settings", icon: <SettingsIcon className="size-4" />, run: () => setView("settings") },
  ].filter((v) => !q || v.label.toLowerCase().includes(q));

  const labelItems: Item[] = labels
    .filter((l) => !q || l.name.toLowerCase().includes(q))
    .slice(0, 6)
    .map((l) => ({
      key: `l:${l.id}`,
      label: l.name,
      icon: <span className="size-2.5 rounded-full inline-block" style={{ background: l.color }} />,
      hint: "Label",
      run: () => openLabel(l.id),
    }));

  const pageItems: Item[] = pages.map((p) => ({
    key: `p:${p.id}`,
    label: p.title,
    icon: p.icon ? <span className="text-sm leading-none">{p.icon}</span> : <FileText className="size-4 text-gray-500" />,
    hint: "Page",
    run: () => openPage(p.id),
  }));

  const runAsk = async () => {
    const question = query.trim();
    if (!question) return;
    setAsking(true);
    setAnswer(null);
    try {
      setAnswer(await askVault(question));
    } catch {
      setAnswer({ answer: "Something went wrong asking your vault.", citations: [] });
    } finally {
      setAsking(false);
    }
  };

  const runCommand = async () => {
    const text = query.trim();
    if (!text) return;
    setRunning(true);
    setPlanMsg(null);
    try {
      setPlanMsg(summarizeOutcome(await plan(text, [])));
    } catch {
      setPlanMsg("Couldn't run that — is the AI set up? Try rephrasing.");
    } finally {
      setRunning(false);
    }
  };

  const actions: Item[] = q
    ? [
        { key: "run", label: `Run: "${query.trim()}"`, icon: <Zap className="size-4 text-indigo-400" />, hint: "AI", keepOpen: true, run: runCommand },
        { key: "ask", label: `Ask your vault: "${query.trim()}"`, icon: <Sparkles className="size-4 text-fuchsia-400" />, keepOpen: true, run: runAsk },
        { key: "new", label: `Create page "${query.trim()}"`, icon: <Plus className="size-4" />, run: () => createPage(null) },
      ]
    : [];

  const items = [...pageItems, ...labelItems, ...views, ...actions];
  const activate = (i: number) => {
    const it = items[i];
    if (!it) return;
    it.run();
    if (!it.keepOpen) close();
  };

  const citedPages = answer ? answer.citations.map((id) => allPages.find((p) => p.id === id)).filter((p): p is Page => !!p) : [];

  return (
    <div className="fixed inset-0 z-50 bg-black/50 flex items-start justify-center pt-[15vh]" onClick={close}>
      <div
        className="w-full max-w-lg mx-4 rounded-xl bg-[#0e1117] border border-white/10 shadow-2xl overflow-hidden"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 px-4 border-b border-white/10">
          <Search className="size-4 text-gray-500" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSel(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") {
                e.preventDefault();
                setSel((s) => Math.min(s + 1, items.length - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setSel((s) => Math.max(s - 1, 0));
              } else if (e.key === "Enter") {
                e.preventDefault();
                activate(sel);
              }
            }}
            placeholder="Search, run a command, jump to a view, or ask…"
            className="flex-1 bg-transparent py-3 text-sm outline-none placeholder:text-gray-600"
          />
          {mode && pages.length > 0 && (
            <span className="text-[10px] px-1.5 py-0.5 rounded bg-white/10 text-gray-400">{mode}</span>
          )}
          <kbd className="text-[10px] text-gray-600 border border-white/10 rounded px-1.5 py-0.5">esc</kbd>
        </div>
        <div className="max-h-[50vh] overflow-y-auto py-1">
          {running || planMsg ? (
            <div className="p-4">
              <button onClick={() => { setPlanMsg(null); setRunning(false); inputRef.current?.focus(); }} className="text-[11px] text-gray-500 hover:text-gray-300 flex items-center gap-1 mb-3">
                <ArrowLeft className="size-3" /> back to search
              </button>
              {running ? (
                <div className="flex items-center gap-2 text-sm text-gray-400">
                  <Loader2 className="size-4 animate-spin" /> Running…
                </div>
              ) : (
                <p className="text-sm text-gray-200 leading-relaxed">{planMsg}</p>
              )}
            </div>
          ) : asking || answer ? (
            <div className="p-4">
              <button onClick={() => { setAnswer(null); setAsking(false); inputRef.current?.focus(); }} className="text-[11px] text-gray-500 hover:text-gray-300 flex items-center gap-1 mb-3">
                <ArrowLeft className="size-3" /> back to search
              </button>
              {asking ? (
                <div className="flex items-center gap-2 text-sm text-gray-400">
                  <Loader2 className="size-4 animate-spin" /> Asking your vault…
                </div>
              ) : (
                answer && (
                  <>
                    <p className="text-sm text-gray-200 whitespace-pre-wrap leading-relaxed">{answer.answer}</p>
                    {citedPages.length > 0 && (
                      <div className="mt-3 pt-3 border-t border-white/10">
                        <div className="text-[10px] uppercase tracking-wider text-gray-600 mb-1.5">Sources</div>
                        <div className="space-y-1">
                          {citedPages.map((p) => (
                            <button
                              key={p.id}
                              onClick={() => { openPage(p.id); close(); }}
                              className="w-full flex items-center gap-2 text-left text-xs px-2 py-1.5 rounded text-indigo-300 hover:bg-white/5"
                            >
                              <FileText className="size-3.5 shrink-0" />
                              <span className="truncate">{p.title}</span>
                            </button>
                          ))}
                        </div>
                      </div>
                    )}
                  </>
                )
              )}
            </div>
          ) : items.length === 0 ? (
            <p className="px-4 py-6 text-center text-xs text-gray-600">Type to search, or ask your vault a question…</p>
          ) : (
            items.map((it, i) => (
              <button
                key={it.key}
                onMouseEnter={() => setSel(i)}
                onClick={() => activate(i)}
                className={clsx(
                  "w-full flex items-center gap-3 px-4 py-2 text-left text-sm",
                  i === sel ? "bg-white/10 text-white" : "text-gray-300",
                )}
              >
                <span className="shrink-0">{it.icon}</span>
                <span className="truncate flex-1">{it.label}</span>
                {it.hint && <span className="text-[10px] text-gray-600">{it.hint}</span>}
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
