// Shared editors for the user's personal schedule (sleep window + recurring routines/blocked
// time). Used by both the first-run OnboardingModal and the Settings pane so the two stay in sync.
import { Briefcase, GraduationCap, Heart, Moon, Palette, Plus, Rocket, Trash2, Users } from "lucide-react";
import clsx from "clsx";
import type { Commitment } from "../lib/ipc";

const DAYS = [
  { n: 1, l: "M" },
  { n: 2, l: "T" },
  { n: 3, l: "W" },
  { n: 4, l: "T" },
  { n: 5, l: "F" },
  { n: 6, l: "S" },
  { n: 7, l: "S" },
];

const inputCls = "rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50";

/** A fresh, empty routine row. */
export function newCommitment(): Commitment {
  const id = typeof crypto !== "undefined" && crypto.randomUUID ? crypto.randomUUID() : String(Date.now() + Math.random());
  return { id, name: "", start: "12:00", end: "13:00", days: [], kind: "blocked" };
}

export function SleepFields({
  enabled,
  start,
  end,
  onChange,
}: {
  enabled: boolean;
  start: string;
  end: string;
  onChange: (patch: Partial<{ sleepEnabled: boolean; sleepStart: string; sleepEnd: string }>) => void;
}) {
  return (
    <div className="space-y-3">
      <label className="flex items-center gap-2 text-sm cursor-pointer">
        <input
          type="checkbox"
          checked={enabled}
          // Enabling always lands on valid times (older settings rows have empty strings).
          onChange={(e) => onChange(e.target.checked ? { sleepEnabled: true, sleepStart: start || "23:00", sleepEnd: end || "07:00" } : { sleepEnabled: false })}
          className="accent-indigo-500"
        />
        <Moon className="size-4 text-indigo-300" /> Keep my sleep time free
      </label>
      {enabled && (
        <div className="grid grid-cols-2 gap-3 pl-6">
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Bedtime</span>
            <input type="time" value={start} onChange={(e) => onChange({ sleepStart: e.target.value })} className={clsx(inputCls, "w-full")} />
          </label>
          <label className="block space-y-1">
            <span className="text-xs text-gray-400">Wake up</span>
            <input type="time" value={end} onChange={(e) => onChange({ sleepEnd: e.target.value })} className={clsx(inputCls, "w-full")} />
          </label>
        </div>
      )}
    </div>
  );
}

export function CommitmentList({ items, onChange }: { items: Commitment[]; onChange: (items: Commitment[]) => void }) {
  const update = (id: string, patch: Partial<Commitment>) => onChange(items.map((c) => (c.id === id ? { ...c, ...patch } : c)));
  const remove = (id: string) => onChange(items.filter((c) => c.id !== id));
  const add = () => onChange([...items, newCommitment()]);
  const toggleDay = (c: Commitment, n: number) => update(c.id, { days: c.days.includes(n) ? c.days.filter((d) => d !== n) : [...c.days, n].sort() });

  return (
    <div className="space-y-3">
      {items.length === 0 && (
        <p className="text-xs text-gray-500">Nothing yet — add things like lunch, gym, the school run, or “no work after 6pm.”</p>
      )}
      {items.map((c) => (
        <div key={c.id} className="rounded-lg border border-white/10 bg-white/[0.02] p-2.5 space-y-2">
          <div className="flex items-center gap-2">
            <input
              value={c.name}
              onChange={(e) => update(c.id, { name: e.target.value })}
              placeholder="e.g. Lunch, Gym, Family dinner"
              className={clsx(inputCls, "flex-1")}
            />
            <button onClick={() => remove(c.id)} className="p-1.5 rounded-md text-gray-500 hover:text-rose-300 hover:bg-white/5" title="Remove">
              <Trash2 className="size-4" />
            </button>
          </div>
          <div className="flex items-center gap-2 flex-wrap">
            <input type="time" value={c.start} onChange={(e) => update(c.id, { start: e.target.value })} className={inputCls} />
            <span className="text-xs text-gray-500">to</span>
            <input type="time" value={c.end} onChange={(e) => update(c.id, { end: e.target.value })} className={inputCls} />
            <div className="ml-auto flex gap-1" title="Tap days to limit to specific weekdays — all lit means every day.">
              {DAYS.map((d) => {
                const on = c.days.length === 0 || c.days.includes(d.n);
                return (
                  <button
                    key={d.n}
                    onClick={() => toggleDay(c, d.n)}
                    className={clsx("size-6 rounded text-[11px]", on ? "bg-indigo-500/30 text-indigo-100 border border-indigo-400/40" : "bg-white/5 text-gray-500 border border-white/10")}
                  >
                    {d.l}
                  </button>
                );
              })}
            </div>
          </div>
          {c.end <= c.start && <p className="text-[11px] text-amber-400/80">Runs overnight — {c.start} until {c.end} the next day.</p>}
        </div>
      ))}
      <button onClick={add} className="flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15">
        <Plus className="size-3.5" /> Add routine / blocked time
      </button>
    </div>
  );
}

/** Selectable user archetypes (multi-select). Keys must match `Settings::profile_prompt` in Rust. */
export const ARCHETYPES = [
  { key: "builder", label: "Builder / Founder", icon: Rocket, blurb: "Shipping something — protects deep-work time." },
  { key: "student", label: "Student", icon: GraduationCap, blurb: "Classes, study blocks, exam deadlines." },
  { key: "creator", label: "Creator", icon: Palette, blurb: "Content or art — project-driven, flexible." },
  { key: "operator", label: "Operator / Manager", icon: Users, blurb: "Lots of meetings, coordinating people." },
  { key: "freelancer", label: "Freelancer", icon: Briefcase, blurb: "Multiple clients, varied work." },
  { key: "parent", label: "Parent / Caregiver", icon: Heart, blurb: "Family routines and errands to weave in." },
] as const;

/** "About you": pick archetypes + a free-form blurb. Feeds the AI's system prompt so it understands the
 *  user from day one. Shared by the WelcomeGuide (setup) and the Settings pane. */
export function AboutYou({
  archetypes,
  aboutMe,
  onChange,
}: {
  archetypes: string[];
  aboutMe: string;
  onChange: (patch: Partial<{ archetypes: string[]; aboutMe: string }>) => void;
}) {
  const toggle = (key: string) =>
    onChange({ archetypes: archetypes.includes(key) ? archetypes.filter((a) => a !== key) : [...archetypes, key] });
  return (
    <div className="space-y-3">
      <div className="grid grid-cols-2 gap-2">
        {ARCHETYPES.map((a) => {
          const Icon = a.icon;
          const on = archetypes.includes(a.key);
          return (
            <button
              key={a.key}
              type="button"
              onClick={() => toggle(a.key)}
              className={clsx(
                "flex items-start gap-2.5 rounded-lg border p-2.5 text-left transition",
                on ? "border-white/30 bg-white/[0.07]" : "border-white/10 bg-white/[0.02] hover:bg-white/[0.04]",
              )}
            >
              <Icon className={clsx("size-4 mt-0.5 shrink-0", on ? "text-gray-100" : "text-gray-500")} />
              <div className="min-w-0">
                <div className={clsx("text-xs font-medium", on ? "text-gray-100" : "text-gray-300")}>{a.label}</div>
                <div className="text-[11px] leading-snug text-gray-500">{a.blurb}</div>
              </div>
            </button>
          );
        })}
      </div>
      <textarea
        value={aboutMe}
        onChange={(e) => onChange({ aboutMe: e.target.value })}
        rows={3}
        placeholder="Anything that helps the AI understand you — your goals, what you're working on, how you like to work, what matters to you…"
        className="w-full rounded-md bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-white/25 resize-y placeholder:text-gray-600"
      />
    </div>
  );
}
