import {
  CalendarDays,
  FolderKanban,
  Flame,
  CalendarClock,
  Settings as SettingsIcon,
  Notebook,
  Network,
  CalendarHeart,
  Users,
  Inbox,
  Loader2,
  PanelLeftClose,
  PanelLeftOpen,
  Sun,
  Library,
  ArrowLeft,
  ChevronRight,
} from "lucide-react";
import clsx from "clsx";
import { useState } from "react";
import SyncBar from "./SyncBar";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { toLocalDate } from "../lib/time";
import VaultTree from "./VaultTree";

type View = ReturnType<typeof useStore.getState>["view"];

function NavItem({
  active,
  collapsed,
  onClick,
  icon,
  label,
  badge,
}: {
  active: boolean;
  collapsed: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  badge?: number;
}) {
  return (
    <button
      onClick={onClick}
      title={collapsed ? label : undefined}
      className={clsx(
        "w-full flex items-center gap-2.5 text-sm",
        // Roomier target — a nav row is a high-frequency destination, so it earns an easy Fitts hit.
        collapsed ? "justify-center px-0 py-2.5" : "px-3 py-2",
        active ? "is-selected text-white font-medium" : "hoverable text-[var(--ink-muted)] hover:text-white",
      )}
    >
      <span className="shrink-0">{icon}</span>
      {!collapsed && <span className="truncate flex-1 text-left">{label}</span>}
      {!collapsed && !!badge && (
        <span className="tnum shrink-0 text-[10px] px-1.5 py-0.5 bg-white/10 text-[var(--ink-muted)]">{badge}</span>
      )}
    </button>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return <div className="px-3 pt-4 pb-1 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--ink-faint)]">{children}</div>;
}

/** Labels list in the sidebar — click a label to open its cross-cutting filtered view. */
function LabelsSection() {
  const labels = useStore((s) => s.labels);
  const currentLabelId = useStore((s) => s.currentLabelId);
  const view = useStore((s) => s.view);
  const openLabel = useStore((s) => s.openLabel);
  if (labels.length === 0) return null;
  return (
    <div className="mt-1">
      <SectionLabel>Labels</SectionLabel>
      {labels.map((l) => (
        <button
          key={l.id}
          onClick={() => openLabel(l.id)}
          className={clsx(
            "w-full flex items-center gap-2 px-3 py-1.5 text-sm",
            view === "label" && currentLabelId === l.id ? "is-selected text-white" : "hoverable text-[var(--ink-muted)] hover:text-white",
          )}
        >
          <span className="size-2 rounded-full shrink-0" style={{ background: l.color }} />
          <span className="truncate flex-1 text-left">{l.name}</span>
          {l.count > 0 && <span className="tnum text-[10px] text-[var(--ink-faint)]">{l.count}</span>}
        </button>
      ))}
    </div>
  );
}

export default function Sidebar() {
  const view = useStore((s) => s.view);
  const space = useStore((s) => s.space);
  const setView = useStore((s) => s.setView);
  const exitVault = useStore((s) => s.exitVault);
  const openDaily = useStore((s) => s.openDaily);
  const inboxCount = useStore((s) => s.inbox.length);
  const collapsed = useStore((s) => s.sidebarCollapsed);
  const setCollapsed = useStore((s) => s.setSidebarCollapsed);
  const llm = useStore((s) => s.llm);
  const embedReady = useStore((s) => s.embedReady);
  const busy = useStore((s) => s.busy);
  const refreshLlm = useStore((s) => s.refreshLlm);
  const [connecting, setConnecting] = useState(false);

  const connect = async () => {
    setConnecting(true);
    try {
      await api.ensureInference();
    } catch {
      /* surfaced via status pill */
    } finally {
      await refreshLlm();
      setConnecting(false);
    }
  };

  const go = (v: View) => () => setView(v);

  return (
    <aside
      className={clsx(
        "shrink-0 h-full flex flex-col bg-[var(--surface)] border-r border-white/10 transition-[width] duration-150",
        collapsed ? "w-[60px]" : "w-[232px]",
      )}
    >
      {/* Brand + collapse toggle */}
      <div className={clsx("h-14 shrink-0 flex items-center border-b border-white/10", collapsed ? "justify-center px-0" : "justify-between px-3")}>
        {!collapsed && (
          <div className="wordmark truncate text-sm text-gray-200" style={{ letterSpacing: "0.22em" }}>
            {space === "vault" ? "Vault" : "Pushin"}
          </div>
        )}
        <button
          onClick={() => setCollapsed(!collapsed)}
          title={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          className="p-1.5 hoverable text-[var(--ink-faint)] hover:text-white"
        >
          {collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
        </button>
      </div>

      {/* Nav — two spaces, one column. The PLANNER (run-your-day) keeps the primary nav short; the VAULT
          (second brain) is a space you step into, so notes/journal/graph/labels don't crowd the day. */}
      <nav className="flex-1 min-h-0 overflow-y-auto px-2 py-2 space-y-0.5">
        {space === "planner" ? (
          <>
            <NavItem active={view === "today"} collapsed={collapsed} onClick={go("today")} icon={<Sun className="size-4" />} label="Today" />
            <NavItem active={view === "calendar"} collapsed={collapsed} onClick={go("calendar")} icon={<CalendarDays className="size-4" />} label="Calendar" />
            <NavItem active={view === "projects"} collapsed={collapsed} onClick={go("projects")} icon={<FolderKanban className="size-4" />} label="Projects" />
            <NavItem active={view === "habits"} collapsed={collapsed} onClick={go("habits")} icon={<Flame className="size-4" />} label="Habits" />
            {/* Booking hidden from public release builds for now (still available in `npm run tauri dev`). */}
            {import.meta.env.DEV && (
              <NavItem active={view === "booking"} collapsed={collapsed} onClick={go("booking")} icon={<CalendarClock className="size-4" />} label="Booking" />
            )}
            <NavItem active={view === "people"} collapsed={collapsed} onClick={go("people")} icon={<Users className="size-4" />} label="People" />

            <div className="my-2 mx-1 border-t border-white/10" />
            {/* Enter the vault space — the trailing chevron signals it opens a place, not a page. */}
            <button
              onClick={() => setView("vault")}
              title={collapsed ? "Vault" : undefined}
              className={clsx(
                "w-full flex items-center gap-2.5 text-sm hoverable text-[var(--ink-muted)] hover:text-white",
                collapsed ? "justify-center px-0 py-2.5" : "px-3 py-2",
              )}
            >
              <Library className="size-4 shrink-0" />
              {!collapsed && (
                <>
                  <span className="truncate flex-1 text-left">Vault</span>
                  {inboxCount > 0 && <span className="tnum text-[10px] px-1.5 py-0.5 bg-white/10 text-[var(--ink-muted)]">{inboxCount}</span>}
                  <ChevronRight className="size-3.5 shrink-0 text-[var(--ink-faint)]" />
                </>
              )}
            </button>
          </>
        ) : (
          <>
            {/* Back to the planner (returns to whichever planner view you left). */}
            <button
              onClick={exitVault}
              title={collapsed ? "Back to app" : undefined}
              className={clsx(
                "w-full flex items-center gap-2.5 text-sm hoverable text-[var(--ink-muted)] hover:text-white",
                collapsed ? "justify-center px-0 py-2.5" : "px-3 py-2",
              )}
            >
              <ArrowLeft className="size-4 shrink-0" />
              {!collapsed && <span className="truncate flex-1 text-left">Back to app</span>}
            </button>

            {!collapsed && <SectionLabel>Vault</SectionLabel>}
            <NavItem active={view === "vault"} collapsed={collapsed} onClick={go("vault")} icon={<Notebook className="size-4" />} label="Notes" />
            <NavItem
              active={false}
              collapsed={collapsed}
              onClick={() => openDaily(toLocalDate(new Date()))}
              icon={<CalendarHeart className="size-4" />}
              label="Today's note"
            />
            <NavItem active={view === "inbox"} collapsed={collapsed} onClick={go("inbox")} icon={<Inbox className="size-4" />} label="Inbox" badge={inboxCount} />
            <NavItem active={view === "graph"} collapsed={collapsed} onClick={go("graph")} icon={<Network className="size-4" />} label="Graph" />
            {!collapsed && <VaultTree />}
            {!collapsed && <LabelsSection />}
          </>
        )}
      </nav>

      {/* Bottom: sync progress + AI status + settings */}
      <div className="shrink-0 border-t border-white/10 p-2 space-y-1">
        {/* Renders itself away when nothing is syncing, so the footer keeps its usual height. */}
        <SyncBar collapsed={collapsed} />
        <button
          onClick={connect}
          title="Click to connect / start the local inference server"
          className={clsx(
            "w-full flex items-center rounded-lg text-xs border transition",
            collapsed ? "justify-center px-0 py-2" : "gap-2 px-3 py-1.5",
            llm?.reachable
              ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-300"
              : "border-amber-500/40 bg-amber-500/10 text-amber-300 hover:bg-amber-500/20",
          )}
        >
          {busy ? (
            <Loader2 className="size-3.5 animate-spin shrink-0" />
          ) : (
            <span className={clsx("size-1.5 rounded-full shrink-0", llm?.reachable ? "bg-emerald-400" : "bg-amber-400")} />
          )}
          {!collapsed && (
            <span className="truncate">
              {connecting ? "Connecting…" : llm?.reachable ? (embedReady ? "AI ready · Memory ✓" : "AI ready · Memory…") : "AI offline"}
            </span>
          )}
        </button>
        <NavItem active={view === "settings"} collapsed={collapsed} onClick={go("settings")} icon={<SettingsIcon className="size-4" />} label="Settings" />
      </div>
    </aside>
  );
}
