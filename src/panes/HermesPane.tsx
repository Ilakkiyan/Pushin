import { useEffect, useMemo, useState } from "react";
import { Brain, Loader2, Search, Sparkles, Trash2 } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type RecallResult } from "../lib/ipc";
import { parseLocal } from "../lib/time";

const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-indigo-500/50 placeholder:text-gray-600";

function when(iso: string): string {
  try {
    return parseLocal(iso).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
  } catch {
    return iso;
  }
}

export default function HermesPane() {
  const notes = useStore((s) => s.notes);
  const loadNotes = useStore((s) => s.loadNotes);
  const addNote = useStore((s) => s.addNote);
  const deleteNote = useStore((s) => s.deleteNote);
  const recallNotes = useStore((s) => s.recallNotes);

  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<RecallResult | null>(null);
  const [recalling, setRecalling] = useState(false);
  const [err, setErr] = useState("");

  useEffect(() => {
    loadNotes().catch((e) => setErr(String(e)));
    // Make sure the on-device memory engine is coming up (auto-downloads its tiny embedder on
    // first use). Best-effort: if it's not ready, recall just uses keyword search.
    api.ensureEmbeddings().catch(() => {});
  }, [loadNotes]);

  // If notes exist but none are indexed, semantic recall isn't wired up yet.
  const semanticOff = useMemo(() => notes.length > 0 && notes.every((n) => !n.indexed), [notes]);

  const save = async () => {
    const text = draft.trim();
    if (!text || saving) return;
    setSaving(true);
    setErr("");
    try {
      await addNote(text);
      setDraft("");
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  const recall = async () => {
    const q = query.trim();
    if (!q || recalling) return;
    setRecalling(true);
    setErr("");
    try {
      setResults(await recallNotes(q, 8));
    } catch (e) {
      setErr(String(e));
    } finally {
      setRecalling(false);
    }
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-3xl mx-auto p-4 sm:p-6 space-y-6">
        {/* Header */}
        <div className="space-y-1">
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <Brain className="size-5 text-fuchsia-400" /> Hermes
          </h1>
          <p className="text-sm text-gray-400">
            Your second brain. Jot down anything — Pushin remembers it and recalls the relevant bits when you ask.
          </p>
        </div>

        {/* Capture */}
        <section className="space-y-2">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
                e.preventDefault();
                save();
              }
            }}
            rows={3}
            placeholder="Remember this… (e.g. 'Sarah prefers afternoon meetings', 'gym is Tue/Thu', ideas, links)"
            className={clsx(inputCls, "resize-none")}
          />
          <div className="flex items-center gap-3">
            <button
              onClick={save}
              disabled={!draft.trim() || saving}
              className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400 disabled:opacity-50"
            >
              {saving ? <Loader2 className="size-4 animate-spin" /> : <Sparkles className="size-4" />}
              {saving ? "Saving…" : "Remember"}
            </button>
            <span className="text-[11px] text-gray-600">⌘/Ctrl + Enter</span>
          </div>
        </section>

        {/* Recall */}
        <section className="space-y-3">
          <form
            onSubmit={(e) => {
              e.preventDefault();
              recall();
            }}
            className="flex items-center gap-2"
          >
            <div className="relative flex-1">
              <Search className="size-4 text-gray-500 absolute left-3 top-1/2 -translate-y-1/2" />
              <input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Recall… ask what you noted about something"
                className={clsx(inputCls, "pl-9")}
              />
            </div>
            <button
              type="submit"
              disabled={!query.trim() || recalling}
              className="text-sm px-3 py-2 rounded-lg bg-white/10 hover:bg-white/15 disabled:opacity-50"
            >
              {recalling ? <Loader2 className="size-4 animate-spin" /> : "Recall"}
            </button>
          </form>

          {results && (
            <div className="space-y-2">
              <div className="text-[11px] text-gray-500 flex items-center gap-2">
                {results.notes.length > 0 ? (
                  <>
                    <span>{results.notes.length} match{results.notes.length === 1 ? "" : "es"}</span>
                    <span className="px-1.5 py-0.5 rounded bg-white/10 text-gray-300">{results.mode}</span>
                  </>
                ) : (
                  <span>No matches.</span>
                )}
              </div>
              {results.notes.map((n) => (
                <div key={n.id} className="rounded-lg border border-fuchsia-400/20 bg-fuchsia-500/[0.04] p-3">
                  <div className="text-sm text-gray-200 whitespace-pre-wrap">{n.content}</div>
                  <div className="mt-1 text-[11px] text-gray-500 flex items-center gap-2">
                    <span>{when(n.createdAt)}</span>
                    {n.score != null && <span className="text-fuchsia-300/80">{Math.round(n.score * 100)}% match</span>}
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>

        {err && <p className="text-xs text-rose-400">{err}</p>}

        {semanticOff && (
          <p className="text-[11px] text-amber-400/80 leading-relaxed">
            Setting up semantic memory… Pushin is preparing the on-device memory engine (a one-time ~37 MB download that
            runs automatically — no setup needed). Until it's ready, recall uses <span className="text-gray-300">keyword</span>{" "}
            search and your notes are indexed as soon as it comes online.
          </p>
        )}

        {/* All notes */}
        <section className="space-y-2">
          <h2 className="text-sm font-semibold text-gray-300">All notes {notes.length > 0 && <span className="text-gray-600">({notes.length})</span>}</h2>
          {notes.length === 0 ? (
            <p className="text-xs text-gray-500">Nothing yet. Write your first note above — Hermes grows as you feed it.</p>
          ) : (
            <div className="space-y-2">
              {notes.map((n) => (
                <div key={n.id} className="group rounded-lg border border-white/10 bg-white/[0.02] p-3 flex items-start gap-3">
                  <span
                    className={clsx("mt-1.5 size-1.5 rounded-full shrink-0", n.indexed ? "bg-emerald-400" : "bg-gray-600")}
                    title={n.indexed ? "Indexed for semantic recall" : "Keyword-only (not embedded)"}
                  />
                  <div className="min-w-0 flex-1">
                    <div className="text-sm text-gray-200 whitespace-pre-wrap">{n.content}</div>
                    <div className="mt-1 text-[11px] text-gray-500">{when(n.createdAt)}</div>
                  </div>
                  <button
                    onClick={() => deleteNote(n.id)}
                    className="opacity-0 group-hover:opacity-100 p-1.5 rounded-md text-gray-500 hover:text-rose-300 hover:bg-white/5"
                    title="Delete"
                  >
                    <Trash2 className="size-4" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
