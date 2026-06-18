import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./state/store";
import Sidebar from "./components/Sidebar";
import ConflictBanner from "./components/ConflictBanner";
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
import OnboardingModal from "./components/OnboardingModal";
import CommandPalette from "./components/CommandPalette";
import TitleBar from "./components/TitleBar";

export default function App() {
  const loaded = useStore((s) => s.loaded);
  const view = useStore((s) => s.view);
  const calMode = useStore((s) => s.calMode);
  const load = useStore((s) => s.load);
  const onboarded = useStore((s) => s.settings?.onboarded ?? true);

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
        <TitleBar />
        <div className="flex-1 grid place-items-center text-gray-500">Loading Pushin…</div>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      <TitleBar />
      <div className="flex-1 min-h-0 flex">
        <Sidebar />
        <div className="flex-1 min-w-0 flex flex-col">
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
      {!onboarded && <OnboardingModal />}
      <CommandPalette />
      <QuickCapture />
    </div>
  );
}
