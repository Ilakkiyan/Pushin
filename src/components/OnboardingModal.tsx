// First-run personalization. Shown once (until settings.onboarded is true) so the user can tell
// Pushin about their sleep, working hours, and recurring routines/blocked time. Everything here
// is also editable later in Settings → "Your routine".
import { useState } from "react";
import { CalendarClock, Clock, Moon, Sparkles } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { Settings } from "../lib/ipc";
import { CommitmentList, SleepFields } from "./Personalization";

const DAYS = [
  { n: 1, l: "Mon" },
  { n: 2, l: "Tue" },
  { n: 3, l: "Wed" },
  { n: 4, l: "Thu" },
  { n: 5, l: "Fri" },
  { n: 6, l: "Sat" },
  { n: 7, l: "Sun" },
];

const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50";

export default function OnboardingModal() {
  const settings = useStore((s) => s.settings)!;
  const saveSettings = useStore((s) => s.saveSettings);
  // Present a complete sleep default in the welcome flow even for older settings rows (which have
  // sleepEnabled=false and empty times). "Skip" still turns sleep off, so this imposes nothing.
  const [form, setForm] = useState<Settings>(() => ({
    ...settings,
    sleepEnabled: true,
    sleepStart: settings.sleepStart || "23:00",
    sleepEnd: settings.sleepEnd || "07:00",
  }));
  const [busy, setBusy] = useState(false);

  const update = (patch: Partial<Settings>) => setForm((f) => ({ ...f, ...patch }));
  const toggleDay = (n: number) =>
    update({ workDays: form.workDays.includes(n) ? form.workDays.filter((d) => d !== n) : [...form.workDays, n].sort() });

  const finish = async (next: Settings) => {
    setBusy(true);
    try {
      await saveSettings(next);
    } finally {
      setBusy(false);
    }
  };
  // Save keeps what they configured; Skip marks onboarding done and imposes nothing.
  const save = () => finish({ ...form, onboarded: true });
  const skip = () => finish({ ...settings, onboarded: true, sleepEnabled: false });

  return (
    <div className="fixed inset-0 z-[60] grid place-items-center bg-black/60 p-4">
      <div className="w-full max-w-lg max-h-[88vh] overflow-y-auto rounded-2xl border border-white/10 bg-[#12151c] shadow-2xl">
        <div className="p-5 sm:p-6 space-y-6">
          <div className="space-y-1">
            <div className="flex items-center gap-2 text-fuchsia-300">
              <Sparkles className="size-5" />
              <span className="text-[11px] uppercase tracking-wide">Welcome to Pushin 📌</span>
            </div>
            <h2 className="text-lg font-semibold">Let's personalize your schedule</h2>
            <p className="text-sm text-gray-400">
              Tell Pushin when you're off-limits — sleep, meals, routines — and it'll plan your tasks around them. You can change any of
              this later in <span className="text-gray-300">Settings</span>.
            </p>
          </div>

          {/* Working hours */}
          <section className="space-y-3">
            <h3 className="text-sm font-medium flex items-center gap-2"><Clock className="size-4 text-indigo-300" /> When do you usually work?</h3>
            <div className="grid grid-cols-2 gap-3">
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">Start</span>
                <input type="time" value={form.workStart} onChange={(e) => update({ workStart: e.target.value })} className={inputCls} />
              </label>
              <label className="block space-y-1">
                <span className="text-xs text-gray-400">End</span>
                <input type="time" value={form.workEnd} onChange={(e) => update({ workEnd: e.target.value })} className={inputCls} />
              </label>
            </div>
            <div className="flex flex-wrap gap-1.5">
              {DAYS.map((d) => (
                <button
                  key={d.n}
                  onClick={() => toggleDay(d.n)}
                  className={clsx(
                    "size-9 rounded-md text-xs",
                    form.workDays.includes(d.n) ? "bg-indigo-500/30 text-indigo-100 border border-indigo-400/40" : "bg-white/5 text-gray-500 border border-white/10",
                  )}
                >
                  {d.l}
                </button>
              ))}
            </div>
          </section>

          {/* Sleep */}
          <section className="space-y-3">
            <h3 className="text-sm font-medium flex items-center gap-2"><Moon className="size-4 text-indigo-300" /> Sleep schedule</h3>
            <SleepFields enabled={form.sleepEnabled} start={form.sleepStart} end={form.sleepEnd} onChange={update} />
          </section>

          {/* Routines / blocked time */}
          <section className="space-y-3">
            <h3 className="text-sm font-medium flex items-center gap-2"><CalendarClock className="size-4 text-indigo-300" /> Routines & blocked time</h3>
            <p className="text-xs text-gray-500">Recurring time to protect — lunch, gym, the commute, family time. Pushin won't book work here.</p>
            <CommitmentList items={form.commitments} onChange={(commitments) => update({ commitments })} />
          </section>

          <div className="flex items-center justify-end gap-3 pt-3 border-t border-white/10">
            <button onClick={skip} disabled={busy} className="text-sm px-3 py-2 rounded-lg text-gray-400 hover:text-gray-200 disabled:opacity-50">
              Skip for now
            </button>
            <button onClick={save} disabled={busy} className="text-sm px-4 py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400 disabled:opacity-50">
              {busy ? "Saving…" : "Save & continue"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
