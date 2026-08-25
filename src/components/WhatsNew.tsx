import { ArrowRight, CalendarPlus, MessagesSquare, Sparkles, Sun, Zap } from "lucide-react";

/**
 * The post-update "What's new" intro, shown once after the app updates + restarts (App.tsx detects a
 * version bump). An italicized title fades in, then the new features rise in one by one as cards.
 *
 * Update this list each release — it's the changelog the user actually sees. Keep it to the headline
 * few; this is a welcome, not release notes.
 */
const FEATURES: { icon: typeof Sparkles; title: string; body: string }[] = [
  {
    icon: Sun,
    title: "Opens on your day, not a grid",
    body: "Today is the new home screen — what's on, what's next, and your briefing in one place. The sidebar now holds one space at a time, so the vault's notes and graph no longer crowd your day.",
  },
  {
    icon: Sparkles,
    title: "A model trained for this",
    body: "Pushin now ships its own on-device model, tuned to read plans far more reliably than the general-purpose one it used to borrow — same size, same privacy, better answers.",
  },
  {
    icon: CalendarPlus,
    title: "Subscribe to any calendar",
    body: "Paste an .ics feed — a shared calendar, a team schedule, holidays — and Pushin plans your tasks around it. Read-only: it never edits or sends anything back.",
  },
  {
    icon: MessagesSquare,
    title: "Tells you why",
    body: "Every auto-scheduled block can now explain itself: waiting on something else, held for a deadline, or just the earliest free slot.",
  },
  {
    icon: Zap,
    title: "Lighter on your machine",
    body: "The AI starts faster and the first reply is quicker. When you step away, the model quietly unloads to free up memory (several GB) and springs back the moment you plan or chat again. Tune or turn it off under Settings ▸ On-device AI.",
  },
];
const TITLE_DELAY = 80;
const FIRST_CARD = 440;
const STEP = 150;

export default function WhatsNew({ version, onDone }: { version?: string; onDone: () => void }) {
  const ctaDelay = FIRST_CARD + FEATURES.length * STEP + 120;

  return (
    <div data-tauri-drag-region className="fixed inset-0 z-[60] flex flex-col items-center justify-center overflow-y-auto bg-[var(--bg)] px-6 py-10">
      <div className="w-full max-w-lg">
        <div className="welcome-in text-center" style={{ animationDelay: `${TITLE_DELAY}ms` }}>
          <h1 className="text-3xl font-light tracking-tight text-gray-100">
            Welcome to the <em className="font-normal italic text-white">new</em>{" "}
            <span className="wordmark text-white" style={{ fontSize: "0.82em", letterSpacing: "0.05em" }}>
              Pushin
            </span>
          </h1>
          {version && <p className="mt-2.5 text-xs tracking-wide text-gray-600">Version {version}</p>}
        </div>

        <div className="mt-8 space-y-2.5">
          {FEATURES.map((f, i) => {
            const Icon = f.icon;
            return (
              <div
                key={f.title}
                className="wn-rise flex items-start gap-3.5 rounded-xl border border-white/10 bg-white/[0.03] p-3.5"
                style={{ animationDelay: `${FIRST_CARD + i * STEP}ms` }}
              >
                <div className="mt-0.5 grid size-9 shrink-0 place-items-center rounded-lg bg-white/[0.06] text-gray-200">
                  <Icon className="size-[18px]" />
                </div>
                <div>
                  <div className="text-sm font-medium text-gray-100">{f.title}</div>
                  <div className="mt-0.5 text-xs leading-relaxed text-gray-500">{f.body}</div>
                </div>
              </div>
            );
          })}
        </div>

        <div className="wn-rise mt-8 flex justify-center" style={{ animationDelay: `${ctaDelay}ms` }}>
          <button
            onClick={onDone}
            className="inline-flex items-center gap-1.5 rounded-lg bg-white/90 px-4 py-2 text-sm font-medium text-gray-900 transition hover:bg-white"
          >
            Explore <ArrowRight className="size-4" />
          </button>
        </div>
      </div>
    </div>
  );
}
