import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./state/store";
import Sidebar from "./components/Sidebar";
import ConflictBanner from "./components/ConflictBanner";
import UpdateBanner from "./components/UpdateBanner";
import OpeningAnimation from "./components/OpeningAnimation";
import WelcomeBack from "./components/WelcomeBack";
import WhatsNew from "./components/WhatsNew";
import TodayPane from "./panes/TodayPane";
import CalendarPane from "./panes/CalendarPane";
import MonthPane from "./panes/MonthPane";
import ProjectsPane from "./panes/ProjectsPane";
import HabitsPane from "./panes/HabitsPane";
import VaultPane from "./panes/VaultPane";
import GraphPane from "./panes/GraphPane";
import InboxPane from "./panes/InboxPane";
import LabelPane from "./panes/LabelPane";
import QuickCapture from "./components/QuickCapture";
import ChatPane from "./panes/ChatPane";
import TaskListPane from "./panes/TaskListPane";
import PeoplePane from "./panes/PeoplePane";
import BookingPane from "./panes/BookingPane";
import SettingsPane from "./panes/SettingsPane";
import WelcomeGuide from "./components/WelcomeGuide";
import CommandPalette from "./components/CommandPalette";
import TitleBar from "./components/TitleBar";
import MobileShell from "./components/MobileShell";
import { useIsMobile } from "./lib/useIsMobile";
import { useHotkeys } from "./lib/useHotkeys";
import { applyVaultChange } from "./lib/vaultImport";
import { api, type VaultChange } from "./lib/ipc";
import { getVersion } from "@tauri-apps/api/app";

export default function App() {
  const loaded = useStore((s) => s.loaded);
  const view = useStore((s) => s.view);
  const calMode = useStore((s) => s.calMode);
  const chatMode = useStore((s) => s.chatMode);
  const load = useStore((s) => s.load);
  // TESTING ONLY (this machine): replay the full new-user opening flow — splash → guided intro — on EVERY
  // launch. Uses a session state (NOT an `onboarded` override) so completing/skipping the guide still
  // dismisses it. Set to `false` (or delete this + the `replayGuide` usage below) to restore normal behavior.
  const FORCE_OPENING_FLOW = false;
  const onboarded = useStore((s) => s.settings?.onboarded ?? true);
  const [replayGuide, setReplayGuide] = useState(FORCE_OPENING_FLOW);
  const isMobile = useIsMobile();
  const [splashDone, setSplashDone] = useState(false);
  // AI boot gate: the opening splash doubles as the loading screen — it holds (showing a spinner) until
  // the on-device model is loaded into memory, so the app never flashes before the AI is ready. Resolves
  // immediately when no model is downloaded yet (first run → the setup card) and in tests. Model checks
  // run while the splash is up (the effect below).
  const [aiBootDone, setAiBootDone] = useState(import.meta.env.MODE === "test");
  const llmReachable = useStore((s) => s.llm?.reachable ?? false);
  const activeModelId = useStore((s) => s.settings?.modelId);
  const splash = splashDone ? null : <OpeningAnimation ready={aiBootDone} onDone={() => setSplashDone(true)} />;
  // The returning-user "welcome back" landing shows after the splash until the user enters the app.
  // Skipped in unit tests; `?enter=1` skips it for inner-app screenshots.
  const [entered, setEntered] = useState(() => {
    if (import.meta.env.MODE === "test") return true;
    return typeof window !== "undefined" && new URLSearchParams(window.location.search).get("enter") === "1";
  });
  // The post-update "what's new" intro (shown once after the app version changes — see the effect below).
  // The FORCED case (dev builds + `?whatsnew=1`) is seeded synchronously here, not in the effect below:
  // effects run AFTER the first paint, so setting it there let one calendar frame slip through before the
  // overlay mounted (the "flash before What's New"). Seeding the initial state closes that gap. Never in
  // tests. Production's version-changed case is still set in the effect and covered by `bootCover`.
  const [whatsNew, setWhatsNew] = useState(
    () =>
      import.meta.env.MODE !== "test" &&
      (import.meta.env.DEV ||
        (typeof window !== "undefined" && new URLSearchParams(window.location.search).get("whatsnew") === "1")),
  );
  const [appVersion, setAppVersion] = useState<string | undefined>(undefined);
  // Until the version check resolves we don't yet know whether to show "what's new"; cover the gap so
  // the app never flashes the calendar before the intro. (`true` in tests so they render the app.)
  const [versionChecked, setVersionChecked] = useState(import.meta.env.MODE === "test");
  // New users get the guided intro; returning users get the welcome-back landing. Both sit over the
  // (already-mounted) shell and clear once the user is in. The guide flips `onboarded` on save.
  const guide =
    !onboarded || replayGuide ? (
      <WelcomeGuide onDone={() => { setReplayGuide(false); setEntered(true); }} />
    ) : null;
  const welcome =
    !guide && onboarded && !entered && !whatsNew ? (
      <WelcomeBack
        onEnter={(t) => {
          if (t) {
            useStore.getState().setView("calendar");
            useStore.getState().setPendingChat(t);
          }
          setEntered(true);
        }}
      />
    ) : null;
  // After an update + restart an existing user gets the "what's new" intro instead of the welcome-back
  // landing; dismissing it drops them straight into the app.
  // NOTE: intentionally NOT gated on `splashDone`. The splash (z-100) fades out over ~460ms BEFORE it
  // calls onDone/sets splashDone, so gating What's New (z-60) on splashDone left it unmounted during the
  // fade — the splash faded to reveal the calendar (z-10), then What's New popped in (the "flash"). By
  // mounting it now, it sits opaque BEHIND the still-on-top splash, so the fade reveals it, not the
  // calendar. `guide` still holds it back so it can't paint over the new-user opening flow.
  const whatsNewEl =
    !guide && whatsNew ? (
      <WhatsNew version={appVersion} onDone={() => { setWhatsNew(false); setEntered(true); }} />
    ) : null;
  // Cover the brief window between the splash clearing and the version check resolving, so the app
  // never flashes behind the (about-to-appear) "what's new" intro.
  const bootCover = splashDone && !versionChecked ? <div className="fixed inset-0 z-[55] bg-[var(--bg)]" /> : null;

  useHotkeys(); // global "g then key" navigation

  useEffect(() => {
    load();
  }, [load]);

  // Day-rollover sweep. Everything a missed task needs happens inside `reschedule` (the Rust
  // `sweep_missed` drops blocks whose day is over and re-plans that work into the next free slot) —
  // but nothing calls `reschedule` when time merely *passes*. So: once when the app opens (it may
  // have been shut for days), then whenever the local date turns over on a long-running window.
  // `toDateString()` is the comparison rather than a timer arithmetic, so waking from sleep, a
  // timezone change, and DST all resolve to "is it a different day than last tick" correctly.
  const lastDayRef = useRef<string | null>(null);
  useEffect(() => {
    if (!loaded) return;
    const sweep = () => {
      const today = new Date().toDateString();
      if (lastDayRef.current === today) return;
      lastDayRef.current = today;
      useStore.getState().reschedule().catch(() => {});
    };
    sweep(); // on open
    const id = setInterval(sweep, 60_000);
    return () => clearInterval(id);
  }, [loaded]);

  // Resolve the AI boot gate (see `aiLoading`): ready once the server is reachable (model in memory);
  // skip when no model is downloaded; safety timeout so a stuck/slow load never traps the loading screen.
  useEffect(() => {
    if (aiBootDone) return;
    if (llmReachable) {
      setAiBootDone(true);
      return;
    }
    let cancelled = false;
    api
      .modelPresent(activeModelId ?? "")
      .then((present) => {
        if (!cancelled && !present) setAiBootDone(true);
      })
      .catch(() => {
        if (!cancelled) setAiBootDone(true);
      });
    const t = setTimeout(() => {
      if (!cancelled) setAiBootDone(true);
    }, 35000);
    return () => {
      cancelled = true;
      clearTimeout(t);
    };
  }, [aiBootDone, llmReachable, activeModelId]);

  // Idle-unload: after `idleUnloadMinutes` of no AI use (0 = never), free the chat model's RAM/VRAM.
  // It respawns transparently on the next AI action (store.wakeAi), so this is invisible apart from a
  // one-time reload. We deliberately DON'T flip `llm.reachable` — a sleeping model is still "set up",
  // and flipping it would wrongly surface the AI-setup card. Checked once a minute; `sleptRef` stops
  // us from re-issuing the (no-op) unload every tick, and resets the moment activity is recent again.
  const sleptRef = useRef(false);
  useEffect(() => {
    const id = setInterval(() => {
      const s = useStore.getState();
      const mins = s.settings?.idleUnloadMinutes ?? 0;
      if (mins <= 0) return;
      const idle = Date.now() - s.lastAiActivity >= mins * 60_000;
      if (!idle) {
        sleptRef.current = false; // activity is recent again → re-arm
        return;
      }
      if (s.busy || sleptRef.current) return; // never mid-request; unload at most once per idle stretch
      api
        .sleepInference()
        .then((killed) => {
          if (killed) sleptRef.current = true;
        })
        .catch(() => {});
    }, 60_000);
    return () => clearInterval(id);
  }, []);

  // Show the "what's new" intro once, on the first launch after the app version changes (i.e. an
  // update was installed + the app restarted). New users (not onboarded) and unit tests are skipped;
  // localStorage remembers the last version seen so it shows exactly once per release.
  useEffect(() => {
    if (!loaded || import.meta.env.MODE === "test") return;
    // Dev builds (and the ?whatsnew=1 hook) replay the full opening sequence — splash → loading →
    // intro — on EVERY launch, so it can be iterated on. Production shows it once per version (below).
    const forced =
      import.meta.env.DEV || (typeof window !== "undefined" && new URLSearchParams(window.location.search).get("whatsnew") === "1");
    if (forced) setWhatsNew(true);
    getVersion()
      .then((v) => {
        setAppVersion(v);
        const key = "pushin:lastSeenVersion";
        const last = localStorage.getItem(key);
        localStorage.setItem(key, v);
        if (!forced && (useStore.getState().settings?.onboarded ?? false) && last !== v) setWhatsNew(true);
      })
      .catch(() => {})
      .finally(() => setVersionChecked(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded]);

  // When the sync engine applies remote changes from another device, refresh the app data.
  useEffect(() => {
    const un = listen("sync-applied", () => load());
    return () => {
      un.then((f) => f());
    };
  }, [load]);

  // Two-way vault: when an external editor changes a `.md` file, fold it into the DB and refresh the
  // page tree. Best-effort — a malformed file is skipped, never crashes the app.
  useEffect(() => {
    const un = listen<VaultChange>("vault-changed", async (e) => {
      try {
        await applyVaultChange(e.payload);
        await useStore.getState().loadPages();
      } catch {
        /* skip a change that won't apply */
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  if (!loaded) {
    return (
      <div className="h-full flex flex-col">
        {splash}
        {!isMobile && <TitleBar />}
        <div className="flex-1 grid place-items-center text-gray-500" />
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      {splash}
      {bootCover}
      {guide}
      {welcome}
      {whatsNewEl}
      {!isMobile && <TitleBar />}
      {isMobile ? (
        <div className="flex-1 min-h-0 flex flex-col">
          <ConflictBanner />
          <div className="flex-1 min-h-0">
            <MobileShell />
          </div>
        </div>
      ) : (
        <div className="flex-1 min-h-0 flex">
          <Sidebar />
          <div className="flex-1 min-w-0 flex flex-col">
            <UpdateBanner />
            <ConflictBanner />
            <main className="flex-1 min-h-0 flex">
              {view === "today" && <TodayPane />}
              {view === "calendar" && (
                <>
                  <div className="flex-1 min-w-0">{calMode === "month" ? <MonthPane /> : <CalendarPane />}</div>
                  {/* Chat mode → a wider, focused conversation: the tasks panel steps aside. */}
                  <aside
                    className={`shrink-0 border-l border-white/10 flex flex-col min-h-0 transition-[width] duration-300 ease-out ${
                      chatMode === "chat" ? "w-[480px]" : "w-[400px]"
                    }`}
                  >
                    <div className="flex-1 min-h-0 overflow-hidden">
                      <ChatPane />
                    </div>
                    {chatMode !== "chat" && (
                      <div className="h-[46%] shrink-0 border-t border-white/10 overflow-hidden">
                        <TaskListPane />
                      </div>
                    )}
                  </aside>
                </>
              )}
              {view === "projects" && <ProjectsPane />}
              {view === "habits" && <HabitsPane />}
              {view === "vault" && <VaultPane />}
              {view === "graph" && <GraphPane />}
              {view === "inbox" && <InboxPane />}
              {view === "label" && <LabelPane />}
              {view === "people" && <PeoplePane />}
              {view === "booking" && import.meta.env.DEV && <BookingPane />}
              {view === "settings" && <SettingsPane />}
            </main>
          </div>
        </div>
      )}
      <CommandPalette />
      <QuickCapture />
    </div>
  );
}
