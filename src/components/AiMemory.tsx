import { useEffect, useState } from "react";
import { Brain, Trash2 } from "lucide-react";
import { api, type Memory } from "../lib/ipc";

/** Settings ▸ AI Memory — the durable facts Pushin has learned about the user (from the chat "Remember
 *  this?" chips). Stored privately on-device (not in the vault); listed here so the user can review or
 *  forget them. */
export default function AiMemory() {
  const [memories, setMemories] = useState<Memory[]>([]);
  const [loading, setLoading] = useState(true);

  const load = () => {
    setLoading(true);
    api
      .listMemories()
      .then(setMemories)
      .catch(() => setMemories([]))
      .finally(() => setLoading(false));
  };
  useEffect(load, []);

  const remove = async (id: number) => {
    await api.deleteMemory(id).catch(() => {});
    load();
  };

  return (
    <section className="space-y-4">
      <div>
        <h2 className="text-sm font-semibold flex items-center gap-2">
          <Brain className="size-4 text-indigo-400" /> AI memory
        </h2>
        <p className="mt-1 text-[11px] text-gray-500">
          Durable facts Pushin has learned about you (from the chat "Remember this?" chips). They stay on your device and
          inform planning — they're kept out of your vault. Delete anything you'd rather it forgot.
        </p>
      </div>

      {loading ? (
        <p className="text-xs text-gray-600">Loading…</p>
      ) : memories.length === 0 ? (
        <p className="text-xs text-gray-600">Nothing remembered yet.</p>
      ) : (
        <ul className="space-y-1.5">
          {memories.map((m) => (
            <li key={m.id} className="group flex items-start gap-2 border border-white/10 bg-white/[0.02] px-3 py-2">
              <span className="flex-1 text-sm text-gray-300">{m.content}</span>
              <button
                onClick={() => remove(m.id)}
                title="Forget this"
                className="shrink-0 text-gray-500 opacity-0 transition-opacity hover:text-rose-400 group-hover:opacity-100"
              >
                <Trash2 className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
