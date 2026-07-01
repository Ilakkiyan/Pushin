import { ArrowRight, CalendarClock, FolderOpen, Keyboard, Sparkles } from "lucide-react";

/**
 * The post-update "What's new" intro, shown once after the app updates + restarts (App.tsx detects a
 * version bump). An italicized title fades in, then the new features rise in one by one as cards.
 *
 * Update this list each release — it's the changelog the user actually sees. Keep it to the headline
 * few; this is a welcome, not release notes.
 */
const FEATURES: { icon: typeof Sparkles; title: string; body: string }[] = [
  {
    icon: Sparkles,
    title: "A fresh look",
    body: "A calmer, minimalist charcoal interface, a wide new wordmark, and a smoother opening.",
  },
  {
    icon: Keyboard,
    title: "Keyboard-first",
    body: "Press g then a key to jump anywhere — Calendar, Vault, Tasks… ⌘K opens the palette for the rest.",
  },
  {
    icon: CalendarClock,
    title: "A faster calendar",
    body: "Arrow-select any time slot, double-click to create, and ⌘T spins up a linked note for an event.",
  },
  {
    icon: FolderOpen,
    title: "Your vault, as real files",
    body: "Mirror notes to markdown in a folder you choose — edit them in Pushin or any editor, both stay in sync.",
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
