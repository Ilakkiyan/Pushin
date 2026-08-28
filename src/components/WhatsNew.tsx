import { useMemo } from "react";
import { ArrowRight, CalendarPlus, Combine, FolderOpen, FolderSync, Layers, Link2, MessagesSquare, RotateCcw, Sparkles, Sun, Zap } from "lucide-react";
import { isNewer } from "../lib/version";

export type Feature = {
  icon: typeof Sparkles;
  title: string;
  body: string;
  /** The release this shipped in. Drives which cards a given update shows — see `featuresSince`. */
  since: string;
};

/**
 * The post-update "What's new" intro, shown after the app updates + restarts (App.tsx detects a
 * version bump). An italicized title fades in, then the new features rise in one by one as cards.
 *
 * Add an entry each release, tagged with the version it ships in, and leave the older ones alone:
 * the overlay shows only what is new to THIS user, so someone skipping three releases sees all three
 * releases' cards and someone who updates every time sees only the latest. Keep each release to its
 * headline few — this is a welcome, not release notes. Newest first.
 */
const FEATURES: Feature[] = [
  {
    icon: FolderOpen,
    since: "0.8.4",
    title: "Your vault is a place now",
    body: "Open the vault and you land in a file browser rather than whatever page you had open last: folders and documents as cards or a list, with a breadcrumb back to the top. Make folders, drag things into them, rename in place. Your daily notes file themselves under Journal.",
  },
  {
    icon: Layers,
    since: "0.8.4",
    title: "The pages you're working across",
    body: "Everything you open stays listed under Open in the sidebar, so moving between two documents is one click instead of hunting the tree for them again. Close one and you land back on the one before it.",
  },
  {
    icon: FolderSync,
    since: "0.8.4",
    title: "Your files travel with your notes",
    body: "Your notes already travelled between paired devices. Now the things sitting next to them do too, the attachments, PDFs and images in your vault folder. Set that folder under Settings ▸ Vault folder. Both devices need this version for files to move between them; one still on the old version keeps syncing everything else exactly as before.",
  },
  {
    icon: Combine,
    since: "0.8.3",
    title: "One task, however it's split",
    body: "A task the scheduler split around a meeting showed up as two identical-looking events, and dragging one moved only that half. Now dragging any piece moves the whole task: it merges back into a single block wherever there's room, and splits again only around what's genuinely in the way.",
  },
  {
    icon: RotateCcw,
    since: "0.8.2",
    title: "Nothing gets quietly lost",
    body: "A task whose planned time came and went unfinished is moved to your next free slot instead of sitting in yesterday, and it carries a mark showing how many times that's happened, so the one you keep pushing is obvious. Subscribed calendars now refresh themselves, too.",
  },
  {
    icon: Link2,
    since: "0.8.1",
    title: "Connect Google once, on any device",
    body: "Pair your devices and connecting Google on one connects them all. The link travels over your private mesh, and the token itself goes into each machine's own keychain. Disconnecting anywhere disconnects everywhere.",
  },
  {
    icon: Sun,
    since: "0.8.0",
    title: "Opens on your day, not a grid",
    body: "Today is the new home screen: what's on, what's next, and your briefing in one place. The sidebar now holds one space at a time, so the vault's notes and graph no longer crowd your day.",
  },
  {
    icon: Sparkles,
    since: "0.8.0",
    title: "A model trained for this",
    body: "Pushin now ships its own on-device model, tuned to read plans far more reliably than the general-purpose one it used to borrow, at the same size and with the same privacy.",
  },
  {
    icon: CalendarPlus,
    since: "0.8.0",
    title: "Subscribe to any calendar",
    body: "Paste an .ics feed for a shared calendar, a team schedule, holidays, and Pushin plans your tasks around it. Read-only: it never edits or sends anything back.",
  },
  {
    icon: MessagesSquare,
    since: "0.8.0",
    title: "Tells you why",
    body: "Every auto-scheduled block can now explain itself: waiting on something else, held for a deadline, or just the earliest free slot.",
  },
  {
    icon: Zap,
    since: "0.8.0",
    title: "Lighter on your machine",
    body: "The AI starts faster and the first reply is quicker. When you step away, the model quietly unloads to free up memory (several GB) and springs back the moment you plan or chat again. Tune or turn it off under Settings ▸ On-device AI.",
  },
];

/**
 * The cards worth showing to someone moving from `from` to `to`.
 *
 * `from` is the last version this install actually ran. `null` means we don't know — a fresh
 * `lastSeenVersion` key, cleared storage, or the forced dev/`?whatsnew=1` preview — in which case
 * everything is shown rather than nothing, so the overlay is never mysteriously empty.
 *
 * The upper bound matters as much as the lower one: a card tagged with an unreleased version sits in
 * the list harmlessly until the release that carries it actually ships.
 */
export function featuresSince(from: string | null | undefined, to?: string | null, list: Feature[] = FEATURES): Feature[] {
  if (!from) return list;
  return list.filter((f) => isNewer(f.since, from) && (!to || !isNewer(f.since, to)));
}

/** Whether an update from `from` to `to` has anything to announce. */
export function hasNewFeatures(from: string | null | undefined, to?: string | null): boolean {
  return featuresSince(from, to).length > 0;
}
const TITLE_DELAY = 80;
const FIRST_CARD = 440;
const STEP = 150;

export default function WhatsNew({
  version,
  from,
  onDone,
}: {
  /** The version now running. */
  version?: string;
  /** The version this install last ran, or null when unknown (fresh key / forced preview). */
  from?: string | null;
  onDone: () => void;
}) {
  // Only what is new to THIS user: someone who skipped three releases sees all three, someone who
  // updates every time sees only the latest. Memoised so the staggered animation delays below stay
  // stable across re-renders.
  const features = useMemo(() => featuresSince(from, version), [from, version]);
  const ctaDelay = FIRST_CARD + features.length * STEP + 120;

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
          {features.map((f, i) => {
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
