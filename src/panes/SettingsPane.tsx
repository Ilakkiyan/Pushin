import { useState } from "react";
import { Calendar, Check, Cpu, RefreshCw } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { type Settings } from "../lib/ipc";

const DAYS = [
  { n: 1, l: "Mon" },
  { n: 2, l: "Tue" },
  { n: 3, l: "Wed" },
  { n: 4, l: "Thu" },
  { n: 5, l: "Fri" },
  { n: 6, l: "Sat" },
  { n: 7, l: "Sun" },
];

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs text-gray-400">{label}</span>
      {children}
    </label>
  );
}

const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50";

export default function SettingsPane() {
  const settings = useStore((s) => s.settings)!;
  const llm = useStore((s) => s.llm);
  const saveSettings = useStore((s) => s.saveSettings);
  const connectGoogle = useStore((s) => s.connectGoogle);
  const disconnectGoogle = useStore((s) => s.disconnectGoogle);
  const syncGoogle = useStore((s) => s.syncGoogle);
  const syncing = useStore((s) => s.syncing);
  const [form, setForm] = useState<Settings>(settings);
  const [saved, setSaved] = useState(false);
  const [googleMsg, setGoogleMsg] = useState("");
  const [googleBusy, setGoogleBusy] = useState(false);
  const [syncMsg, setSyncMsg] = useState("");

  const update = (patch: Partial<Settings>) => {
    setForm((f) => ({ ...f, ...patch }));
    setSaved(false);
  };

  const toggleDay = (n: number) =>
    update({ workDays: form.workDays.includes(n) ? form.workDays.filter((d) => d !== n) : [...form.workDays, n].sort() });

  const save = async () => {
    await saveSettings(form);
    setSaved(true);
  };

  const doConnect = async () => {
    setGoogleBusy(true);
    setGoogleMsg("Saving credentials and opening Google sign-in in your browser…");
    try {
      await saveSettings(form); // persist client id/secret first so the backend can use them
      const email = await connectGoogle();
      setGoogleMsg(`Connected as ${email}. Your calendar is now syncing both ways.`);
    } catch (e) {
      setGoogleMsg(String(e));
    } finally {
      setGoogleBusy(false);
    }
  };

  const doDisconnect = async () => {
    await disconnectGoogle();
    setGoogleMsg("Disconnected from Google Calendar.");
  };

  const syncNow = async () => {
    setSyncMsg("Syncing…");
    try {
      const s = await syncGoogle();
      setSyncMsg(`Synced — pulled ${s.pulled}, pushed ${s.pushed} event(s), mirrored ${s.blocksMirrored} task block(s).`);
    } catch (e) {
      setSyncMsg(String(e));
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="max-w-2xl mx-auto p-6 space-y-8">
        {/* Working hours */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Calendar className="size-4 text-indigo-400" /> Working hours</h2>
          <div className="grid grid-cols-2 gap-4">
            <Field label="Start"><input type="time" value={form.workStart} onChange={(e) => update({ workStart: e.target.value })} className={inputCls} /></Field>
            <Field label="End"><input type="time" value={form.workEnd} onChange={(e) => update({ workEnd: e.target.value })} className={inputCls} /></Field>
          </div>
          <div>
            <span className="text-xs text-gray-400">Work days</span>
            <div className="flex gap-1.5 mt-1">
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
          </div>
          <div className="grid grid-cols-3 gap-4">
            <Field label="Plan ahead (days)"><input type="number" min={1} max={60} value={form.horizonDays} onChange={(e) => update({ horizonDays: Number(e.target.value) })} className={inputCls} /></Field>
            <Field label="Buffer (min)"><input type="number" min={0} step={5} value={form.bufferMinutes} onChange={(e) => update({ bufferMinutes: Number(e.target.value) })} className={inputCls} /></Field>
            <Field label="Min block (min)"><input type="number" min={15} step={15} value={form.defaultMinChunk} onChange={(e) => update({ defaultMinChunk: Number(e.target.value) })} className={inputCls} /></Field>
          </div>
        </section>

        {/* AI model */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Cpu className="size-4 text-fuchsia-400" /> On-device AI</h2>
          <Field label="Model">
            <select value={form.modelId} onChange={(e) => update({ modelId: e.target.value })} className={inputCls}>
              {(llm?.models ?? [{ id: form.modelId, name: form.modelId }]).map((m) => (
                <option key={m.id} value={m.id} className="bg-[#12151c]">{m.name}</option>
              ))}
            </select>
          </Field>
          <Field label="Local inference server URL">
            <input value={form.llmBaseUrl} onChange={(e) => update({ llmBaseUrl: e.target.value })} placeholder="http://127.0.0.1:8080" className={inputCls} />
          </Field>
          <p className="text-[11px] text-gray-500">
            Status: {llm?.reachable ? <span className="text-emerald-400">reachable</span> : <span className="text-amber-400">offline</span>}. Point this at a local
            llama-server or an Ollama server (<code>http://127.0.0.1:11434</code>).
          </p>
        </section>

        {/* Google Calendar two-way sync */}
        <section className="space-y-3">
          <h2 className="text-sm font-semibold flex items-center gap-2"><RefreshCw className="size-4 text-sky-400" /> Google Calendar</h2>
          <p className="text-xs text-gray-500">
            Two-way sync with your <span className="text-gray-300">primary</span> calendar: Google events flow in (the scheduler plans
            around them) and your events + task blocks are mirrored out.
          </p>

          {!form.googleConnected && (
            <>
              <div className="rounded-lg border border-white/10 bg-white/[0.02] p-3 text-[11px] text-gray-400 leading-relaxed">
                <span className="text-gray-200">One-time setup:</span> in{" "}
                <a className="text-sky-400 underline" href="https://console.cloud.google.com/apis/credentials" target="_blank" rel="noopener noreferrer">Google Cloud Console</a>{" "}
                → enable the <span className="text-gray-300">Google Calendar API</span>, configure the OAuth consent screen (add yourself as a
                test user), then create an <span className="text-gray-300">OAuth client ID → Desktop app</span>. Paste the Client ID/secret below.
              </div>
              <Field label="OAuth Client ID">
                <input value={form.googleClientId} onChange={(e) => update({ googleClientId: e.target.value })} placeholder="xxxxx.apps.googleusercontent.com" className={inputCls} />
              </Field>
              <Field label="OAuth Client secret">
                <input type="password" value={form.googleClientSecret} onChange={(e) => update({ googleClientSecret: e.target.value })} placeholder="GOCSPX-…" className={inputCls} />
              </Field>
              <button
                onClick={doConnect}
                disabled={googleBusy || !form.googleClientId.trim()}
                className="text-xs px-3 py-1.5 rounded-md bg-sky-500/80 hover:bg-sky-500 disabled:opacity-50"
              >
                {googleBusy ? "Connecting…" : "Connect Google Calendar"}
              </button>
            </>
          )}

          {form.googleConnected && (
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-xs px-2 py-1 rounded-full bg-emerald-500/10 border border-emerald-500/30 text-emerald-300">● Connected</span>
              <button onClick={syncNow} disabled={syncing} className="text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15 disabled:opacity-50">
                {syncing ? "Syncing…" : "Sync now"}
              </button>
              <button onClick={doDisconnect} className="text-xs px-3 py-1.5 rounded-md bg-white/5 hover:bg-white/10 text-gray-400">Disconnect</button>
            </div>
          )}

          {googleMsg && <p className="text-xs text-gray-400">{googleMsg}</p>}
          {syncMsg && <p className="text-xs text-gray-400">{syncMsg}</p>}
        </section>

        <div className="flex items-center gap-3 pt-2">
          <button onClick={save} className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400">
            {saved ? <Check className="size-4" /> : null}
            {saved ? "Saved" : "Save settings"}
          </button>
          <span className="text-xs text-gray-500">Saving re-plans your calendar.</span>
        </div>
      </div>
    </div>
  );
}
