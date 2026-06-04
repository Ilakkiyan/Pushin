import { CalendarDays, CalendarClock, CalendarRange, Flame, Settings as SettingsIcon, Loader2 } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";
import { useState } from "react";

function NavButton({ active, onClick, icon, label }: { active: boolean; onClick: () => void; icon: React.ReactNode; label: string }) {
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex items-center gap-2 px-3 py-1.5 rounded-lg text-sm transition",
        active ? "bg-white/10 text-white" : "text-gray-400 hover:text-white hover:bg-white/5",
      )}
    >
      {icon}
      {label}
    </button>
  );
}

export default function TopBar() {
  const view = useStore((s) => s.view);
  const setView = useStore((s) => s.setView);
  const llm = useStore((s) => s.llm);
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

  return (
    <header className="h-14 shrink-0 border-b border-white/10 flex items-center justify-between px-4 bg-[#0e1117]">
      <div className="flex items-center gap-2">
        <div className="size-7 rounded-lg bg-gradient-to-br from-indigo-500 to-fuchsia-500 grid place-items-center text-sm leading-none">
          📌
        </div>
        <span className="font-semibold tracking-tight">Pushin</span>
        <span className="text-xs text-gray-500 ml-1">local-AI calendar</span>
      </div>

      <nav className="flex items-center gap-1">
        <NavButton active={view === "calendar"} onClick={() => setView("calendar")} icon={<CalendarDays className="size-4" />} label="Week" />
        <NavButton active={view === "month"} onClick={() => setView("month")} icon={<CalendarRange className="size-4" />} label="Month" />
        <NavButton active={view === "habits"} onClick={() => setView("habits")} icon={<Flame className="size-4" />} label="Habits" />
        <NavButton active={view === "booking"} onClick={() => setView("booking")} icon={<CalendarClock className="size-4" />} label="Booking" />
        <NavButton active={view === "settings"} onClick={() => setView("settings")} icon={<SettingsIcon className="size-4" />} label="Settings" />
      </nav>

      <div className="flex items-center gap-3">
        {busy && <Loader2 className="size-4 animate-spin text-gray-400" />}
        <button
          onClick={connect}
          title="Click to connect / start a local inference server"
          className={clsx(
            "flex items-center gap-2 px-2.5 py-1 rounded-full text-xs border transition",
            llm?.reachable
              ? "border-emerald-500/40 bg-emerald-500/10 text-emerald-300"
              : "border-amber-500/40 bg-amber-500/10 text-amber-300 hover:bg-amber-500/20",
          )}
        >
          <span className={clsx("size-1.5 rounded-full", llm?.reachable ? "bg-emerald-400" : "bg-amber-400")} />
          {connecting ? "Connecting…" : llm?.reachable ? "AI ready" : "AI offline"}
        </button>
      </div>
    </header>
  );
}
