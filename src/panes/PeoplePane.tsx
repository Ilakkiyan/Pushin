import { useEffect, useMemo, useState } from "react";
import { UserPlus, Trash2, CalendarClock } from "lucide-react";
import clsx from "clsx";
import { api, type Person, type Booking } from "../lib/ipc";
import { useStore } from "../state/store";
import { parseLocal } from "../lib/time";
import LabelPicker from "../components/LabelPicker";

/** The relationship layer (private CRM): people are auto-created from booking invitees and editable
 *  here, with their labels and meeting history. */
export default function PeoplePane() {
  const [people, setPeople] = useState<Person[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const bookings = useStore((s) => s.bookings);

  const load = () => api.listPeople().then(setPeople).catch(() => setPeople([]));
  useEffect(() => { load(); }, []);

  const selected = people.find((p) => p.id === selectedId) ?? null;

  const createNew = async () => {
    const p = await api.createPerson("New person", null, "");
    await load();
    setSelectedId(p.id);
  };

  return (
    <div className="flex h-full text-gray-100">
      <div className="w-72 shrink-0 border-r border-white/10 flex flex-col">
        <div className="flex items-center justify-between px-4 py-3 border-b border-white/10">
          <h2 className="text-sm font-semibold">People</h2>
          <button onClick={createNew} title="Add person" className="grid size-7 place-items-center rounded-md text-gray-400 hover:bg-white/10 hover:text-white">
            <UserPlus className="size-4" />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto">
          {people.length === 0 ? (
            <p className="p-4 text-xs leading-relaxed text-gray-500">No people yet. They appear automatically when someone books a time with you — or add one with +.</p>
          ) : (
            people.map((p) => (
              <button
                key={p.id}
                onClick={() => setSelectedId(p.id)}
                className={clsx("w-full text-left px-4 py-2.5 border-b border-white/5 hover:bg-white/5", selectedId === p.id && "bg-white/10")}
              >
                <div className="truncate text-sm">{p.name || "Unnamed"}</div>
                {p.email && <div className="truncate text-xs text-gray-500">{p.email}</div>}
              </button>
            ))
          )}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        {selected ? (
          <PersonDetail
            key={selected.id}
            person={selected}
            bookings={bookings}
            onSaved={load}
            onDeleted={() => { setSelectedId(null); load(); }}
          />
        ) : (
          <div className="flex h-full items-center justify-center px-6 text-center text-sm text-gray-500">Select a person to see their details.</div>
        )}
      </div>
    </div>
  );
}

function PersonDetail({ person, bookings, onSaved, onDeleted }: { person: Person; bookings: Booking[]; onSaved: () => void; onDeleted: () => void }) {
  const [name, setName] = useState(person.name);
  const [email, setEmail] = useState(person.email ?? "");
  const [notes, setNotes] = useState(person.notes);

  const save = async () => {
    await api.updatePerson(person.id, name.trim() || "Unnamed", email.trim() || null, notes);
    onSaved();
  };

  const meetings = useMemo(
    () =>
      person.email
        ? bookings
            .filter((b) => b.inviteeEmail.toLowerCase() === person.email!.toLowerCase())
            .sort((a, b) => b.start.localeCompare(a.start))
        : [],
    [bookings, person.email],
  );

  return (
    <div className="max-w-2xl mx-auto px-6 py-6 space-y-5">
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={save}
        className="w-full bg-transparent text-2xl font-semibold outline-none border-b border-transparent focus:border-white/20"
      />

      <div className="space-y-1">
        <label className="text-xs text-gray-400">Email</label>
        <input
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          onBlur={save}
          placeholder="name@example.com"
          className="w-full rounded-md bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-indigo-500/50"
        />
      </div>

      <div className="space-y-1">
        <label className="text-xs text-gray-400">Labels</label>
        <LabelPicker kind="person" entityId={person.id} />
      </div>

      <div className="space-y-1">
        <label className="text-xs text-gray-400">Notes</label>
        <textarea
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          onBlur={save}
          rows={5}
          placeholder="What you want to remember about them…"
          className="w-full rounded-md bg-white/5 border border-white/10 px-3 py-2 text-sm outline-none focus:border-indigo-500/50 resize-y"
        />
      </div>

      <div className="space-y-1">
        <label className="text-xs text-gray-400">Meetings ({meetings.length})</label>
        {meetings.length === 0 ? (
          <p className="text-xs text-gray-500">No booked meetings with this person yet.</p>
        ) : (
          <ul className="space-y-1">
            {meetings.map((m) => (
              <li key={m.id} className="flex items-center gap-2 text-sm text-gray-300">
                <CalendarClock className="size-3.5 text-gray-500 shrink-0" />
                <span>{parseLocal(m.start).toLocaleString([], { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })}</span>
                {m.status === "cancelled" && <span className="text-xs text-rose-400">cancelled</span>}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="pt-2">
        <button
          onClick={() => api.deletePerson(person.id).then(onDeleted)}
          className="inline-flex items-center gap-1.5 text-xs px-3 py-1.5 rounded-md border border-rose-500/40 text-rose-300 hover:bg-rose-500/10"
        >
          <Trash2 className="size-3.5" /> Delete person
        </button>
      </div>
    </div>
  );
}
