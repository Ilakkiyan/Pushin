import { useEffect, useRef, useState } from "react";
import { ArrowRight, CalendarDays, Clock, ListChecks } from "lucide-react";
import { api, type Briefing } from "../lib/ipc";
import { parseLocal } from "../lib/time";

/**
 * The returning-user landing, shown after the opening animation: a time-aware greeting, today's
 * agenda (the deterministic Daily Briefing), and a prominent chat box to start thinking out loud.
 * `onEnter(text?)` hands control to the app — with `text`, that message is sent into the chat.
 */
export default function WelcomeBack({ onEnter }: { onEnter: (text?: string) => void }) {
  const [briefing, setBriefing] = useState<Briefing | null>(null);
  const [text, setText] = useState("");
  const taRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    Promise.resolve()
      .then(() => api.dailyBriefing())
      .then(setBriefing)
      .catch(() => {});
    taRef.current?.focus();
  }, []);

  const hour = new Date().getHours();
  const greeting = hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
  const submit = () => onEnter(text.trim() || undefined);

  const fmt = (iso: string) => parseLocal(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const focus = briefing && briefing.focusMinutes >= 60 ? `${(briefing.focusMinutes / 60).toFixed(1)}h` : `${briefing?.focusMinutes ?? 0}m`;
  const hasAgenda = !!briefing && (briefing.events.length > 0 || briefing.dueTasks.length > 0);

  return (
    <div className="fixed inset-0 z-50 flex flex-col items-center justify-center bg-[var(--bg)] px-6 welcome-in">
      <div className="w-full max-w-xl">
        <div className="wordmark mb-12 text-center text-xl text-gray-400/80" style={{ letterSpacing: "0.34em" }}>
          Pushin
        </div>

        <h1 className="text-3xl font-light tracking-tight text-gray-100">{greeting}.</h1>
        <p className="mt-1.5 text-sm text-gray-500">{briefing ? `Here's your ${briefing.weekday}.` : "Welcome back."}</p>

        {hasAgenda && (
          <div className="mt-6 rounded-xl border border-white/10 bg-white/[0.02] p-4 space-y-3">
            <div className="flex items-center gap-4 text-xs text-gray-500">
              <span className="flex items-center gap-1.5"><CalendarDays className="size-3.5" />{briefing!.events.length} events</span>
              <span className="flex items-center gap-1.5"><ListChecks className="size-3.5" />{briefing!.dueTasks.length} due</span>
              {briefing!.focusMinutes > 0 && <span className="flex items-center gap-1.5"><Clock className="size-3.5" />{focus} focus</span>}
            </div>
            {briefing!.events.slice(0, 3).map((e) => (
              <div key={e.id} className="flex items-center gap-3 text-sm">
                <span className="w-16 shrink-0 text-gray-500 tabular-nums">{fmt(e.start)}</span>
                <span className="truncate text-gray-200">{e.title}</span>
              </div>
            ))}
            {briefing!.dueTasks.length > 0 && (
              <div className="flex flex-wrap gap-1.5 pt-1">
                {briefing!.dueTasks.slice(0, 5).map((t) => (
                  <span key={t.id} className="rounded-full border border-white/10 bg-white/[0.04] px-2 py-0.5 text-xs text-gray-300">
                    {t.title}
                  </span>
                ))}
              </div>
            )}
          </div>
        )}

        <div className="mt-7">
          <div className="rounded-2xl border border-white/12 bg-white/[0.03] p-1.5 focus-within:border-white/25 transition">
            <textarea
              ref={taRef}
              value={text}
              onChange={(e) => setText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  submit();
                }
              }}
              rows={2}
              placeholder="What's on your mind? Plan your day, jot a thought…"
              className="w-full resize-none bg-transparent px-3 py-2 text-sm text-gray-100 outline-none placeholder:text-gray-600"
            />
            <div className="flex items-center justify-between px-2 pb-1">
              <span className="text-[11px] text-gray-600">Enter to start · Shift+Enter for a new line</span>
              <button
                onClick={submit}
                className="inline-flex items-center gap-1.5 rounded-lg bg-white/90 px-3 py-1.5 text-xs font-medium text-gray-900 hover:bg-white transition"
              >
                Start <ArrowRight className="size-3.5" />
              </button>
            </div>
          </div>
        </div>

        <button onClick={() => onEnter()} className="mt-5 text-xs text-gray-600 hover:text-gray-400 transition">
          Skip to app →
        </button>
      </div>
    </div>
  );
}
