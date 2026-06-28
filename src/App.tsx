import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./state/store";
import Sidebar from "./components/Sidebar";
import ConflictBanner from "./components/ConflictBanner";
import UpdateBanner from "./components/UpdateBanner";
import OpeningAnimation from "./components/OpeningAnimation";
import WelcomeBack from "./components/WelcomeBack";
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

export default function App() {
  const loaded = useStore((s) => s.loaded);
  const view = useStore((s) => s.view);
  const calMode = useStore((s) => s.calMode);
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
  // New users get the guided intro; returning users get the welcome-back landing. Both sit over the
  // (already-mounted) shell and clear once the user is in. The guide flips `onboarded` on save.
  const guide = !onboarded ? <WelcomeGuide onDone={() => setEntered(true)} /> : null;
  const welcome =
    onboarded && !entered ? (
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

  useHotkeys(); // global "g then key" navigation

  useEffect(() => {
    load();
  }, [load]);

  // When the sync engine applies remote changes from another device, refresh the app data.
  useEffect(() => {
    const un = listen("sync-applied", () => load());
    return () => {
      un.then((f) => f());
    };
  }, [load]);

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
      {guide}
      {welcome}
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
                  <aside className="w-[400px] shrink-0 border-l border-white/10 flex flex-col min-h-0">
                    <div className="flex-1 min-h-0 overflow-hidden">
                      <ChatPane />
                    </div>
                    <div className="h-[46%] shrink-0 border-t border-white/10 overflow-hidden">
                      <TaskListPane />
                    </div>
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
              {view === "booking" && <BookingPane />}
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
