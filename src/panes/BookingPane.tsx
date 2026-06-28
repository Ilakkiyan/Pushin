import { useEffect, useMemo, useState } from "react";
import {
  CalendarClock,
  Check,
  Copy,
  ExternalLink,
  Link2,
  Play,
  Plus,
  RefreshCw,
  Square,
  Trash2,
  X,
} from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type BookingServerStatus, type BookingSlot } from "../lib/ipc";
import { parseLocal } from "../lib/time";

type EventTypeDraft = {
  name: string;
  durationMinutes: number;
  bufferMinutes: number;
  color: string;
  enabled: boolean;
};

const DEFAULT_DRAFT: EventTypeDraft = {
  name: "",
  durationMinutes: 30,
  bufferMinutes: 10,
  color: "#0ea5e9",
  enabled: true,
};

export default function BookingPane() {
  const eventTypes = useStore((s) => s.eventTypes);
  const bookings = useStore((s) => s.bookings);
  const settings = useStore((s) => s.settings)!;
  const load = useStore((s) => s.load);
  const cancelBooking = useStore((s) => s.cancelBooking);
  const createBooking = useStore((s) => s.createBooking);

  const [selected, setSelected] = useState<number | null>(null);
  const [slots, setSlots] = useState<BookingSlot[]>([]);
  const [loadingSlots, setLoadingSlots] = useState(false);
  const [draft, setDraft] = useState<EventTypeDraft>(DEFAULT_DRAFT);
  const [creating, setCreating] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<BookingServerStatus | null>(null);
  const [serverBusy, setServerBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [externalUrl, setExternalUrl] = useState("");
  const [picked, setPicked] = useState<BookingSlot | null>(null);

  useEffect(() => {
    api.bookingServerStatus().then(setStatus).catch(() => setStatus({ running: false, localUrl: null, host: "127.0.0.1", port: null }));
  }, []);

  useEffect(() => {
    if (selected == null && eventTypes.length) setSelected(eventTypes[0].id);
  }, [eventTypes, selected]);

  const activeType = eventTypes.find((e) => e.id === selected) ?? null;

  useEffect(() => {
    if (creating) {
      setDraft(DEFAULT_DRAFT);
      return;
    }
    if (activeType) {
      setDraft({
        name: activeType.name,
        durationMinutes: activeType.durationMinutes,
        bufferMinutes: activeType.bufferMinutes,
        color: activeType.color,
        enabled: activeType.enabled,
      });
    }
  }, [activeType, creating]);

  useEffect(() => {
    if (!activeType) {
      setSlots([]);
      return;
    }
    setLoadingSlots(true);
    api
      .bookingSlots(activeType.id, Math.min(settings.horizonDays, 14))
      .then(setSlots)
      .catch(() => setSlots([]))
      .finally(() => setLoadingSlots(false));
  }, [activeType?.id, settings.horizonDays]);

  const slotsByDay = useMemo(() => {
    const map = new Map<string, BookingSlot[]>();
    for (const slot of slots) {
      const key = parseLocal(slot.start).toDateString();
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(slot);
    }
    return Array.from(map.entries()).slice(0, 7);
  }, [slots]);

  const eventTypeById = useMemo(() => new Map(eventTypes.map((et) => [et.id, et])), [eventTypes]);
  const confirmedBookings = bookings.filter((b) => b.status === "confirmed");
  const localLink = activeType && status?.localUrl ? `${status.localUrl}/b/${activeType.shareToken}/${activeType.slug}` : "";
  const trimmedExternalUrl = externalUrl.trim();
  const tunnelLink =
    activeType && trimmedExternalUrl
      ? trimmedExternalUrl.includes("/b/")
        ? trimmedExternalUrl
        : `${trimmedExternalUrl.replace(/\/$/, "")}/b/${activeType.shareToken}/${activeType.slug}`
      : "";
  const publicLink = tunnelLink || localLink;

  const copy = async (text: string, key: string) => {
    if (!text) return;
    await navigator.clipboard?.writeText(text);
    setCopied(key);
    window.setTimeout(() => setCopied((cur) => (cur === key ? null : cur)), 1400);
  };

  const startServer = async () => {
    setServerBusy(true);
    try {
      setStatus(await api.startBookingServer());
    } finally {
      setServerBusy(false);
    }
  };

  const stopServer = async () => {
    setServerBusy(true);
    try {
      setStatus(await api.stopBookingServer());
    } finally {
      setServerBusy(false);
    }
  };

  const saveType = async () => {
    if (!draft.name.trim()) return;
    setSaving(true);
    try {
      if (creating) {
        const id = await api.createEventType(draft.name.trim(), draft.durationMinutes, draft.bufferMinutes, draft.color);
        setCreating(false);
        setSelected(id);
      } else if (activeType) {
        await api.updateEventType(activeType.id, draft.name.trim(), draft.durationMinutes, draft.bufferMinutes, draft.color, draft.enabled);
      }
      await load();
    } finally {
      setSaving(false);
    }
  };

  const removeType = async (id: number) => {
    await api.deleteEventType(id);
    if (selected === id) setSelected(null);
    await load();
  };

  const regenerateLink = async () => {
    if (!activeType) return;
    await api.regenerateEventTypeToken(activeType.id);
    await load();
  };

  return (
    <div className="h-full w-full overflow-hidden flex bg-[var(--bg)]">
      <aside className="w-72 shrink-0 border-r border-white/10 flex flex-col bg-[var(--surface)]">
        <div className="px-4 py-3 border-b border-white/10 flex items-center justify-between">
          <div>
            <div className="text-sm font-medium">Booking</div>
            <div className="text-[11px] text-gray-500">Event types and links</div>
          </div>
          <button
            onClick={() => {
              setCreating(true);
              setSelected(null);
            }}
            className="grid size-8 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white"
            title="New event type"
          >
            <Plus className="size-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-2 space-y-1">
          {eventTypes.map((et) => (
            <div
              key={et.id}
              role="button"
              tabIndex={0}
              onClick={() => {
                setCreating(false);
                setSelected(et.id);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  setCreating(false);
                  setSelected(et.id);
                }
              }}
              className={clsx(
                "group w-full flex items-center gap-2 px-3 py-2 rounded-md text-left",
                activeType?.id === et.id && !creating ? "bg-white/10" : "hover:bg-white/5",
              )}
            >
              <span className="size-2 rounded-full" style={{ background: et.color }} />
              <span className="min-w-0 flex-1">
                <span className="block text-sm truncate">{et.name}</span>
                <span className="block text-[11px] text-gray-500">
                  {et.durationMinutes} min · {et.bufferMinutes}m buffer · {et.enabled ? "active" : "off"}
                </span>
              </span>
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  removeType(et.id);
                }}
                className="opacity-0 group-hover:opacity-100 grid size-7 place-items-center rounded-md text-gray-500 hover:bg-rose-500/10 hover:text-rose-300"
                title="Delete event type"
              >
                  <Trash2 className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      </aside>

      <main className="flex-1 min-w-0 overflow-y-auto">
        <div className="max-w-6xl mx-auto p-5 space-y-4">
          <section className="grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_360px] gap-4">
            <div className="space-y-4">
              <Panel>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold flex items-center gap-2">
                      <CalendarClock className="size-4 text-sky-400" />
                      {creating ? "New event type" : activeType?.name ?? "Pick an event type"}
                    </h2>
                    <p className="text-xs text-gray-500 mt-1">
                      Shareable pages use these settings and your live Pushin availability.
                    </p>
                  </div>
                  {!creating && activeType && (
                    <label className="flex items-center gap-2 text-xs text-gray-400">
                      <input
                        type="checkbox"
                        checked={draft.enabled}
                        onChange={(e) => setDraft({ ...draft, enabled: e.target.checked })}
                      />
                      Active
                    </label>
                  )}
                </div>

                <div className="grid grid-cols-1 md:grid-cols-[minmax(0,1fr)_120px_120px_120px] gap-3 mt-4">
                  <Field label="Name">
                    <input
                      value={draft.name}
                      onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                      placeholder="Intro call"
                      className={inputCls}
                    />
                  </Field>
                  <Field label="Duration">
                    <input
                      type="number"
                      min={15}
                      step={15}
                      value={draft.durationMinutes}
                      onChange={(e) => setDraft({ ...draft, durationMinutes: Number(e.target.value) })}
                      className={inputCls}
                    />
                  </Field>
                  <Field label="Buffer">
                    <input
                      type="number"
                      min={0}
                      step={5}
                      value={draft.bufferMinutes}
                      onChange={(e) => setDraft({ ...draft, bufferMinutes: Number(e.target.value) })}
                      className={inputCls}
                    />
                  </Field>
                  <Field label="Color">
                    <div className="flex items-center gap-2">
                      <input
                        type="color"
                        value={draft.color}
                        onChange={(e) => setDraft({ ...draft, color: e.target.value })}
                        className="size-9 rounded-md border border-white/10 bg-transparent"
                      />
                      <input value={draft.color} onChange={(e) => setDraft({ ...draft, color: e.target.value })} className={inputCls} />
                    </div>
                  </Field>
                </div>

                <div className="flex items-center gap-2 mt-4">
                  <button onClick={saveType} disabled={saving || !draft.name.trim()} className={primaryBtn}>
                    {saving ? <RefreshCw className="size-4 animate-spin" /> : <Check className="size-4" />}
                    {creating ? "Create" : "Save"}
                  </button>
                  {creating && (
                    <button onClick={() => setCreating(false)} className={ghostBtn}>
                      <X className="size-4" />
                      Cancel
                    </button>
                  )}
                </div>
              </Panel>

              <Panel>
                <div className="flex items-start justify-between gap-4">
                  <div>
                    <h2 className="text-base font-semibold">Availability preview</h2>
                    <p className="text-xs text-gray-500 mt-1">These slots come from the same scheduler used by the public page.</p>
                  </div>
                  {loadingSlots && <RefreshCw className="size-4 animate-spin text-gray-500" />}
                </div>
                {!activeType && !creating && <p className="mt-4 text-sm text-gray-500">Create or select an event type to see open times.</p>}
                {activeType && !loadingSlots && slotsByDay.length === 0 && (
                  <p className="mt-4 text-sm text-gray-500">No open slots in range. Free up some time or extend your planning horizon.</p>
                )}
                <div className="grid grid-cols-2 md:grid-cols-4 gap-3 mt-4">
                  {slotsByDay.map(([day, daySlots]) => (
                    <div key={day} className="space-y-2">
                      <div className="text-xs font-medium text-gray-300">
                        {new Date(day).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })}
                      </div>
                      {daySlots.slice(0, 8).map((slot) => (
                        <button
                          key={slot.start}
                          onClick={() => setPicked(slot)}
                          className="w-full text-sm px-3 py-1.5 rounded-md border border-sky-400/30 text-sky-200 hover:bg-sky-400/10"
                        >
                          {parseLocal(slot.start).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}
                        </button>
                      ))}
                    </div>
                  ))}
                </div>
              </Panel>

              <Panel>
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <h2 className="text-base font-semibold">Bookings</h2>
                    <p className="text-xs text-gray-500 mt-1">Confirmed bookings reserve fixed events on your calendar.</p>
                  </div>
                  <span className="text-xs text-gray-500">{confirmedBookings.length} confirmed</span>
                </div>
                <div className="mt-3 divide-y divide-white/10">
                  {bookings.length === 0 && <p className="py-4 text-sm text-gray-500">No bookings yet.</p>}
                  {bookings.map((booking) => {
                    const et = eventTypeById.get(booking.eventTypeId);
                    const cancelled = booking.status !== "confirmed";
                    return (
                      <div key={booking.id} className="py-3 flex items-center gap-3">
                        <span className="size-2 rounded-full" style={{ background: et?.color ?? "#64748b" }} />
                        <div className="min-w-0 flex-1">
                          <div className={clsx("text-sm truncate", cancelled && "line-through text-gray-500")}>{booking.inviteeName}</div>
                          <div className="text-[11px] text-gray-500 truncate">
                            {et?.name ?? "Deleted event type"} · {parseLocal(booking.start).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })} · {booking.inviteeEmail}
                          </div>
                        </div>
                        <span className={clsx("text-[11px] px-2 py-1 rounded-md", cancelled ? "bg-white/5 text-gray-500" : "bg-emerald-500/10 text-emerald-300")}>
                          {booking.status}
                        </span>
                        {!cancelled && (
                          <button
                            onClick={() => cancelBooking(booking.id)}
                            className="grid size-8 place-items-center rounded-md text-gray-500 hover:bg-rose-500/10 hover:text-rose-300"
                            title="Cancel booking"
                          >
                            <X className="size-4" />
                          </button>
                        )}
                      </div>
                    );
                  })}
                </div>
              </Panel>
            </div>

            <div className="space-y-4">
              <Panel>
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h2 className="text-base font-semibold">Local server</h2>
                    <p className="text-xs text-gray-500 mt-1">
                      Start this before sharing a booking page through a tunnel.
                    </p>
                  </div>
                  <span className={clsx("text-[11px] px-2 py-1 rounded-md", status?.running ? "bg-emerald-500/10 text-emerald-300" : "bg-white/5 text-gray-500")}>
                    {status?.running ? "Running" : "Stopped"}
                  </span>
                </div>
                <div className="mt-4 flex items-center gap-2">
                  {status?.running ? (
                    <button onClick={stopServer} disabled={serverBusy} className={dangerBtn}>
                      <Square className="size-4" />
                      Stop
                    </button>
                  ) : (
                    <button onClick={startServer} disabled={serverBusy} className={primaryBtn}>
                      <Play className="size-4" />
                      Start
                    </button>
                  )}
                  <button onClick={() => api.bookingServerStatus().then(setStatus)} className={ghostBtn}>
                    <RefreshCw className="size-4" />
                    Refresh
                  </button>
                </div>
                <LinkRow label="Local URL" value={status?.localUrl ?? "Start the server to create a local URL."} onCopy={() => copy(status?.localUrl ?? "", "server")} copied={copied === "server"} />
              </Panel>

              <Panel>
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <h2 className="text-base font-semibold">Share link</h2>
                    <p className="text-xs text-gray-500 mt-1">Use the local URL directly or paste your tunnel URL below.</p>
                  </div>
                  {activeType && (
                    <button onClick={regenerateLink} className="grid size-8 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white" title="Regenerate private link token">
                      <RefreshCw className="size-4" />
                    </button>
                  )}
                </div>
                <LinkRow label="Booking page" value={localLink || "Select an event type and start the server."} onCopy={() => copy(localLink, "local-link")} copied={copied === "local-link"} />
                <Field label="External tunnel URL">
                  <input
                    value={externalUrl}
                    onChange={(e) => setExternalUrl(e.target.value)}
                    placeholder="https://your-tunnel.ngrok-free.app"
                    className={inputCls}
                  />
                </Field>
                <LinkRow
                  label="Public URL to send"
                  value={activeType && publicLink ? publicLink : "Paste a tunnel URL or start the local server."}
                  onCopy={() => {
                    copy(publicLink, "public-link");
                  }}
                  copied={copied === "public-link"}
                />
              </Panel>

              <Panel>
                <h2 className="text-base font-semibold">Tunnel commands</h2>
                <p className="text-xs text-gray-500 mt-1">Run one of these while the server is running, then paste the tunnel base URL above.</p>
                <CodeCopy value={status?.port ? `ngrok http ${status.port}` : "ngrok http 47610"} onCopy={() => copy(status?.port ? `ngrok http ${status.port}` : "ngrok http 47610", "ngrok")} copied={copied === "ngrok"} />
                <CodeCopy value={status?.port ? `cloudflared tunnel --url http://127.0.0.1:${status.port}` : "cloudflared tunnel --url http://127.0.0.1:47610"} onCopy={() => copy(status?.port ? `cloudflared tunnel --url http://127.0.0.1:${status.port}` : "cloudflared tunnel --url http://127.0.0.1:47610", "cloudflared")} copied={copied === "cloudflared"} />
              </Panel>
            </div>
          </section>
        </div>
      </main>

      {picked && activeType && (
        <BookingModal
          slot={picked}
          typeName={activeType.name}
          onClose={() => setPicked(null)}
          onConfirm={async (name, email) => {
            await createBooking(activeType.id, name, email, picked.start, picked.end);
            setPicked(null);
            api.bookingSlots(activeType.id, Math.min(settings.horizonDays, 14)).then(setSlots);
          }}
        />
      )}
    </div>
  );
}

function Panel({ children }: { children: React.ReactNode }) {
  return <section className="rounded-lg border border-white/10 bg-[var(--surface)] p-4">{children}</section>;
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block text-xs text-gray-500 space-y-1.5">
      <span>{label}</span>
      {children}
    </label>
  );
}

function LinkRow({ label, value, onCopy, copied }: { label: string; value: string; onCopy: () => void; copied: boolean }) {
  return (
    <div className="mt-3">
      <div className="text-xs text-gray-500 mb-1">{label}</div>
      <div className="flex items-center gap-2 rounded-md border border-white/10 bg-black/20 px-2 py-2">
        <Link2 className="size-4 text-gray-500 shrink-0" />
        <span className="min-w-0 flex-1 truncate text-xs text-gray-300">{value}</span>
        <button onClick={onCopy} className="grid size-7 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white" title="Copy">
          {copied ? <Check className="size-4 text-emerald-300" /> : <Copy className="size-4" />}
        </button>
        {value.startsWith("http") && (
          <a href={value} target="_blank" rel="noreferrer" className="grid size-7 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white" title="Open">
            <ExternalLink className="size-4" />
          </a>
        )}
      </div>
    </div>
  );
}

function CodeCopy({ value, onCopy, copied }: { value: string; onCopy: () => void; copied: boolean }) {
  return (
    <div className="mt-3 flex items-center gap-2 rounded-md border border-white/10 bg-black/20 px-2 py-2">
      <code className="min-w-0 flex-1 truncate text-xs text-gray-300">{value}</code>
      <button onClick={onCopy} className="grid size-7 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white" title="Copy command">
        {copied ? <Check className="size-4 text-emerald-300" /> : <Copy className="size-4" />}
      </button>
    </div>
  );
}

function BookingModal({
  slot,
  typeName,
  onClose,
  onConfirm,
}: {
  slot: BookingSlot;
  typeName: string;
  onClose: () => void;
  onConfirm: (name: string, email: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60 p-4" onClick={onClose}>
      <div className="w-full max-w-sm rounded-lg border border-white/10 bg-[var(--raised)] p-4 space-y-3" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium">Preview booking: {typeName}</h3>
          <button onClick={onClose} className="grid size-8 place-items-center rounded-md text-gray-500 hover:bg-white/10 hover:text-white">
            <X className="size-4" />
          </button>
        </div>
        <p className="text-xs text-gray-400">
          {parseLocal(slot.start).toLocaleString([], { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}
        </p>
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Invitee name" className={inputCls} />
        <input value={email} onChange={(e) => setEmail(e.target.value)} placeholder="Invitee email" className={inputCls} />
        <button
          disabled={busy || !name.trim() || !email.trim()}
          onClick={async () => {
            setBusy(true);
            try {
              await onConfirm(name, email);
            } finally {
              setBusy(false);
            }
          }}
          className="w-full inline-flex items-center justify-center gap-2 text-sm py-2 rounded-lg bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40"
        >
          {busy ? <RefreshCw className="size-4 animate-spin" /> : <Check className="size-4" />}
          Confirm booking
        </button>
      </div>
    </div>
  );
}

const inputCls = "w-full rounded-md bg-white/5 border border-white/10 px-2 py-2 text-sm outline-none focus:border-indigo-500/50";
const primaryBtn = "inline-flex items-center gap-2 px-3 py-2 rounded-md bg-white/90 hover:bg-white text-gray-900 disabled:opacity-40 text-sm";
const ghostBtn = "inline-flex items-center gap-2 px-3 py-2 rounded-md bg-white/10 hover:bg-white/15 text-sm text-gray-300";
const dangerBtn = "inline-flex items-center gap-2 px-3 py-2 rounded-md bg-rose-500/15 hover:bg-rose-500/25 text-sm text-rose-200";
