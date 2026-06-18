import { useEffect, useState } from "react";
import { Laptop, Copy, Check, RefreshCw, Trash2, Wifi, WifiOff } from "lucide-react";
import { api, type SyncStatus } from "../lib/ipc";

const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50";
const btn = "rounded-md bg-white/5 border border-white/10 px-3 py-1.5 text-sm hover:bg-white/10 disabled:opacity-50";
const btnPrimary = "rounded-md bg-indigo-600 hover:bg-indigo-500 px-3 py-1.5 text-sm disabled:opacity-50";

function shortId(id: string) {
  return id.length > 14 ? `${id.slice(0, 8)}…${id.slice(-4)}` : id;
}

/**
 * Settings ▸ Devices: pair this install with your other devices over a private peer-to-peer mesh
 * (Iroh) and keep them in sync without any cloud. "Add a device" mints an invite code; paste it into
 * "Join" on another device. Data flows device→device, end-to-end encrypted.
 */
export default function DevicesSync() {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [invite, setInvite] = useState("");
  const [joinText, setJoinText] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [copied, setCopied] = useState(false);
  const [nameDraft, setNameDraft] = useState("");

  const refresh = async () => {
    try {
      const s = await api.syncStatus();
      setStatus(s);
      setNameDraft(s.deviceName);
    } catch (e) {
      setMsg(String(e));
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const run = async (fn: () => Promise<unknown>, working: string) => {
    setBusy(true);
    setMsg(working);
    try {
      await fn();
      setMsg("");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  };

  const createInvite = async () => {
    setBusy(true);
    setMsg("Creating an invite…");
    try {
      const t = await api.syncCreateInvite();
      setInvite(t);
      setMsg("");
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  };

  const join = () =>
    run(async () => {
      await api.syncJoin(joinText.trim());
      setJoinText("");
    }, "Joining the network…");

  const copyInvite = () => {
    navigator.clipboard.writeText(invite).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  const saveName = () => {
    if (nameDraft.trim() && nameDraft !== status?.deviceName) {
      run(() => api.syncSetDeviceName(nameDraft.trim()), "Saving device name…");
    }
  };

  const enabled = status?.enabled ?? false;

  return (
    <section className="space-y-4">
      <h2 className="text-sm font-semibold flex items-center gap-2">
        <Laptop className="size-4 text-indigo-400" /> Devices &amp; sync
      </h2>
      <p className="text-xs text-gray-400 -mt-2">
        Keep Pushin in sync across your devices over a private peer-to-peer network — no cloud, no
        account. Data flows device&nbsp;to&nbsp;device, end-to-end encrypted, joined by a shared key.
      </p>

      {/* This device */}
      <div className="rounded-lg border border-white/10 p-3 space-y-2">
        <div className="flex items-center justify-between gap-2">
          <div className="flex-1 flex items-center gap-2">
            <input
              className={inputCls}
              value={nameDraft}
              onChange={(e) => setNameDraft(e.target.value)}
              onBlur={saveName}
              placeholder="This device's name"
            />
          </div>
          <span className="text-[11px] text-gray-500 shrink-0">
            {status?.running ? <Wifi className="size-4 inline text-emerald-400" /> : <WifiOff className="size-4 inline text-gray-500" />}
            {status?.nodeId ? ` ${shortId(status.nodeId)}` : " offline"}
          </span>
        </div>
        {enabled && (
          <label className="flex items-center gap-2 text-xs text-gray-400">
            <input
              type="checkbox"
              checked={status?.useRelay ?? true}
              onChange={(e) => run(() => api.syncSetRelay(e.target.checked), "Updating network mode…")}
              disabled={busy}
            />
            Use relays for connectivity (off = LAN/direct-only, maximum privacy but may not reach across networks)
          </label>
        )}
      </div>

      {/* Pairing */}
      <div className="grid sm:grid-cols-2 gap-3">
        {/* Add a device */}
        <div className="rounded-lg border border-white/10 p-3 space-y-2">
          <div className="text-xs font-medium text-gray-300">Add a device</div>
          <button className={btnPrimary} onClick={createInvite} disabled={busy}>
            Create invite code
          </button>
          {invite && (
            <div className="space-y-1">
              <textarea readOnly value={invite} className={`${inputCls} h-20 font-mono text-[10px] break-all`} />
              <button className={btn} onClick={copyInvite}>
                {copied ? <Check className="size-3.5 inline" /> : <Copy className="size-3.5 inline" />} Copy
              </button>
              <p className="text-[11px] text-gray-500">Paste this into “Join a network” on your other device.</p>
            </div>
          )}
        </div>

        {/* Join a network */}
        <div className="rounded-lg border border-white/10 p-3 space-y-2">
          <div className="text-xs font-medium text-gray-300">Join a network</div>
          <textarea
            value={joinText}
            onChange={(e) => setJoinText(e.target.value)}
            placeholder="Paste an invite code from another device"
            className={`${inputCls} h-20 font-mono text-[10px] break-all`}
          />
          <button className={btnPrimary} onClick={join} disabled={busy || !joinText.trim()}>
            Join
          </button>
        </div>
      </div>

      {/* Peers */}
      {enabled && (
        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <div className="text-xs font-medium text-gray-300">Paired devices</div>
            <button className={btn} onClick={() => run(() => api.syncNow(), "Syncing…")} disabled={busy}>
              <RefreshCw className="size-3.5 inline" /> Sync now
            </button>
          </div>
          {status && status.peers.length === 0 && (
            <p className="text-[11px] text-gray-500">No other devices yet. Create an invite and join from another device.</p>
          )}
          <ul className="space-y-1">
            {status?.peers.map((p) => (
              <li key={p.nodeId} className="flex items-center justify-between rounded-md bg-white/5 px-2 py-1.5 text-sm">
                <div className="min-w-0">
                  <div className="truncate">{p.name || "Unnamed device"}</div>
                  <div className="text-[10px] text-gray-500 font-mono">{shortId(p.nodeId)}{p.lastSeen ? ` · seen ${p.lastSeen.replace("T", " ")}` : ""}</div>
                </div>
                <button
                  className="text-gray-400 hover:text-red-400 shrink-0"
                  title="Remove device"
                  onClick={() => run(() => api.syncRemovePeer(p.nodeId), "Removing device…")}
                >
                  <Trash2 className="size-4" />
                </button>
              </li>
            ))}
          </ul>
          <button
            className="text-[11px] text-gray-500 hover:text-red-400"
            onClick={() => run(() => api.syncLeave(), "Leaving the network…")}
            disabled={busy}
          >
            Leave this sync network
          </button>
        </div>
      )}

      {msg && <p className="text-xs text-amber-400">{msg}</p>}
    </section>
  );
}
