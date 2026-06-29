import { useEffect, useRef, useState } from "react";
import { Send, Sparkles, Brain, X, Tag } from "lucide-react";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { suggestAutoLabels, type AutoLabelSuggestion } from "../lib/autoLabels";
import InferenceSetup from "../components/InferenceSetup";

type PendingAutoLabelSuggestion = Omit<AutoLabelSuggestion, "entityId" | "key"> & {
  key: string;
  entityIds: number[];
};

export default function ChatPane() {
  const llm = useStore((s) => s.llm);
  const busy = useStore((s) => s.busy);
  const plan = useStore((s) => s.plan);
  const addNote = useStore((s) => s.addNote);
  const loadPages = useStore((s) => s.loadPages);
  const quickLabel = useStore((s) => s.quickLabel);
  const setEntityLabels = useStore((s) => s.setEntityLabels);
  // Transcript lives in the store so it persists across page/settings changes (cleared on app close).
  const messages = useStore((s) => s.chatMessages);
  const setMessages = useStore((s) => s.setChatMessages);
  const pendingChat = useStore((s) => s.pendingChat);
  const setPendingChat = useStore((s) => s.setPendingChat);
  const [input, setInput] = useState("");
  // Plan = the harnessed calendar planner (schema-constrained); Chat = the deharnessed general
  // "second brain" assistant (free-form, RAG-grounded, grows with you). Same 7B, two modes. Lifted to
  // the store so the shell can widen the pane + hide the tasks panel in chat mode.
  const mode = useStore((s) => s.chatMode);
  const setMode = useStore((s) => s.setChatMode);
  const [chatBusy, setChatBusy] = useState(false);
  // Durable facts the AI noticed in the last message — offered for the user to confirm into memory.
  const [memSuggestions, setMemSuggestions] = useState<string[]>([]);
  // Deterministic label guesses for just-created tasks/events — confirmed before storing.
  const [labelSuggestions, setLabelSuggestions] = useState<PendingAutoLabelSuggestion[]>([]);
  const scrollRef = useRef<HTMLDivElement>(null);

  const saveMemory = async (fact: string) => {
    setMemSuggestions((s) => s.filter((f) => f !== fact));
    await addNote(fact);
    await loadPages();
  };

  const applyLabelSuggestion = async (suggestion: PendingAutoLabelSuggestion) => {
    setLabelSuggestions((s) => s.filter((x) => x.key !== suggestion.key));
    const labels = await quickLabel(suggestion.labelName, suggestion.color);
    const label = labels.find((l) => l.name.toLowerCase() === suggestion.labelName.toLowerCase());
    if (!label) return;
    for (const entityId of suggestion.entityIds) {
      const current = await api.labelsFor(suggestion.kind, entityId).catch(() => []);
      const next = [...new Set([...current.map((l) => l.id), label.id])];
      await setEntityLabels(suggestion.kind, entityId, next);
    }
  };

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
  }, [messages, busy, memSuggestions, labelSuggestions]);

  const send = async (text: string) => {
    const trimmed = text.trim();
    if (!trimmed || busy || chatBusy) return;
    // Conversation context (prior turns) so follow-ups like "this friday at 7pm" work.
    const history = messages.map((m) => ({ role: m.role === "ai" ? "assistant" : "user", content: m.text }));
    setInput("");
    setLabelSuggestions([]);
    setMemSuggestions([]);
    setMessages((m) => [...m, { role: "user", text: trimmed }]);

    // Auto mode classifies the message (planner vs assistant); Plan/Chat force the route.
    let route: "plan" | "chat" = mode === "chat" ? "chat" : "plan";
    if (mode === "auto") {
      setChatBusy(true);
      try {
        route = await api.routeIntent(trimmed);
      } catch {
        route = "plan"; // classify failed → fall back to the planner (the established default)
      }
      setChatBusy(false);
    }

    // Chat route → the deharnessed assistant: free-form reply + offer to remember durable facts (the
    // "grows with you" loop — saved facts become grounding context for future chats).
    if (route === "chat") {
      setChatBusy(true);
      try {
        const reply = await api.assistantChat(trimmed, history);
        setMessages((m) => [...m, { role: "ai", text: reply || "…" }]);
        api.extractMemories(trimmed).then((facts) => facts.length && setMemSuggestions(facts)).catch(() => {});
      } catch (e) {
        setMessages((m) => [...m, { role: "ai", text: "I couldn't respond — " + String(e) }]);
      } finally {
        setChatBusy(false);
      }
      return;
    }

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
      const latest = useStore.getState();
      const targets = [
        ...o.createdTaskIds
          .map((id) => {
            const task = latest.tasks.find((t) => t.id === id);
            return task ? { kind: "task" as const, id, title: task.title, text: task.notes } : null;
          })
          .filter((x): x is NonNullable<typeof x> => !!x),
        ...(o.createdEventIds ?? [])
          .map((id) => {
            const event = latest.events.find((e) => e.id === id);
            return event ? { kind: "event" as const, id, title: event.title } : null;
          })
          .filter((x): x is NonNullable<typeof x> => !!x),
      ];
      setLabelSuggestions(groupLabelSuggestions(suggestAutoLabels(targets, trimmed)));
      // Best-effort: notice durable facts worth remembering and offer to save them (confirmed, not silent).
      api
        .extractMemories(trimmed)
        .then((facts) => facts.length && setMemSuggestions(facts))
        .catch(() => {});
    } catch (e) {
      setMessages((m) => [...m, { role: "ai", text: "I couldn't plan that — " + String(e) }]);
    }
  };

  // A message handed off from the welcome screen's chat box — send it once, then clear.
  useEffect(() => {
    if (!pendingChat) return;
    const text = pendingChat;
    setPendingChat(null);
    send(text);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pendingChat]);

  return (
    <div className="h-full flex flex-col">
      <div className="px-4 py-2.5 border-b border-white/10 flex items-center gap-2 shrink-0">
        <Sparkles className="size-4 text-fuchsia-400" />
        <span className="text-sm font-medium flex-1">AI</span>
        {/* Plan = schema-harnessed calendar planner · Chat = deharnessed second-brain assistant. */}
        <div className="flex items-center rounded-lg bg-white/[0.06] p-0.5 text-xs">
          {(["auto", "plan", "chat"] as const).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              title={m === "auto" ? "Auto — pick Plan or Chat per message" : m === "plan" ? "Plan — calendar planner" : "Chat — second-brain assistant"}
              className={
                mode === m
                  ? "px-2.5 py-1 rounded-md bg-white/90 text-gray-900 font-medium"
                  : "px-2.5 py-1 rounded-md text-gray-400 hover:text-gray-200"
              }
            >
              {m.charAt(0).toUpperCase() + m.slice(1)}
            </button>
          ))}
        </div>
      </div>

      <div ref={scrollRef} className="flex-1 min-h-0 overflow-y-auto p-4 space-y-3">
        {llm && !llm.reachable && <InferenceSetup />}

        {messages.length === 0 && llm?.reachable && (
          <div className="text-sm text-gray-400 space-y-3">
            {mode === "chat" ? (
              <p>Think out loud, ask questions, or capture a thought. I’ll remember what matters and get more useful over time.</p>
            ) : mode === "plan" ? (
              <p>Describe what you’re working on in plain language and I’ll break it into tasks and schedule them.</p>
            ) : (
              <p>Tell me what to schedule or just talk — I’ll figure out whether to plan it or chat. Use the toggle above to force one.</p>
            )}
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

        {(busy || chatBusy) && <div className="text-xs text-gray-500">Thinking…</div>}
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

      {labelSuggestions.length > 0 && (
        <div className="px-3 pt-2 shrink-0 space-y-1.5">
          <div className="text-[11px] text-sky-300/80 flex items-center gap-1">
            <Tag className="size-3" /> Apply labels?
          </div>
          {labelSuggestions.map((suggestion) => (
            <div key={suggestion.key} className="flex items-center gap-2 rounded-lg border border-sky-400/20 bg-sky-500/[0.06] px-2.5 py-1.5">
              <span className="size-2 rounded-full shrink-0" style={{ background: suggestion.color }} />
              <span className="text-xs text-gray-200 flex-1 min-w-0">
                <span className="text-gray-400">{suggestion.kind === "task" ? "Task" : "Event"}</span>{" "}
                <span className="text-gray-100">{suggestion.entityTitle}</span>{" "}
                <span className="text-gray-500">as</span>{" "}
                <span style={{ color: suggestion.color }}>{suggestion.labelName}</span>
              </span>
              <button onClick={() => applyLabelSuggestion(suggestion)} className="text-[11px] px-2 py-0.5 rounded bg-sky-500/80 hover:bg-sky-500 text-white shrink-0">
                Apply
              </button>
              <button onClick={() => setLabelSuggestions((s) => s.filter((x) => x.key !== suggestion.key))} className="text-gray-500 hover:text-gray-300 shrink-0" title="Dismiss">
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
            placeholder={!llm?.reachable ? "Set up the AI above to start…" : mode === "plan" ? "Describe your projects and tasks…" : mode === "chat" ? "Ask anything, or think out loud…" : "Plan something, or just ask…"}
            className="flex-1 resize-none rounded-lg bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-indigo-500/50 placeholder:text-gray-600"
          />
          <button
            type="submit"
            disabled={busy || !input.trim()}
            className="size-9 shrink-0 grid place-items-center rounded-lg bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40"
          >
            <Send className="size-4" />
          </button>
        </div>
      </form>
    </div>
  );
}

function groupLabelSuggestions(suggestions: AutoLabelSuggestion[]): PendingAutoLabelSuggestion[] {
  const byKey = new Map<string, PendingAutoLabelSuggestion>();
  for (const suggestion of suggestions) {
    const groupKey =
      suggestion.kind === "event"
        ? `${suggestion.kind}:${suggestion.entityTitle.toLowerCase()}:${suggestion.labelName.toLowerCase()}`
        : suggestion.key;
    const existing = byKey.get(groupKey);
    if (existing) {
      existing.entityIds.push(suggestion.entityId);
    } else {
      const { entityId: _entityId, key: _key, ...rest } = suggestion;
      byKey.set(groupKey, { ...rest, key: groupKey, entityIds: [suggestion.entityId] });
    }
  }
  return [...byKey.values()];
}
