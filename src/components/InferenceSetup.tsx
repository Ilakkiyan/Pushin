import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Check, Cpu, Download, Loader2, Plug, ShieldCheck } from "lucide-react";
import clsx from "clsx";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

interface Progress {
  downloaded: number;
  total: number;
}

export default function InferenceSetup() {
  const llm = useStore((s) => s.llm);
  const settings = useStore((s) => s.settings);
  const refreshLlm = useStore((s) => s.refreshLlm);
  const saveSettings = useStore((s) => s.saveSettings);
  const [msg, setMsg] = useState<string>("");
  const [working, setWorking] = useState(false);
  const [progress, setProgress] = useState<Record<string, Progress>>({});
  const [downloading, setDownloading] = useState<string | null>(null);
  const [present, setPresent] = useState<Record<string, boolean>>({});

  const models = llm?.models ?? [];

  // Which models are already downloaded?
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

  // Live download + setup status events.
  useEffect(() => {
    const unProg = listen<Progress>("model-download-progress", (e) => {
      if (downloading) setProgress((p) => ({ ...p, [downloading]: e.payload }));
    });
    const unStatus = listen<string>("inference-status", (e) => setMsg(e.payload));
    return () => {
      unProg.then((f) => f());
      unStatus.then((f) => f());
    };
  }, [downloading]);

  const anyPresent = Object.values(present).some(Boolean);

  const download = async (id: string) => {
    setDownloading(id);
    setMsg("");
    try {
      await api.downloadModel(id);
      setPresent((p) => ({ ...p, [id]: true }));
      // Make the freshly downloaded model the active one so "Start AI" uses it.
      if (settings && settings.modelId !== id) {
        await saveSettings({ ...settings, modelId: id });
      }
      setMsg("Model ready. Now click “Start the AI”.");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setDownloading(null);
    }
  };

  const start = async () => {
    setWorking(true);
    setMsg("Starting…");
    try {
      const r = await api.ensureInference();
      setMsg(r);
    } catch (e) {
      setMsg(String(e));
    } finally {
      await refreshLlm();
      setWorking(false);
    }
  };

  return (
    <div className="rounded-xl border border-white/10 bg-white/[0.03] p-4 space-y-4">
      <div className="flex items-center gap-2 text-sm font-medium">
        <Cpu className="size-4 text-indigo-400" />
        Set up the on-device AI
      </div>
      <p className="text-xs text-gray-400 leading-relaxed">
        Pushin runs a small language model <span className="text-gray-200">entirely on your machine</span> — nothing
        leaves your device. Pick a model to download, then start it. Pushin fetches the inference engine for you
        automatically.
      </p>

      <div className="flex items-center gap-2 text-[11px] text-gray-500">
        <ShieldCheck className="size-3.5 text-emerald-400" /> Offline &amp; private · server: {llm?.baseUrl}
      </div>

      <div className="space-y-2">
        {models.map((m) => {
          const p = progress[m.id];
          const pct = p && p.total ? Math.round((p.downloaded / p.total) * 100) : 0;
          const isDownloading = downloading === m.id;
          const isPresent = present[m.id];
          const isActive = settings?.modelId === m.id;
          return (
            <div key={m.id} className={clsx("rounded-lg border p-3", isPresent ? "border-emerald-500/30 bg-emerald-500/5" : "border-white/10")}>
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <div className="text-sm flex items-center gap-2">
                    {m.name}
                    {isPresent && isActive && <span className="text-[10px] px-1.5 py-0.5 rounded bg-emerald-500/20 text-emerald-300">active</span>}
                  </div>
                  <div className="text-[11px] text-gray-500">~{Math.round(m.sizeMb / 10) / 100} GB · {m.note}</div>
                </div>

                {isPresent ? (
                  <div className="shrink-0 flex items-center gap-1.5 text-xs px-2.5 py-1.5 rounded-md bg-emerald-500/15 text-emerald-300">
                    <Check className="size-4" />
                    Downloaded
                  </div>
                ) : (
                  <button
                    disabled={isDownloading}
                    onClick={() => download(m.id)}
                    className="shrink-0 flex items-center gap-1.5 text-xs px-2.5 py-1.5 rounded-md bg-indigo-500/20 text-indigo-200 hover:bg-indigo-500/30 disabled:opacity-50"
                  >
                    {isDownloading ? <Loader2 className="size-3.5 animate-spin" /> : <Download className="size-3.5" />}
                    {isDownloading ? `${pct}%` : "Download"}
                  </button>
                )}
              </div>
              {isDownloading && (
                <div className="mt-2 h-1.5 rounded-full bg-white/10 overflow-hidden">
                  <div className="h-full bg-indigo-400 transition-all" style={{ width: `${pct}%` }} />
                </div>
              )}
            </div>
          );
        })}
      </div>

      <button
        onClick={start}
        disabled={working || !anyPresent}
        title={anyPresent ? "" : "Download a model first"}
        className="w-full flex items-center justify-center gap-2 text-sm py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400 disabled:opacity-50 disabled:hover:bg-indigo-500"
      >
        {working ? <Loader2 className="size-4 animate-spin" /> : <Plug className="size-4" />}
        {working ? "Starting…" : "Start the AI"}
      </button>

      {msg && <p className="text-xs text-gray-400">{msg}</p>}
    </div>
  );
}
