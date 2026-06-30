import { useState } from "react";
import clsx from "clsx";
import { useStore } from "../state/store";
import type { Settings } from "../lib/ipc";
import { AboutYou, CommitmentList, SleepFields } from "./Personalization";

/**
 * The new-user intro, shown after the opening animation when `settings.onboarded` is false. A calm,
 * full-screen, on-brand version of the old OnboardingModal: a one-line "what is Pushin" + the same
 * working-hours / sleep / routine setup, so the scheduler can plan around the user's life. `onDone`
 * hands control to the app (it's called after settings save, which flips `onboarded`).
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
const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-white/25";

export default function WelcomeGuide({ onDone }: { onDone: () => void }) {
  const settings = useStore((s) => s.settings)!;
  const saveSettings = useStore((s) => s.saveSettings);
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
      onDone();
    } finally {
      setBusy(false);
    }
  };
  const save = () => finish({ ...form, onboarded: true });
  const skip = () => finish({ ...settings, onboarded: true, sleepEnabled: false });

  return (
    <div className="fixed inset-0 z-50 overflow-y-auto bg-[var(--bg)] welcome-in">
      <div className="min-h-full flex items-center justify-center px-6 py-14">
        <div className="w-full max-w-lg">
          <div className="wordmark mb-10 text-center text-xl text-gray-400/80" style={{ letterSpacing: "0.34em" }}>
            Pushin
          </div>

          <h1 className="text-3xl font-light tracking-tight text-gray-100">Welcome.</h1>
          <p className="mt-3 text-sm leading-relaxed text-gray-500">
            Your private, on-device home base. Describe your day in plain language and a local AI plans it around your
            life; keep notes in a vault that never leaves your machine. First, a few basics — all editable later in Settings.
          </p>

          <div className="mt-9 space-y-8">
            {/* About you — seeds the AI's understanding of the user from day one. */}
            <section className="space-y-3">
              <h2 className="text-sm font-medium text-gray-200">A bit about you</h2>
              <p className="text-xs text-gray-500">
                Helps the on-device AI understand you from the start — pick what fits and add anything else. Optional, and editable later.
              </p>
              <AboutYou archetypes={form.archetypes ?? []} aboutMe={form.aboutMe ?? ""} onChange={update} />
            </section>

            {/* Working hours */}
            <section className="space-y-3">
              <h2 className="text-sm font-medium text-gray-200">When do you usually work?</h2>
              <div className="grid grid-cols-2 gap-3">
                <label className="block space-y-1">
                  <span className="text-xs text-gray-500">Start</span>
                  <input type="time" value={form.workStart} onChange={(e) => update({ workStart: e.target.value })} className={inputCls} />
                </label>
                <label className="block space-y-1">
                  <span className="text-xs text-gray-500">End</span>
                  <input type="time" value={form.workEnd} onChange={(e) => update({ workEnd: e.target.value })} className={inputCls} />
                </label>
              </div>
              <div className="flex flex-wrap gap-1.5 pt-1">
                {DAYS.map((d) => (
                  <button
                    key={d.n}
                    onClick={() => toggleDay(d.n)}
                    className={clsx(
                      "size-9 rounded-md text-xs transition",
                      form.workDays.includes(d.n) ? "bg-white/15 text-gray-100 border border-white/20" : "bg-white/5 text-gray-500 border border-white/10",
                    )}
                  >
                    {d.l}
                  </button>
                ))}
              </div>
            </section>

            {/* Sleep */}
            <section className="space-y-3">
              <h2 className="text-sm font-medium text-gray-200">Sleep schedule</h2>
              <SleepFields enabled={form.sleepEnabled} start={form.sleepStart} end={form.sleepEnd} onChange={update} />
            </section>

            {/* Routines */}
            <section className="space-y-3">
              <h2 className="text-sm font-medium text-gray-200">Routines &amp; blocked time</h2>
              <p className="text-xs text-gray-500">Recurring time to protect — lunch, gym, the commute. Pushin won't book work here.</p>
              <CommitmentList items={form.commitments} onChange={(commitments) => update({ commitments })} />
            </section>
          </div>

          <div className="flex items-center justify-end gap-3 pt-9">
            <button onClick={skip} disabled={busy} className="text-sm px-3 py-2 rounded-lg text-gray-500 hover:text-gray-300 disabled:opacity-50">
              Skip for now
            </button>
            <button
              onClick={save}
              disabled={busy}
              className="text-sm px-5 py-2 rounded-lg bg-white/90 text-gray-900 font-medium hover:bg-white disabled:opacity-50"
            >
              {busy ? "Setting up…" : "Get started"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
