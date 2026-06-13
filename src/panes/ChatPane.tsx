import { useEffect, useRef, useState } from "react";
import { Send, Sparkles, Brain, X } from "lucide-react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import InferenceSetup from "../components/InferenceSetup";

export default function ChatPane() {
  const llm = useStore((s) => s.llm);
  const busy = useStore((s) => s.busy);
  const plan = useStore((s) => s.plan);
  const addNote = useStore((s) => s.addNote);
  const loadPages = useStore((s) => s.loadPages);
  // Transcript lives in the store so it persists across page/settings changes (cleared on app close).
  const messages = useStore((s) => s.chatMessages);
  const setMessages = useStore((s) => s.setChatMessages);
  const [input, setInput] = useState("");
  // Durable facts the AI noticed in the last message — offered for the user to confirm into memory.
  const [memSuggestions, setMemSuggestions] = useState<string[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);

  const saveMemory = async (fact: string) => {
    setMemSuggestions((s) => s.filter((f) => f !== fact));
    await addNote(fact);
    await loadPages();
  };

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, busy]);

  const send = async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed || busy) return;
    // Conversation context (prior turns) so follow-ups like "this friday at 7pm" work.
    const history = messages.map((m) => ({ role: m.role === "ai" ? "assistant" : "user", content: m.text }));
    setInput("");
    setMessages((m) => [...m, { role: "user", text: trimmed }]);
    try {
      const o = await plan(trimmed, history);
      const n = o.createdTaskIds.length;
      const ev = o.createdEventTitles.length;
      const hab = o.createdHabitNames.length;
      const upd = o.updatedEventTitles.length;
      const rem = o.removedEventTitles.length;

      const actions: string[] = [];
      const bits: string[] = [];
      if (n) bits.push(`${n} task${n === 1 ? "" : "s"}${o.projectNames.length ? ` to ${o.projectNames.join(", ")}` : ""}`);
      if (ev) bits.push(`${ev} event${ev === 1 ? "" : "s"} (${o.createdEventTitles.join(", ")})`);
      if (hab) bits.push(`${hab} habit${hab === 1 ? "" : "s"} (${o.createdHabitNames.join(", ")})`);
      if (bits.length) actions.push(`Added ${bits.join(" and ")}`);
      if (upd) actions.push(`updated ${upd} event${upd === 1 ? "" : "s"} (${[...new Set(o.updatedEventTitles)].join(", ")})`);
      if (rem) actions.push(`removed ${rem} event${rem === 1 ? "" : "s"}`);

      const parts: string[] = [];
      if (actions.length) {
        parts.push(actions.join(", ") + ", and re-planned your calendar.");
      } else if (!o.clarifications.length) {
        parts.push("I didn't catch anything to change — try giving a bit more detail.");
      }
      if (o.clarifications.length) {
        parts.push("A few things to confirm:\n" + o.clarifications.map((c) => "• " + c).join("\n"));
      }
      // Transparency: show which saved notes informed the plan.
      if (o.recalledNotes?.length) {
        parts.push("📌 Recalled from your notes:\n" + o.recalledNotes.map((r) => "• " + r).join("\n"));
      }
      setMessages((m) => [...m, { role: "ai", text: parts.join("\n\n") }]);
      // Best-effort: notice durable facts worth remembering and offer to save them (confirmed, not silent).
      api
        .extractMemories(trimmed)
        .then((facts) => facts.length && setMemSuggestions(facts))
        .catch(() => {});
    } catch (e) {
      setMessages((m) => [...m, { role: "ai", text: "I couldn't plan that — " + String(e) }]);
    }
  };

  return (
    <div className="h-full flex flex-col">
      <div className="px-4 py-3 border-b border-white/10 flex items-center gap-2 shrink-0">
        <Sparkles className="size-4 text-fuchsia-400" />
        <span className="text-sm font-medium">Plan with AI</span>
      </div>

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto p-4 space-y-3">
        {llm && !llm.reachable && <InferenceSetup />}

        {messages.length === 0 && llm?.reachable && (
          <div className="text-sm text-gray-400 space-y-3">
            <p>Describe what you’re working on in plain language and I’ll break it into tasks and schedule them.</p>
          </div>
        )}

        {messages.map((m, i) => (
          <div key={i} className={m.role === "user" ? "flex justify-end" : "flex justify-start"}>
            <div
              className={
                m.role === "user"
                  ? "max-w-[85%] rounded-2xl rounded-br-sm bg-indigo-500/80 text-white px-3 py-2 text-sm whitespace-pre-wrap"
                  : "max-w-[90%] rounded-2xl rounded-bl-sm bg-white/[0.06] text-gray-200 px-3 py-2 text-sm whitespace-pre-wrap"
              }
            >
              {m.text}
            </div>
          </div>
        ))}

        {busy && <div className="text-xs text-gray-500">Thinking…</div>}
      </div>

      {memSuggestions.length > 0 && (
        <div className="px-3 pt-2 shrink-0 space-y-1.5">
          <div className="text-[11px] text-fuchsia-300/80 flex items-center gap-1">
            <Brain className="size-3" /> Remember this?
          </div>
          {memSuggestions.map((fact) => (
            <div key={fact} className="flex items-center gap-2 rounded-lg border border-fuchsia-400/20 bg-fuchsia-500/[0.06] px-2.5 py-1.5">
              <span className="text-xs text-gray-200 flex-1 min-w-0">{fact}</span>
              <button onClick={() => saveMemory(fact)} className="text-[11px] px-2 py-0.5 rounded bg-fuchsia-500/80 hover:bg-fuchsia-500 text-white shrink-0">
                Save
              </button>
              <button onClick={() => setMemSuggestions((s) => s.filter((f) => f !== fact))} className="text-gray-500 hover:text-gray-300 shrink-0" title="Dismiss">
                <X className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      )}

      <form
        onSubmit={(e) => {
          e.preventDefault();
          send(input);
        }}
        className="p-3 border-t border-white/10 shrink-0"
      >
        <div className="flex items-end gap-2">
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send(input);
              }
            }}
            rows={2}
            placeholder={llm?.reachable ? "Describe your projects and tasks…" : "Set up the AI above to start planning…"}
            className="flex-1 resize-none rounded-lg bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-indigo-500/50 placeholder:text-gray-600"
          />
          <button
            type="submit"
            disabled={busy || !input.trim()}
            className="size-9 shrink-0 grid place-items-center rounded-lg bg-indigo-500 hover:bg-indigo-400 disabled:opacity-40"
          >
            <Send className="size-4" />
          </button>
        </div>
      </form>
    </div>
  );
}
