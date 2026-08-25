import { useEffect, useState } from "react";
import { BookOpen, Calendar, Check, Cpu, DownloadCloud, ExternalLink, FolderOpen, Github, Loader2, Moon, RefreshCw, Sparkles, UserRound } from "lucide-react";
import { openUrl, openPath } from "@tauri-apps/plugin-opener";
import { open } from "@tauri-apps/plugin-dialog";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import type { Update } from "@tauri-apps/plugin-updater";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type IcsSubscription, type Settings } from "../lib/ipc";
import { checkForUpdate, installUpdate } from "../lib/updates";
import { exportAllPages } from "../lib/vaultExport";
import { AboutYou, CommitmentList, SleepFields } from "../components/Personalization";
import AiMemory from "../components/AiMemory";
import DevicesSync from "../components/DevicesSync";

// Base (vanilla Qwen) model → Pushin's tuned equivalent. Drives the one-time "a more reliable model is
// available" nudge for users still on a base model. 14B has no tuned build, so it maps to the tuned 7B
// (accuracy beats raw size for this task — same reasoning as model_manager::recommend_model).
const TUNED_FOR_BASE: Record<string, string> = {
  "qwen2.5-3b-instruct-q4_k_m": "pushin-arch3b-tuned-q4_k_m",
  "qwen2.5-7b-instruct-q4_k_m": "pushin-arch7b-chat-tuned-q4_k_m",
  "qwen2.5-14b-instruct-q4_k_m": "pushin-arch7b-chat-tuned-q4_k_m",
};

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

const inputCls = "w-full bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-white/30";

export default function SettingsPane() {
  const settings = useStore((s) => s.settings)!;
  const llm = useStore((s) => s.llm);
  const saveSettings = useStore((s) => s.saveSettings);
  const connectGoogle = useStore((s) => s.connectGoogle);
  const disconnectGoogle = useStore((s) => s.disconnectGoogle);
  const syncGoogle = useStore((s) => s.syncGoogle);
  const syncing = useStore((s) => s.syncing);
  const pages = useStore((s) => s.pages);
  const [form, setForm] = useState<Settings>(settings);
  const [saved, setSaved] = useState(false);
  const [modelMsg, setModelMsg] = useState("");
  // Which chat models are downloaded, so switching to an un-downloaded one fetches it first (a switch to a
  // missing model would otherwise just fail — the server can only load a GGUF that's on disk).
  const [present, setPresent] = useState<Record<string, boolean>>({});
  const [switchingModel, setSwitchingModel] = useState(false);
  const [dlPct, setDlPct] = useState<number | null>(null);
  const [tunedNudgeDismissed, setTunedNudgeDismissed] = useState(
    () => typeof localStorage !== "undefined" && localStorage.getItem("pushin:tunedNudgeDismissed") === "1",
  );
  const [icsSubs, setIcsSubs] = useState<IcsSubscription[]>([]);
  const [icsName, setIcsName] = useState("");
  const [icsUrl, setIcsUrl] = useState("");
  const [icsBusy, setIcsBusy] = useState(false);
  const [icsMsg, setIcsMsg] = useState("");
  const [googleMsg, setGoogleMsg] = useState("");
  const [googleBusy, setGoogleBusy] = useState(false);
  const [syncMsg, setSyncMsg] = useState("");
  const [vaultMsg, setVaultMsg] = useState("");

  // Vault folder: pick a directory, persist it, and bulk-export existing notes so it isn't empty.
  const chooseVault = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Choose a vault folder" });
    if (!picked || Array.isArray(picked)) return;
    const next = { ...form, vaultDir: picked };
    setForm(next);
    await saveSettings(next);
    setVaultMsg("Exporting your notes…");
    try {
      const n = await exportAllPages(pages);
      setVaultMsg(`Mirrored ${n} note${n === 1 ? "" : "s"} to this folder.`);
    } catch {
      setVaultMsg("Folder set — notes will export as you edit them.");
    }
    // Start watching the new folder so external edits flow back in (files → DB).
    await api.vaultRefreshWatch().catch(() => {});
  };
  const revealVault = () => {
    if (form.vaultDir) openPath(form.vaultDir).catch(() => {});
  };

  // In-app auto-update (desktop). `appVersion` is the running build; `pendingUpdate` holds a found
  // newer release so the user can install it from here as well as from the launch banner.
  const [appVersion, setAppVersion] = useState("");
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateMsg, setUpdateMsg] = useState("");
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installPct, setInstallPct] = useState<number | null>(null);

  useEffect(() => {
    api.listIcsSubscriptions().then(setIcsSubs).catch(() => {});
  }, []);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  // Track which chat models are on disk (drives the "not downloaded" hint + download-on-switch).
  const models = llm?.models ?? [];
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const map: Record<string, boolean> = {};
      for (const m of models) map[m.id] = await api.modelPresent(m.id);
      if (!cancelled) setPresent(map);
    })();
    return () => {
      cancelled = true;
    };
  }, [llm?.models?.length]);

  // Live download progress while switching to a not-yet-downloaded model.
  useEffect(() => {
    const un = listen<{ downloaded: number; total: number }>("model-download-progress", (e) => {
      const { downloaded, total } = e.payload;
      setDlPct(total ? Math.round((downloaded / total) * 100) : 0);
    });
    return () => {
      un.then((f) => f());
    };
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

  // Persist `next`, then — if the model changed — download it if missing and restart the server on it.
  // Persisting settings only writes model_id to the DB; the running llama-server keeps serving the OLD
  // model until restarted. Shared by the Save button and the "switch to the tuned model" nudge.
  const persistAndMaybeSwitch = async (next: Settings) => {
    const modelChanged = next.modelId !== settings.modelId;
    await saveSettings(next);
    setSaved(true);
    if (modelChanged) {
      setSwitchingModel(true);
      try {
        const modelName = models.find((m) => m.id === next.modelId)?.name ?? next.modelId;
        if (!present[next.modelId]) {
          setDlPct(0);
          setModelMsg(`Downloading ${modelName}…`);
          await api.downloadModel(next.modelId);
          setPresent((p) => ({ ...p, [next.modelId]: true }));
          setDlPct(null);
        }
        setModelMsg(`Switching to ${modelName}…`);
        await api.restartInference();
        setModelMsg(`Now running ${modelName}.`);
      } catch (e) {
        setModelMsg(`Couldn't switch model: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        setDlPct(null);
        setSwitchingModel(false);
        useStore.getState().refreshLlm();
      }
    }
  };

  const save = () => persistAndMaybeSwitch(form);

  // One-time nudge for users still on a vanilla base model → offer the tuned equivalent (keyed off the
  // SAVED model, not the unsaved form). Dismissal is remembered so it never nags.
  const suggestTunedId = TUNED_FOR_BASE[settings.modelId];
  const tunedName = models.find((m) => m.id === suggestTunedId)?.name ?? "Pushin's tuned model";
  const showTunedNudge = !!suggestTunedId && !tunedNudgeDismissed && !switchingModel;
  const switchToTuned = async () => {
    if (!suggestTunedId) return;
    const next = { ...form, modelId: suggestTunedId };
    setForm(next);
    await persistAndMaybeSwitch(next);
  };
  const dismissTunedNudge = () => {
    try {
      localStorage.setItem("pushin:tunedNudgeDismissed", "1");
    } catch {
      /* private mode / no storage — just hide it for this session */
    }
    setTunedNudgeDismissed(true);
  };

  // Read-only .ics calendar subscriptions. After add/refresh/remove the backend reschedules around the
  // new fixed events, so reload app data to repaint the calendar.
  const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const addIcs = async () => {
    if (!icsUrl.trim()) return;
    setIcsBusy(true);
    setIcsMsg("Fetching calendar…");
    try {
      const sub = await api.addIcsSubscription(icsName, icsUrl);
      setIcsSubs((s) => [...s, sub]);
      setIcsName("");
      setIcsUrl("");
      setIcsMsg("Added.");
      await useStore.getState().load();
    } catch (e) {
      setIcsMsg(errMsg(e));
    } finally {
      setIcsBusy(false);
    }
  };
  const refreshIcs = async () => {
    setIcsBusy(true);
    setIcsMsg("Refreshing…");
    try {
      const n = await api.refreshIcsSubscriptions();
      setIcsSubs(await api.listIcsSubscriptions());
      setIcsMsg(`Refreshed — ${n} event${n === 1 ? "" : "s"}.`);
      await useStore.getState().load();
    } catch (e) {
      setIcsMsg(errMsg(e));
    } finally {
      setIcsBusy(false);
    }
  };
  const removeIcs = async (id: number) => {
    try {
      await api.removeIcsSubscription(id);
      setIcsSubs((s) => s.filter((x) => x.id !== id));
      await useStore.getState().load();
    } catch {
      /* ignore */
    }
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
        {/* About you — feeds the on-device AI's understanding of the user. */}
        <section className="space-y-4">
          <h2 className="text-sm font-semibold flex items-center gap-2"><UserRound className="size-4 text-indigo-400" /> About you</h2>
          <p className="text-[11px] text-gray-500">
            Pick the archetypes that fit and add anything else — it's fed to the on-device AI so it understands you. Never leaves your device.
          </p>
          <AboutYou archetypes={form.archetypes ?? []} aboutMe={form.aboutMe ?? ""} onChange={update} />
        </section>

        <AiMemory />

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
                    "size-9 text-xs border",
                    form.workDays.includes(d.n) ? "bg-white/20 text-white border-white/40" : "bg-white/5 text-[var(--ink-muted)] border-white/10 hover:bg-white/10",
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
          {showTunedNudge && (
            <div className="flex items-start gap-3 rounded-lg border border-fuchsia-400/30 bg-fuchsia-400/[0.06] p-3">
              <Sparkles className="mt-0.5 size-4 shrink-0 text-fuchsia-300" />
              <div className="min-w-0 flex-1">
                <p className="text-xs text-gray-200">
                  A more reliable on-device model is available. <span className="text-gray-100">{tunedName}</span> reads
                  your plans more accurately at the same size — it's Pushin's own fine-tune.
                </p>
                <div className="mt-2 flex items-center gap-3">
                  <button
                    onClick={switchToTuned}
                    disabled={switchingModel}
                    className="rounded-md bg-white/90 px-2.5 py-1 text-[11px] font-medium text-gray-900 transition hover:bg-white disabled:opacity-50"
                  >
                    Switch to {tunedName}
                  </button>
                  <button onClick={dismissTunedNudge} className="text-[11px] text-gray-400 transition hover:text-gray-200">
                    Not now
                  </button>
                </div>
              </div>
            </div>
          )}
          <Field label="Model">
            <select value={form.modelId} onChange={(e) => update({ modelId: e.target.value })} className={inputCls}>
              {(llm?.models ?? [{ id: form.modelId, name: form.modelId }]).map((m) => (
                <option key={m.id} value={m.id} className="bg-[var(--raised)]">{m.name}</option>
              ))}
            </select>
          </Field>
          {form.modelId !== settings.modelId && !switchingModel && (
            <p className="text-[11px] text-amber-400">
              {present[form.modelId] === false
                ? `Not downloaded yet — Save will download it (~${Math.round((models.find((m) => m.id === form.modelId)?.sizeMb ?? 0) / 10) / 100} GB), then restart the AI on it.`
                : "Save to load this model — the AI restarts on the new model."}
            </p>
          )}
          {dlPct !== null && (
            <div className="h-1.5 rounded-full bg-white/10 overflow-hidden">
              <div className="h-full bg-indigo-400 transition-all" style={{ width: `${dlPct}%` }} />
            </div>
          )}
          {modelMsg && (
            <p className="text-[11px] text-gray-400 flex items-center gap-1.5">
              {switchingModel && <Loader2 className="size-3 animate-spin" />}
              {modelMsg}
              {dlPct !== null && ` ${dlPct}%`}
            </p>
          )}
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
          <Field label="Unload the model when idle">
            <select
              value={String(form.idleUnloadMinutes)}
              onChange={(e) => update({ idleUnloadMinutes: Number(e.target.value) })}
              className={inputCls}
            >
              <option value="0" className="bg-[var(--raised)]">Never — keep it loaded</option>
              <option value="5" className="bg-[var(--raised)]">After 5 minutes idle</option>
              <option value="10" className="bg-[var(--raised)]">After 10 minutes idle</option>
              <option value="30" className="bg-[var(--raised)]">After 30 minutes idle</option>
            </select>
          </Field>
          <p className="text-[11px] text-gray-500">
            Frees the model's memory (several GB) after you stop using AI for a while; it reloads
            automatically the next time you plan or chat. Turn off for instant responses at all times.
          </p>
        </section>

        {/* Subscribed calendars (read-only .ics feeds) */}
        <section className="space-y-3">
          <h2 className="text-sm font-semibold flex items-center gap-2"><Calendar className="size-4 text-sky-400" /> Subscribed calendars</h2>
          <p className="text-[11px] text-gray-500">
            Add a read-only iCalendar (<code>.ics</code>) feed by URL — a shared calendar, a team
            schedule, holidays. Its events appear on your calendar and the scheduler plans around them.
            Recurring events currently show their next occurrence.
          </p>
          {icsSubs.length > 0 && (
            <div className="space-y-1.5">
              {icsSubs.map((s) => (
                <div key={s.id} className="flex items-center gap-2 rounded-md border border-white/10 bg-white/[0.03] px-2.5 py-1.5">
                  <span className="size-2 shrink-0 rounded-full" style={{ background: s.color }} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-xs text-gray-200">{s.name}</div>
                    <div className="truncate text-[10px] text-gray-500">{s.url}</div>
                  </div>
                  <button onClick={() => removeIcs(s.id)} className="shrink-0 text-[11px] text-gray-400 transition hover:text-red-300">Remove</button>
                </div>
              ))}
            </div>
          )}
          <div className="flex flex-col gap-2 sm:flex-row">
            <input value={icsName} onChange={(e) => setIcsName(e.target.value)} placeholder="Name (optional)" className={clsx(inputCls, "sm:w-40")} />
            <input value={icsUrl} onChange={(e) => setIcsUrl(e.target.value)} placeholder="https://…/calendar.ics" className={inputCls} />
            <button onClick={addIcs} disabled={icsBusy || !icsUrl.trim()} className="shrink-0 rounded-md bg-white/90 px-3 py-1.5 text-sm font-medium text-gray-900 transition hover:bg-white disabled:opacity-50">
              Add
            </button>
          </div>
          <div className="flex items-center gap-3">
            <button onClick={refreshIcs} disabled={icsBusy || icsSubs.length === 0} className="text-[11px] text-indigo-300 transition hover:text-indigo-200 disabled:opacity-40">
              Refresh all
            </button>
            {icsMsg && <span className="text-[11px] text-gray-500">{icsMsg}</span>}
          </div>
        </section>

        {/* Two-way markdown vault */}
        <section className="space-y-3">
          <h2 className="text-sm font-semibold flex items-center gap-2"><FolderOpen className="size-4 text-amber-400" /> Vault folder</h2>
          <p className="text-[11px] text-gray-500">
            Mirror your notes as markdown files in a folder you choose — edit them in Pushin or any editor,
            and see them in your file manager. Leave unset to keep the vault inside Pushin only.
          </p>
          {form.vaultDir ? (
            <div className="flex items-center gap-2">
              <code className="flex-1 truncate rounded bg-[var(--raised)] px-2 py-1.5 text-[11px] text-gray-300" title={form.vaultDir}>{form.vaultDir}</code>
              <button onClick={revealVault} className="rounded-lg border border-white/10 bg-[var(--raised)] px-3 py-1.5 text-xs font-medium text-gray-200 hover:bg-white/10">Reveal</button>
              <button onClick={chooseVault} className="rounded-lg border border-white/10 bg-[var(--raised)] px-3 py-1.5 text-xs font-medium text-gray-200 hover:bg-white/10">Change…</button>
            </div>
          ) : (
            <button onClick={chooseVault} className="rounded-lg bg-white/90 px-3 py-1.5 text-xs font-medium text-gray-900 hover:bg-white">Choose folder…</button>
          )}
          {vaultMsg && <p className="text-[11px] text-gray-400">{vaultMsg}</p>}
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
              <span className="text-xs px-2 py-1 bg-emerald-500/10 border border-emerald-500/30 text-emerald-300">● Connected</span>
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
            {appVersion && <span className="tnum text-xs px-2 py-1 bg-white/5 border border-white/10 text-gray-300">v{appVersion}</span>}
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
          <button onClick={save} disabled={switchingModel} className="flex items-center gap-2 text-sm px-4 py-2 rounded-lg bg-white/90 hover:bg-white text-gray-900 disabled:opacity-50">
            {switchingModel ? <Loader2 className="size-4 animate-spin" /> : saved ? <Check className="size-4" /> : null}
            {switchingModel ? "Switching model…" : saved ? "Saved" : "Save settings"}
          </button>
          <span className="text-xs text-gray-500">Saving re-plans your calendar.</span>
        </div>
      </div>
    </div>
  );
}
