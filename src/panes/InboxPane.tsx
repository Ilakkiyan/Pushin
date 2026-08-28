import { useEffect, useState } from "react";
import { Inbox, Sparkles, FileText, Trash2, Loader2 } from "lucide-react";
import { useStore } from "../state/store";

/** The unsorted Inbox: quick captures awaiting triage. Each can be planned with AI (→ task/event),
 *  kept as a vault note, or deleted. The "one box, zero decisions" half of capture. */
export default function InboxPane() {
  const inbox = useStore((s) => s.inbox);
  const loadInbox = useStore((s) => s.loadInbox);
  const keepInboxNote = useStore((s) => s.keepInboxNote);
  const deletePage = useStore((s) => s.deletePage);
  const plan = useStore((s) => s.plan);
  const setCaptureOpen = useStore((s) => s.setCaptureOpen);
  const [busyId, setBusyId] = useState<number | null>(null);

  useEffect(() => {
    // A rejected load must not leave an unhandled rejection: the pane keeps whatever it had
    // (nothing, on first paint) rather than the promise escaping into the console. Every other
    // pane in the app already does this — these two were the exceptions.
    loadInbox().catch(() => {});
  }, [loadInbox]);

  const planItem = async (id: number, text: string) => {
    setBusyId(id);
    try {
      await plan(text, []);
      await deletePage(id);
      await loadInbox();
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (id: number) => {
    setBusyId(id);
    try {
      await deletePage(id);
      await loadInbox();
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-2xl mx-auto p-6 space-y-4">
        <div className="flex items-center justify-between">
          <h1 className="text-lg font-semibold flex items-center gap-2">
            <Inbox className="size-5 text-indigo-400" /> Inbox {inbox.length > 0 && <span className="tnum text-[var(--ink-faint)] font-normal">· {inbox.length}</span>}
          </h1>
          <button onClick={() => setCaptureOpen(true)} className="text-xs px-3 py-1.5 hoverable border border-white/10">
            Capture <kbd className="kbd ml-1">⌘⇧N</kbd>
          </button>
        </div>

        {inbox.length === 0 ? (
          <p className="text-sm text-[var(--ink-muted)] py-10 text-center">Inbox zero. Capture a thought with ⌘/Ctrl+Shift+N, then sort it here later.</p>
        ) : (
          <div className="space-y-2">
            {inbox.map((item) => (
              <div key={item.id} className="border border-white/10 bg-white/[0.02] p-3">
                <div className="text-sm text-gray-200 whitespace-pre-wrap">{item.content}</div>
                <div className="mt-2 flex items-center gap-2">
                  <button
                    onClick={() => planItem(item.id, item.content)}
                    disabled={busyId === item.id}
                    className="flex items-center gap-1.5 text-xs px-2.5 py-1 bg-white/90 hover:bg-white text-gray-900 font-medium disabled:opacity-40"
                    title="Let the AI turn this into a task or event"
                  >
                    {busyId === item.id ? <Loader2 className="size-3.5 animate-spin" /> : <Sparkles className="size-3.5" />} Plan with AI
                  </button>
                  <button
                    onClick={() => keepInboxNote(item.id)}
                    disabled={busyId === item.id}
                    className="flex items-center gap-1.5 text-xs px-2.5 py-1 hoverable border border-white/10 disabled:opacity-40"
                    title="Keep as a vault note"
                  >
                    <FileText className="size-3.5" /> Keep as note
                  </button>
                  <button
                    onClick={() => remove(item.id)}
                    disabled={busyId === item.id}
                    className="ml-auto text-gray-500 hover:text-rose-400 disabled:opacity-40"
                    title="Delete"
                  >
                    <Trash2 className="size-4" />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
