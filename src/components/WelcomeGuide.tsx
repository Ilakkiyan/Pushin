import { useState } from "react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { Settings } from "../lib/ipc";
import { AboutYou, CommitmentList, SleepFields } from "./Personalization";

/**
 * The new-user setup, shown after the opening animation when `settings.onboarded` is false. A calm,
 * full-screen, MULTI-STEP wizard — one focused thing per screen, sliding seamlessly between steps —
 * so it never feels overwhelming. `onDone` hands control to the app (called after settings save, which
 * flips `onboarded`). The same fields are editable later in Settings.
 */
const DAYS = [
  { n: 1, l: "Mon" },
  { n: 2, l: "Tue" },
  { n: 3, l: "Wed" },
  { n: 4, l: "Thu" },
  { n: 5, l: "Fri" },
  { n: 6, l: "Sat" },
  { n: 7, l: "Sun" },
];
const bigInput = "w-full bg-white/5 border border-white/10 px-3 py-3 text-base outline-none focus:border-white/25";

export default function WelcomeGuide({ onDone }: { onDone: () => void }) {
  const settings = useStore((s) => s.settings)!;
  const saveSettings = useStore((s) => s.saveSettings);
  const [form, setForm] = useState<Settings>(() => ({
    ...settings,
    sleepEnabled: true,
    sleepStart: settings.sleepStart || "23:00",
    sleepEnd: settings.sleepEnd || "07:00",
  }));
  const [step, setStep] = useState(0);
  const [dir, setDir] = useState(1); // slide direction: +1 forward, -1 back
  const [busy, setBusy] = useState(false);

  const update = (patch: Partial<Settings>) => setForm((f) => ({ ...f, ...patch }));
  const toggleDay = (n: number) =>
    update({ workDays: form.workDays.includes(n) ? form.workDays.filter((d) => d !== n) : [...form.workDays, n].sort() });

  const finish = async (next: Settings) => {
    setBusy(true);
    try {
      await saveSettings(next);
      onDone();
    } finally {
      setBusy(false);
    }
  };
  const save = () => finish({ ...form, onboarded: true });
  const skip = () => finish({ ...settings, onboarded: true, sleepEnabled: false });

  const steps = [
    {
      title: "A bit about you",
      subtitle: "Helps your on-device AI understand you from the start. Pick what fits — and add anything else.",
      body: <AboutYou large archetypes={form.archetypes ?? []} aboutMe={form.aboutMe ?? ""} onChange={update} />,
    },
    {
      title: "When do you usually work?",
      subtitle: "Pushin schedules your tasks inside these hours, on the days you pick.",
      body: (
        <div className="space-y-7">
          <div className="grid max-w-md grid-cols-2 gap-4">
            <label className="block space-y-1.5">
              <span className="text-sm text-gray-500">Start</span>
              <input type="time" value={form.workStart} onChange={(e) => update({ workStart: e.target.value })} className={bigInput} />
            </label>
            <label className="block space-y-1.5">
              <span className="text-sm text-gray-500">End</span>
              <input type="time" value={form.workEnd} onChange={(e) => update({ workEnd: e.target.value })} className={bigInput} />
            </label>
          </div>
          <div className="flex flex-wrap gap-2">
            {DAYS.map((d) => (
              <button
                key={d.n}
                type="button"
                onClick={() => toggleDay(d.n)}
                className={clsx(
                  "h-12 w-16 border text-sm transition",
                  form.workDays.includes(d.n) ? "border-white/25 bg-white/15 text-gray-100" : "border-white/10 bg-white/5 text-gray-500 hover:bg-white/10",
                )}
              >
                {d.l}
              </button>
            ))}
          </div>
        </div>
      ),
    },
    {
      title: "Sleep schedule",
      subtitle: "Pushin keeps this window free, so nothing lands while you're resting.",
      body: (
        <div className="text-base">
          <SleepFields enabled={form.sleepEnabled} start={form.sleepStart} end={form.sleepEnd} onChange={update} />
        </div>
      ),
    },
    {
      title: "Routines & blocked time",
      subtitle: "Recurring time to protect — lunch, the gym, the commute. Pushin won't book work here.",
      body: <CommitmentList items={form.commitments} onChange={(commitments) => update({ commitments })} />,
    },
  ];

  const isLast = step === steps.length - 1;
  const current = steps[step];
  const go = (target: number) => {
    setDir(target > step ? 1 : -1);
    setStep(target);
  };

  return (
    <div className="fixed inset-0 z-50 flex flex-col bg-[var(--bg)] welcome-in">
      {/* header: wordmark · progress · skip */}
      <div className="flex shrink-0 items-center justify-between px-8 pt-8">
        <div className="wordmark text-sm text-gray-500" style={{ letterSpacing: "0.3em" }}>
          Pushin
        </div>
        <div className="flex items-center gap-1.5">
          {steps.map((_, i) => (
            <div
              key={i}
              className={clsx("h-1 transition-all duration-300", i === step ? "w-8 bg-white/80" : i < step ? "w-4 bg-white/35" : "w-4 bg-white/12")}
            />
          ))}
        </div>
        <button onClick={skip} disabled={busy} className="text-xs text-gray-600 transition hover:text-gray-400 disabled:opacity-50">
          Skip
        </button>
      </div>

      {/* one fat, centered step — slides in on change */}
      <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto px-8">
        <div key={step} className={clsx("w-full max-w-2xl py-12", dir > 0 ? "step-in-right" : "step-in-left")}>
          <div className="text-xs uppercase tracking-widest text-gray-600">Step {step + 1} of {steps.length}</div>
          <h1 className="mt-3 text-4xl font-light tracking-tight text-gray-100">{current.title}</h1>
          {current.subtitle && <p className="mt-3 text-base leading-relaxed text-gray-500">{current.subtitle}</p>}
          <div className="mt-10">{current.body}</div>
        </div>
      </div>

      {/* nav */}
      <div className="shrink-0 px-8 pb-10">
        <div className="mx-auto flex w-full max-w-2xl items-center justify-between">
          <button
            onClick={() => go(step - 1)}
            className={clsx("px-4 py-2.5 text-sm transition", step === 0 ? "pointer-events-none opacity-0" : "text-gray-400 hover:text-gray-200")}
          >
            ← Back
          </button>
          <button
            onClick={() => (isLast ? save() : go(step + 1))}
            disabled={busy}
            className="bg-white/90 px-7 py-2.5 text-sm font-medium text-gray-900 transition hover:bg-white disabled:opacity-50"
          >
            {isLast ? (busy ? "Setting up…" : "Get started") : "Continue →"}
          </button>
        </div>
      </div>
    </div>
  );
}
