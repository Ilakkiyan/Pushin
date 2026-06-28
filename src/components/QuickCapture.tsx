import { useEffect, useRef, useState } from "react";
import { Inbox, Loader2 } from "lucide-react";
import { useStore } from "../state/store";

/** One-box quick capture (Cmd/Ctrl+Shift+N): jot anything, it lands in the Inbox to sort later.
 *  Zero decisions — no picking task vs event vs note up front. */
export default function QuickCapture() {
  const open = useStore((s) => s.captureOpen);
  const setOpen = useStore((s) => s.setCaptureOpen);
  const captureNote = useStore((s) => s.captureNote);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);
  const ref = useRef<HTMLTextAreaElement | null>(null);

  // Global hotkey to open.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "n") {
        e.preventDefault();
        setOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [setOpen]);

  useEffect(() => {
    if (open) {
      setText("");
      requestAnimationFrame(() => ref.current?.focus());
    }
  }, [open]);

  if (!open) return null;

  const save = async () => {
    const t = text.trim();
    if (!t || saving) return;
    setSaving(true);
    try {
      await captureNote(t);
      setOpen(false);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fade-in fixed inset-0 z-50 bg-black/50 flex items-start justify-center pt-[20vh]" onClick={() => setOpen(false)}>
      <div className="pop-in w-full max-w-lg mx-4 rounded-xl bg-[var(--raised)] border border-white/10 shadow-2xl overflow-hidden" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2 px-4 py-2 border-b border-white/10 text-sm text-gray-400">
          <Inbox className="size-4 text-indigo-400" /> Quick capture
          <span className="ml-auto text-[10px] text-gray-600">⌘/Ctrl+Enter to save · Esc to close</span>
        </div>
        <textarea
          ref={ref}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setOpen(false);
            else if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
              e.preventDefault();
              save();
            }
          }}
          rows={4}
          placeholder="Capture a thought, task, idea, link… sort it later from the Inbox."
          className="w-full bg-transparent px-4 py-3 text-sm outline-none resize-none placeholder:text-gray-600"
        />
        <div className="flex items-center justify-end gap-2 px-4 py-2 border-t border-white/10">
          <button onClick={() => setOpen(false)} className="text-xs px-3 py-1.5 rounded-md text-gray-400 hover:text-white hover:bg-white/5">
            Cancel
          </button>
          <button
            onClick={save}
            disabled={!text.trim() || saving}
            className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40"
          >
            {saving ? <Loader2 className="size-3.5 animate-spin" /> : <Inbox className="size-3.5" />}
            Capture
          </button>
        </div>
      </div>
    </div>
  );
}
