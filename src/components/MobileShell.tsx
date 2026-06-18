import { useState } from "react";
import {
  CalendarDays,
  Sparkles,
  ListTodo,
  Notebook,
  Menu,
  FolderKanban,
  Flame,
  CalendarClock,
  Users,
  Inbox as InboxIcon,
  Network,
  Settings as SettingsIcon,
  X,
  Plus,
} from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import CalendarPane from "../panes/CalendarPane";
import MonthPane from "../panes/MonthPane";
import ChatPane from "../panes/ChatPane";
import TaskListPane from "../panes/TaskListPane";
import VaultPane from "../panes/VaultPane";
import ProjectsPane from "../panes/ProjectsPane";
import HabitsPane from "../panes/HabitsPane";
import BookingPane from "../panes/BookingPane";
import PeoplePane from "../panes/PeoplePane";
import InboxPane from "../panes/InboxPane";
import GraphPane from "../panes/GraphPane";
import LabelPane from "../panes/LabelPane";
import SettingsPane from "../panes/SettingsPane";

/** Primary destinations on the bottom tab bar. "more" opens a sheet for everything else. */
type Tab = "calendar" | "chat" | "tasks" | "vault" | "more";

const TABS: { id: Tab; label: string; icon: React.ReactNode }[] = [
  { id: "calendar", label: "Calendar", icon: <CalendarDays className="size-5" /> },
  { id: "chat", label: "Plan", icon: <Sparkles className="size-5" /> },
  { id: "tasks", label: "Tasks", icon: <ListTodo className="size-5" /> },
  { id: "vault", label: "Notes", icon: <Notebook className="size-5" /> },
  { id: "more", label: "More", icon: <Menu className="size-5" /> },
];

/** Secondary destinations shown in the "More" sheet — each maps to a store `view`. */
const MORE_ITEMS = [
  { v: "projects", label: "Projects", icon: <FolderKanban className="size-5" /> },
  { v: "habits", label: "Habits", icon: <Flame className="size-5" /> },
  { v: "booking", label: "Booking", icon: <CalendarClock className="size-5" /> },
  { v: "people", label: "People", icon: <Users className="size-5" /> },
  { v: "inbox", label: "Inbox", icon: <InboxIcon className="size-5" /> },
  { v: "graph", label: "Graph", icon: <Network className="size-5" /> },
  { v: "settings", label: "Settings", icon: <SettingsIcon className="size-5" /> },
] as const;

/** Renders whichever secondary pane the store `view` currently points at (chosen from the sheet). */
function MoreView() {
  const view = useStore((s) => s.view);
  switch (view) {
    case "projects": return <ProjectsPane />;
    case "habits": return <HabitsPane />;
    case "booking": return <BookingPane />;
    case "people": return <PeoplePane />;
    case "inbox": return <InboxPane />;
    case "graph": return <GraphPane />;
    case "label": return <LabelPane />;
    case "settings": return <SettingsPane />;
    default: return <SettingsPane />;
  }
}

/**
 * Phone layout: one full-screen pane at a time with a bottom tab bar (Calendar / Plan / Tasks /
 * Notes / More). The desktop side-by-side calendar+chat+tasks split is broken out into separate
 * tabs; the persistent sidebar becomes the "More" sheet. Used by `App` when the viewport is narrow.
 */
export default function MobileShell() {
  const [tab, setTab] = useState<Tab>("calendar");
  const [moreOpen, setMoreOpen] = useState(false);
  const calMode = useStore((s) => s.calMode);
  const setView = useStore((s) => s.setView);
  const setCaptureOpen = useStore((s) => s.setCaptureOpen);
  const inboxCount = useStore((s) => s.inbox.length);

  let content: React.ReactNode;
  if (tab === "calendar") {
    // Phone calendar = a single full-width day column (swipe via the ‹ Today › nav). Month view is
    // its own grid. (Desktop uses the 7-day week grid — same component, `days={7}` default.)
    content = calMode === "month" ? <MonthPane /> : <CalendarPane days={1} />;
  } else if (tab === "chat") {
    content = <ChatPane />;
  } else if (tab === "tasks") {
    content = <TaskListPane />;
  } else if (tab === "vault") {
    content = <VaultPane />;
  } else {
    content = <MoreView />;
  }

  return (
    <div className="h-full flex flex-col min-h-0">
      <main className="flex-1 min-h-0 overflow-hidden">{content}</main>

      {/* Quick-capture FAB — the desktop's Cmd/Ctrl+Shift+N has no touch equivalent. Opens the same
          QuickCapture modal (rendered globally in App). Sits above the tab bar + below the More sheet. */}
      <button
        onClick={() => setCaptureOpen(true)}
        aria-label="Quick capture"
        className="fixed right-4 z-30 size-14 rounded-full bg-indigo-500 active:bg-indigo-400 text-white grid place-items-center shadow-lg shadow-indigo-900/40"
        style={{ bottom: "calc(env(safe-area-inset-bottom) + 72px)" }}
      >
        <Plus className="size-6" />
      </button>

      {/* Bottom tab bar (respects Android/iOS bottom safe-area inset). */}
      <nav
        className="shrink-0 flex items-stretch border-t border-white/10 bg-[#0e1117]"
        style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
      >
        {TABS.map((t) => {
          const active = tab === t.id;
          return (
            <button
              key={t.id}
              onClick={() => (t.id === "more" ? setMoreOpen(true) : setTab(t.id))}
              className={clsx(
                "relative flex-1 flex flex-col items-center gap-0.5 py-2 transition",
                active ? "text-indigo-300" : "text-gray-500 active:text-gray-300",
              )}
            >
              {t.icon}
              <span className="text-[10px] leading-none">{t.label}</span>
              {t.id === "more" && inboxCount > 0 && (
                <span className="absolute top-1.5 right-[28%] size-1.5 rounded-full bg-indigo-400" />
              )}
            </button>
          );
        })}
      </nav>

      {/* "More" bottom sheet. */}
      {moreOpen && (
        <div className="fixed inset-0 z-40 flex flex-col justify-end" onClick={() => setMoreOpen(false)}>
          <div className="absolute inset-0 bg-black/50" />
          <div
            className="relative bg-[#0e1117] border-t border-white/10 rounded-t-2xl"
            style={{ paddingBottom: "env(safe-area-inset-bottom)" }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between px-4 py-3">
              <span className="text-sm font-semibold">More</span>
              <button onClick={() => setMoreOpen(false)} className="p-1 text-gray-400 active:text-white">
                <X className="size-5" />
              </button>
            </div>
            <div className="grid grid-cols-3 gap-2 p-3 pt-0">
              {MORE_ITEMS.map((m) => (
                <button
                  key={m.v}
                  onClick={() => {
                    setView(m.v);
                    setTab("more");
                    setMoreOpen(false);
                  }}
                  className="flex flex-col items-center gap-1.5 rounded-xl bg-white/5 py-4 text-xs text-gray-300 active:bg-white/10"
                >
                  {m.icon}
                  <span>{m.label}</span>
                  {m.v === "inbox" && inboxCount > 0 && <span className="text-[10px] text-indigo-300">{inboxCount}</span>}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
