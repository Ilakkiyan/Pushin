import { useEffect } from "react";
import { useStore } from "./state/store";
import TopBar from "./components/TopBar";
import ConflictBanner from "./components/ConflictBanner";
import CalendarPane from "./panes/CalendarPane";
import MonthPane from "./panes/MonthPane";
import HabitsPane from "./panes/HabitsPane";
import ChatPane from "./panes/ChatPane";
import TaskListPane from "./panes/TaskListPane";
import BookingPane from "./panes/BookingPane";
import SettingsPane from "./panes/SettingsPane";

export default function App() {
  const loaded = useStore((s) => s.loaded);
  const view = useStore((s) => s.view);
  const load = useStore((s) => s.load);

  useEffect(() => {
    load();
  }, [load]);

  if (!loaded) {
    return <div className="h-full grid place-items-center text-gray-500">Loading Pushin…</div>;
  }

  return (
    <div className="h-full flex flex-col">
      <TopBar />
      <ConflictBanner />
      <main className="flex-1 min-h-0 flex">
        {(view === "calendar" || view === "month") && (
          <>
            <div className="flex-1 min-w-0">{view === "month" ? <MonthPane /> : <CalendarPane />}</div>
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
        {view === "habits" && <HabitsPane />}
        {view === "booking" && <BookingPane />}
        {view === "settings" && <SettingsPane />}
      </main>
    </div>
  );
}
