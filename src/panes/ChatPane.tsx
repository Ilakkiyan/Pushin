import { useEffect, useRef, useState } from "react";
import { Send, Sparkles } from "lucide-react";
import { useStore } from "../state/store";
import InferenceSetup from "../components/InferenceSetup";

type Msg = { role: "user" | "ai"; text: string };

const EXAMPLES = [
  "Launch a side project in 3 weeks: design a logo, build a landing page, write 3 blog posts, set up analytics.",
  "Prep for my exam next Friday: review 4 chapters, do 2 practice tests, make a cheat sheet.",
];

export default function ChatPane() {
  const llm = useStore((s) => s.llm);
  const busy = useStore((s) => s.busy);
  const plan = useStore((s) => s.plan);
  const [messages, setMessages] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

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
      const upd = o.updatedEventTitles.length;
      const rem = o.removedEventTitles.length;

      const actions: string[] = [];
      const bits: string[] = [];
      if (n) bits.push(`${n} task${n === 1 ? "" : "s"}${o.projectNames.length ? ` to ${o.projectNames.join(", ")}` : ""}`);
      if (ev) bits.push(`${ev} event${ev === 1 ? "" : "s"} (${o.createdEventTitles.join(", ")})`);
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
      setMessages((m) => [...m, { role: "ai", text: parts.join("\n\n") }]);
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
            <div className="space-y-2">
              {EXAMPLES.map((ex) => (
                <button
                  key={ex}
                  onClick={() => send(ex)}
                  className="block w-full text-left text-xs rounded-lg border border-white/10 px-3 py-2 text-gray-300 hover:bg-white/5"
                >
                  {ex}
                </button>
              ))}
            </div>
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
