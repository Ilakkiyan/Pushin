import { useEffect, useState } from "react";
import { BookOpen, Calendar, Check, Cpu, DownloadCloud, ExternalLink, Github, Loader2, Moon, RefreshCw } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import type { Update } from "@tauri-apps/plugin-updater";
import clsx from "clsx";
import { useStore } from "../state/store";
import { type Settings } from "../lib/ipc";
import { checkForUpdate, installUpdate } from "../lib/updates";
import { CommitmentList, SleepFields } from "../components/Personalization";
import DevicesSync from "../components/DevicesSync";

const REPO_URL = "https://github.com/Ilakkiyan/Pushin";
const DOCS = {
  repo: REPO_URL,
  googleSetup: `${REPO_URL}#google-calendar-sync-optional`,
  troubleshooting: `${REPO_URL}#troubleshooting`,
};

/** Open a URL in the user's default browser (Tauri opener), with a web fallback for `vite` preview. */
function openExternal(url: string) {
  openUrl(url).catch(() => window.open(url, "_blank", "noopener,noreferrer"));
}

/** Anchor that opens externally via the OS browser instead of navigating the app webview. */
function ExtLink({ href, className, children }: { href: string; className?: string; children: React.ReactNode }) {
  return (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      onClick={(e) => {
        e.preventDefault();
        openExternal(href);
      }}
      className={className}
    >
      {children}
    </a>
  );
}

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

  // In-app auto-update (desktop). `appVersion` is the running build; `pendingUpdate` holds a found
  // newer release so the user can install it from here as well as from the launch banner.
  const [appVersion, setAppVersion] = useState("");
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState("");
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installPct, setInstallPct] = useState<number | null>(null);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  const checkUpdates = async () => {
    setCheckingUpdate(true);
    setUpdateMsg("");
    setPendingUpdate(null);
    try {
      const u = await checkForUpdate();
      if (u) {
        setPendingUpdate(u);
        setUpdateMsg(`Pushin ${u.version} is available.`);
      } else {
        setUpdateMsg(`You're on the latest version${appVersion ? ` (v${appVersion})` : ""}.`);
      }
    } catch (e) {
      setUpdateMsg(`Couldn't check for updates: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      setCheckingUpdate(false);
    }
  };

  const installUpdates = async () => {
    if (!pendingUpdate) return;
    setInstalling(true);
    try {
      await installUpdate(pendingUpdate, (p) => setInstallPct(p.pct));
      // installUpdate relaunches on success — nothing after this runs.
    } catch (e) {
      setInstalling(false);
      setUpdateMsg(`Update failed: ${e instanceof Error ? e.message : String(e)}`);
    }
  };

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
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-2xl mx-auto p-4 sm:p-6 space-y-8">
        {/* Working hours */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Calendar className="size-4 text-indigo-400" /> Working hours</h2>
          <div className="grid grid-cols-2 gap-4">
            <Field label="Start"><input type="time" value={form.workStart} onChange={(e) => update({ workStart: e.target.value })} className={inputCls} /></Field>
            <Field label="End"><input type="time" value={form.workEnd} onChange={(e) => update({ workEnd: e.target.value })} className={inputCls} /></Field>
          </div>
          <div>
            <span className="text-xs text-gray-400">Work days</span>
            <div className="flex flex-wrap gap-1.5 mt-1">
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
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-4">
            <Field label="Plan ahead (days)"><input type="number" min={1} max={60} value={form.horizonDays} onChange={(e) => update({ horizonDays: Number(e.target.value) })} className={inputCls} /></Field>
            <Field label="Buffer (min)"><input type="number" min={0} step={5} value={form.bufferMinutes} onChange={(e) => update({ bufferMinutes: Number(e.target.value) })} className={inputCls} /></Field>
            <Field label="Min block (min)"><input type="number" min={15} step={15} value={form.defaultMinChunk} onChange={(e) => update({ defaultMinChunk: Number(e.target.value) })} className={inputCls} /></Field>
          </div>
        </section>

        {/* Personal routine: sleep + recurring blocked time the scheduler & AI plan around */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Moon className="size-4 text-indigo-400" /> Your routine</h2>
          <p className="text-xs text-gray-500">
            Time the scheduler keeps free and the AI plans around. Sleep, meals, gym, commute — whatever's yours.
          </p>
          <SleepFields enabled={form.sleepEnabled} start={form.sleepStart} end={form.sleepEnd} onChange={update} />
          <div className="space-y-2">
            <span className="text-xs text-gray-400">Routines & blocked time</span>
            <CommitmentList items={form.commitments} onChange={(commitments) => update({ commitments })} />
          </div>
        </section>

        {/* AI model */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Cpu className="size-4 text-fuchsia-400" /> On-device AI</h2>
          <Field label="Model">
            <select value={form.modelId} onChange={(e) => update({ modelId: e.target.value })} className={inputCls}>
              {(llm?.models ?? [{ id: form.modelId, name: form.modelId }]).map((m) => (
                <option key={m.id} value={m.id} className="bg-[var(--raised)]">{m.name}</option>
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
          <Field label="Embedding model — Hermes recall">
            <input value={form.embedModel} onChange={(e) => update({ embedModel: e.target.value })} placeholder="bge-small-en-v1.5-q8_0" className={inputCls} />
          </Field>
          <p className="text-[11px] text-gray-500">
            Powers semantic memory recall in <span className="text-gray-300">Hermes</span>. Pushin downloads a small embedding model
            (~37 MB) and runs it on-device automatically — no setup. Leave blank to use keyword-only recall.
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
              <div className="rounded-lg border border-white/10 bg-white/[0.02] p-3 text-[11px] text-gray-400 leading-relaxed space-y-1.5">
                <p className="text-gray-200">One-time setup in the{" "}
                  <ExtLink className="text-sky-400 underline" href="https://console.cloud.google.com/">Google Cloud Console</ExtLink>:</p>
                <ol className="list-decimal pl-4 space-y-1">
                  <li>Create or pick a project.</li>
                  <li>Enable the <span className="text-gray-300">Google Calendar API</span> (APIs &amp; Services → Library).</li>
                  <li>Configure the OAuth consent screen: <span className="text-gray-300">External</span>, and add your Gmail under <span className="text-gray-300">Test users</span>.</li>
                  <li>Create credentials → OAuth client ID → <span className="text-gray-300">Application type: Desktop app</span> (not Web).</li>
                  <li>Copy the Client ID &amp; secret into the fields below.</li>
                  <li>After connecting, <span className="text-gray-300">Publish app</span> (consent screen → Production) so sync doesn't expire after 7 days.</li>
                </ol>
                <p className="pt-0.5">
                  When the browser opens, you'll see <span className="text-gray-300">"Google hasn't verified this app"</span> — that's expected for your own
                  app. Click <span className="text-gray-300">Advanced → Go to Pushin (unsafe)</span> to continue. It's safe: this is the client <em>you</em> just
                  created, and the exchange happens locally on your machine.
                </p>
                <p className="text-gray-500">Full walkthrough &amp; troubleshooting in the{" "}
                  <ExtLink className="text-sky-400 underline" href={DOCS.googleSetup}>project README</ExtLink>.</p>
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

        {/* Device-to-device sync (private Iroh mesh) */}
        <DevicesSync />

        {/* Documentation */}
        <section className="space-y-3">
          <h2 className="text-sm font-semibold flex items-center gap-2"><BookOpen className="size-4 text-emerald-400" /> Documentation</h2>
          <p className="text-xs text-gray-500">
            Setup guides, the full Google Calendar walkthrough, and troubleshooting live on GitHub — they open in your browser.
          </p>
          <div className="flex flex-wrap gap-2">
            <ExtLink href={DOCS.repo} className="inline-flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15">
              <Github className="size-3.5" /> GitHub repository
            </ExtLink>
            <ExtLink href={DOCS.googleSetup} className="inline-flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15">
              <ExternalLink className="size-3.5" /> Google Calendar setup
            </ExtLink>
            <ExtLink href={DOCS.troubleshooting} className="inline-flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15">
              <ExternalLink className="size-3.5" /> Troubleshooting
            </ExtLink>
          </div>
        </section>

        {/* In-app auto-update from GitHub Releases */}
        <section className="space-y-3">
          <h2 className="text-sm font-semibold flex items-center gap-2"><DownloadCloud className="size-4 text-indigo-400" /> Updates</h2>
          <p className="text-xs text-gray-500">
            Pushin checks GitHub for a newer release on launch and offers a one-click update. Installing keeps all your
            data — tasks, notes, people, and settings live outside the app and aren't touched.
          </p>
          <div className="flex items-center gap-2 flex-wrap">
            {appVersion && <span className="text-xs px-2 py-1 rounded-full bg-white/5 border border-white/10 text-gray-300">v{appVersion}</span>}
            {pendingUpdate ? (
              <button
                onClick={installUpdates}
                disabled={installing}
                className="text-xs px-3 py-1.5 rounded-md bg-white/90 hover:bg-white text-gray-900 disabled:opacity-50 inline-flex items-center gap-1.5"
              >
                {installing ? <Loader2 className="size-3.5 animate-spin" /> : <DownloadCloud className="size-3.5" />}
                {installing ? (installPct !== null ? `Installing ${installPct}%…` : "Installing…") : `Update to ${pendingUpdate.version} & restart`}
              </button>
            ) : (
              <button
                onClick={checkUpdates}
                disabled={checkingUpdate}
                className="text-xs px-3 py-1.5 rounded-md bg-white/10 hover:bg-white/15 disabled:opacity-50 inline-flex items-center gap-1.5"
              >
                {checkingUpdate ? <Loader2 className="size-3.5 animate-spin" /> : <RefreshCw className="size-3.5" />}
                {checkingUpdate ? "Checking…" : "Check for updates"}
              </button>
            )}
          </div>
          {updateMsg && <p className="text-xs text-gray-400">{updateMsg}</p>}
        </section>

        <div className="flex items-center gap-3 pt-2">
          <button onClick={save} className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-white/90 hover:bg-white text-gray-900">
            {saved ? <Check className="size-4" /> : null}
            {saved ? "Saved" : "Save settings"}
          </button>
          <span className="text-xs text-gray-500">Saving re-plans your calendar.</span>
        </div>
      </div>
    </div>
  );
}
