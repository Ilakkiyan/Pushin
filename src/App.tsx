import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./state/store";
import Sidebar from "./components/Sidebar";
import ConflictBanner from "./components/ConflictBanner";
import UpdateBanner from "./components/UpdateBanner";
import OpeningAnimation from "./components/OpeningAnimation";
import WelcomeBack from "./components/WelcomeBack";
import WhatsNew from "./components/WhatsNew";
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
import type { VaultChange } from "./lib/ipc";
import { getVersion } from "@tauri-apps/api/app";

export default function App() {
  const loaded = useStore((s) => s.loaded);
  const view = useStore((s) => s.view);
  const calMode = useStore((s) => s.calMode);
  const chatMode = useStore((s) => s.chatMode);
  const load = useStore((s) => s.load);
  const onboarded = useStore((s) => s.settings?.onboarded ?? true);
  const isMobile = useIsMobile();
  const [splashDone, setSplashDone] = useState(false);
  const splash = splashDone ? null : <OpeningAnimation onDone={() => setSplashDone(true)} />;
  // The returning-user "welcome back" landing shows after the splash until the user enters the app.
  // Skipped in unit tests; `?enter=1` skips it for inner-app screenshots.
  const [entered, setEntered] = useState(() => {
    if (import.meta.env.MODE === "test") return true;
    return typeof window !== "undefined" && new URLSearchParams(window.location.search).get("enter") === "1";
  });
  // The post-update "what's new" intro (shown once after the app version changes — see the effect below).
  const [whatsNew, setWhatsNew] = useState(false);
  const [appVersion, setAppVersion] = useState<string | undefined>(undefined);
  // Until the version check resolves we don't yet know whether to show "what's new"; cover the gap so
  // the app never flashes the calendar before the intro. (`true` in tests so they render the app.)
  const [versionChecked, setVersionChecked] = useState(import.meta.env.MODE === "test");
  // New users get the guided intro; returning users get the welcome-back landing. Both sit over the
  // (already-mounted) shell and clear once the user is in. The guide flips `onboarded` on save.
  const guide = !onboarded ? <WelcomeGuide onDone={() => setEntered(true)} /> : null;
  const welcome =
    onboarded && !entered && !whatsNew ? (
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
  const whatsNewEl =
    splashDone && whatsNew ? (
      <WhatsNew version={appVersion} onDone={() => { setWhatsNew(false); setEntered(true); }} />
    ) : null;
  // Cover the brief window between the splash clearing and the version check resolving, so the app
  // never flashes behind the (about-to-appear) "what's new" intro.
  const bootCover = splashDone && !versionChecked ? <div className="fixed inset-0 z-[55] bg-[var(--bg)]" /> : null;

  useHotkeys(); // global "g then key" navigation

  useEffect(() => {
    load();
  }, [load]);

  // Show the "what's new" intro once, on the first launch after the app version changes (i.e. an
  // update was installed + the app restarted). New users (not onboarded) and unit tests are skipped;
  // localStorage remembers the last version seen so it shows exactly once per release.
  useEffect(() => {
    if (!loaded || import.meta.env.MODE === "test") return;
    const forced = typeof window !== "undefined" && new URLSearchParams(window.location.search).get("whatsnew") === "1";
    if (forced) setWhatsNew(true); // dev/preview hook (works without a Tauri getVersion)
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
