import { CalendarDays, CalendarRange } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";

/** Week ⇆ Month segmented toggle, lives in the calendar toolbar (separate from page nav). */
export default function ViewToggle() {
  const calMode = useStore((s) => s.calMode);
  const setCalMode = useStore((s) => s.setCalMode);

  const Btn = ({ mode, icon, label }: { mode: "week" | "month"; icon: React.ReactNode; label: string }) => (
    <button
      onClick={() => setCalMode(mode)}
      className={clsx(
        "flex items-center gap-1 px-2 py-1 rounded-md text-xs transition",
        calMode === mode ? "bg-white/10 text-white" : "text-gray-400 hover:text-white",
      )}
    >
      {icon}
      {label}
    </button>
  );

  return (
    <div className="flex items-center gap-0.5 rounded-lg bg-white/5 border border-white/10 p-0.5">
      <Btn mode="week" icon={<CalendarDays className="size-3.5" />} label="Week" />
      <Btn mode="month" icon={<CalendarRange className="size-3.5" />} label="Month" />
    </div>
  );
}
