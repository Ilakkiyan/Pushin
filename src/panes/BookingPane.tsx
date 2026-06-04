import { useEffect, useMemo, useState } from "react";
import { CalendarClock, Clock, Link2, Plus, Trash2, X } from "lucide-react";
import clsx from "clsx";
import { useStore } from "../state/store";
import { api, type BookingSlot } from "../lib/ipc";
import { parseLocal } from "../lib/time";

export default function BookingPane() {
  const eventTypes = useStore((s) => s.eventTypes);
  const settings = useStore((s) => s.settings)!;
  const load = useStore((s) => s.load);
  const createBooking = useStore((s) => s.createBooking);

  const [selected, setSelected] = useState<number | null>(null);
  const [slots, setSlots] = useState<BookingSlot[]>([]);
  const [loadingSlots, setLoadingSlots] = useState(false);
  const [creating, setCreating] = useState(false);
  const [picked, setPicked] = useState<BookingSlot | null>(null);
  const [newType, setNewType] = useState({ name: "", duration: 30, buffer: 10 });

  useEffect(() => {
    if (selected == null && eventTypes.length) setSelected(eventTypes[0].id);
  }, [eventTypes, selected]);

  const activeType = eventTypes.find((e) => e.id === selected);

  useEffect(() => {
    if (selected == null) return;
    setLoadingSlots(true);
    api
      .bookingSlots(selected, Math.min(settings.horizonDays, 14))
      .then(setSlots)
      .catch(() => setSlots([]))
      .finally(() => setLoadingSlots(false));
  }, [selected, settings.horizonDays]);

  const slotsByDay = useMemo(() => {
    const map = new Map<string, BookingSlot[]>();
    for (const s of slots) {
      const key = parseLocal(s.start).toDateString();
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(s);
    }
    return Array.from(map.entries()).slice(0, 7);
  }, [slots]);

  const addType = async () => {
    if (!newType.name.trim()) return;
    await api.createEventType(newType.name.trim(), newType.duration, newType.buffer, "#0ea5e9");
    setNewType({ name: "", duration: 30, buffer: 10 });
    setCreating(false);
    await load();
  };

  const removeType = async (id: number) => {
    await api.deleteEventType(id);
    if (selected === id) setSelected(null);
    await load();
  };

  return (
    <div className="h-full flex">
      {/* Left: event types */}
      <div className="w-72 shrink-0 border-r border-white/10 flex flex-col">
        <div className="px-4 py-3 border-b border-white/10 flex items-center justify-between">
          <span className="text-sm font-medium">Event types</span>
          <button onClick={() => setCreating((v) => !v)} className="text-gray-400 hover:text-white"><Plus className="size-4" /></button>
        </div>
        {creating && (
          <div className="p-3 border-b border-white/10 space-y-2">
            <input autoFocus value={newType.name} onChange={(e) => setNewType({ ...newType, name: e.target.value })} placeholder="e.g. Intro call" className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50" />
            <div className="flex items-center gap-2 text-xs">
              <input type="number" min={15} step={15} value={newType.duration} onChange={(e) => setNewType({ ...newType, duration: Number(e.target.value) })} className="w-16 rounded-md bg-white/5 border border-white/10 px-2 py-1.5 outline-none" />
              <span className="text-gray-500">min, buffer</span>
              <input type="number" min={0} step={5} value={newType.buffer} onChange={(e) => setNewType({ ...newType, buffer: Number(e.target.value) })} className="w-14 rounded-md bg-white/5 border border-white/10 px-2 py-1.5 outline-none" />
              <button onClick={addType} className="ml-auto px-3 py-1.5 rounded-md bg-indigo-500 hover:bg-indigo-400">Add</button>
            </div>
          </div>
        )}
        <div className="flex-1 overflow-y-auto p-2 space-y-1">
          {eventTypes.map((et) => (
            <div
              key={et.id}
              onClick={() => setSelected(et.id)}
              className={clsx("group flex items-center gap-2 px-3 py-2 rounded-lg cursor-pointer", selected === et.id ? "bg-white/10" : "hover:bg-white/5")}
            >
              <span className="size-2 rounded-full" style={{ background: et.color }} />
              <div className="min-w-0 flex-1">
                <div className="text-sm truncate">{et.name}</div>
                <div className="text-[11px] text-gray-500">{et.durationMinutes} min · {et.bufferMinutes}m buffer</div>
              </div>
              <button onClick={(e) => { e.stopPropagation(); removeType(et.id); }} className="opacity-0 group-hover:opacity-100 text-gray-500 hover:text-rose-400">
                <Trash2 className="size-3.5" />
              </button>
            </div>
          ))}
        </div>
      </div>

      {/* Right: public-page mockup */}
      <div className="flex-1 min-w-0 overflow-y-auto">
        <div className="max-w-3xl mx-auto p-6 space-y-5">
          <div className="rounded-xl border border-dashed border-white/15 p-4 flex items-center gap-3">
            <Link2 className="size-4 text-sky-400 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="text-sm">Your booking link</div>
              <div className="text-xs text-gray-500 truncate">pushin.app/you/{activeType?.name.toLowerCase().replace(/\s+/g, "-") ?? "event"}</div>
            </div>
            <button className="text-xs px-3 py-1.5 rounded-md bg-white/10 text-gray-300 cursor-not-allowed" title="Public hosting needs a relay — planned">Publish (coming soon)</button>
          </div>

          <div>
            <h2 className="text-base font-semibold flex items-center gap-2">
              <CalendarClock className="size-4 text-sky-400" />
              {activeType ? activeType.name : "Pick an event type"}
            </h2>
            <p className="text-xs text-gray-500 mt-1">
              Invitee preview — these are real open slots from your calendar (computed by the same scheduler engine).
            </p>
          </div>

          {loadingSlots && <p className="text-sm text-gray-500">Finding open times…</p>}
          {!loadingSlots && activeType && slotsByDay.length === 0 && (
            <p className="text-sm text-gray-500">No open slots in range. Free up some time or extend your planning horizon.</p>
          )}

          <div className="grid grid-cols-2 md:grid-cols-3 gap-4">
            {slotsByDay.map(([day, daySlots]) => (
              <div key={day} className="space-y-2">
                <div className="text-xs font-medium text-gray-300">{new Date(day).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })}</div>
                <div className="space-y-1.5">
                  {daySlots.slice(0, 8).map((s) => (
                    <button
                      key={s.start}
                      onClick={() => setPicked(s)}
                      className="w-full text-sm px-3 py-1.5 rounded-md border border-sky-400/30 text-sky-200 hover:bg-sky-400/10"
                    >
                      {parseLocal(s.start).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}
                    </button>
                  ))}
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>

      {picked && activeType && (
        <BookingModal
          slot={picked}
          typeName={activeType.name}
          onClose={() => setPicked(null)}
          onConfirm={async (name, email) => {
            await createBooking(activeType.id, name, email, picked.start, picked.end);
            setPicked(null);
            if (selected != null) api.bookingSlots(selected, Math.min(settings.horizonDays, 14)).then(setSlots);
          }}
        />
      )}
    </div>
  );
}

function BookingModal({ slot, typeName, onClose, onConfirm }: { slot: BookingSlot; typeName: string; onClose: () => void; onConfirm: (name: string, email: string) => void }) {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50" onClick={onClose}>
      <div className="w-80 rounded-xl border border-white/10 bg-[#12151c] p-4 space-y-3" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-medium">Book: {typeName}</h3>
          <button onClick={onClose} className="text-gray-500 hover:text-white"><X className="size-4" /></button>
        </div>
        <p className="text-xs text-gray-400 flex items-center gap-1.5">
          <Clock className="size-3.5" />
          {parseLocal(slot.start).toLocaleString([], { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}
        </p>
        <input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="Invitee name" className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50" />
        <input value={email} onChange={(e) => setEmail(e.target.value)} placeholder="Invitee email" className="w-full rounded-md bg-white/5 border border-white/10 px-2 py-1.5 text-sm outline-none focus:border-indigo-500/50" />
        <button
          disabled={!name.trim()}
          onClick={() => onConfirm(name.trim(), email.trim())}
          className="w-full text-sm py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400 disabled:opacity-40"
        >
          Confirm booking
        </button>
        <p className="text-[11px] text-gray-500 text-center">Adds a fixed event and re-plans your tasks around it.</p>
      </div>
    </div>
  );
}
