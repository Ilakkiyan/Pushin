import { create } from "zustand";
import {
  api,
  type AppData,
  type Block,
  type Booking,
  type CalEvent,
  type Conflict,
  type EventType,
  type HabitStats,
  type LlmStatus,
  type PlanOutcome,
  type Project,
  type ScheduleResult,
  type Settings,
  type SyncSummary,
  type Task,
} from "../lib/ipc";

type View = "calendar" | "projects" | "habits" | "booking" | "settings";
type CalMode = "week" | "month";

// One turn in the chat transcript. Kept in the store (not ChatPane's local state) so the
// conversation survives navigating away from the calendar; it lives only for the session and
// is gone when the app closes.
export type ChatMsg = { role: "user" | "ai"; text: string };

interface State {
  loaded: boolean;
  busy: boolean;
  view: View;
  settings: Settings | null;
  projects: Project[];
  tasks: Task[];
  events: CalEvent[];
  blocks: Block[];
  eventTypes: EventType[];
  bookings: Booking[];
  conflicts: Conflict[];
  llm: LlmStatus | null;

  // Calendar view mode (week vs month), independent of which page you're on.
  calMode: CalMode;
  // When the month view hands off to the week view, the day to open to.
  focusDateIso: string | null;
  habits: HabitStats[];

  // Chat transcript, persisted for the session so it isn't lost on page/settings changes.
  chatMessages: ChatMsg[];
  setChatMessages: (m: ChatMsg[] | ((prev: ChatMsg[]) => ChatMsg[])) => void;

  setView: (v: View) => void;
  setCalMode: (m: CalMode) => void;
  setFocusDate: (iso: string | null) => void;
  load: () => Promise<void>;
  refreshLlm: () => Promise<void>;

  loadHabits: () => Promise<void>;
  createHabit: (name: string, color: string, durationMinutes: number) => Promise<void>;
  updateHabit: (id: number, name: string, color: string, durationMinutes: number) => Promise<void>;
  toggleHabit: (id: number, day?: string | null) => Promise<void>;
  deleteHabit: (id: number) => Promise<void>;
  scheduleHabit: (id: number, day?: string | null) => Promise<void>;

  plan: (text: string, history: { role: string; content: string }[]) => Promise<PlanOutcome>;
  createTask: (title: string, minutes: number, deadline: string | null, priority: number, projectId?: number | null) => Promise<void>;
  setTaskStatus: (id: number, status: string) => Promise<void>;
  deleteTask: (id: number) => Promise<void>;
  deleteProject: (id: number) => Promise<void>;
  setProjectArchived: (id: number, archived: boolean) => Promise<void>;
  addEvent: (title: string, start: string, end: string, kind: string) => Promise<void>;
  deleteEvent: (id: number) => Promise<void>;
  moveBlock: (id: number, start: string, end: string) => Promise<void>;
  unlockBlock: (id: number, start: string, end: string) => Promise<void>;
  reschedule: () => Promise<void>;
  saveSettings: (s: Settings) => Promise<void>;
  createBooking: (eventTypeId: number, name: string, email: string, start: string, end: string) => Promise<void>;

  syncing: boolean;
  connectGoogle: () => Promise<string>;
  disconnectGoogle: () => Promise<void>;
  syncGoogle: () => Promise<SyncSummary>;
}

export const useStore = create<State>((set, get) => {
  const applyData = (d: AppData) =>
    set({
      settings: d.settings,
      projects: d.projects,
      tasks: d.tasks,
      events: d.events,
      blocks: d.blocks,
      eventTypes: d.eventTypes,
      bookings: d.bookings,
      loaded: true,
    });

  const refreshData = async () => applyData(await api.loadAll());

  // Fire-and-forget Google sync after a local change (only when connected and idle —
  // the `syncing` guard naturally debounces bursts of edits).
  const maybeSync = () => {
    const st = get();
    if (st.settings?.googleConnected && !st.syncing) {
      st.syncGoogle().catch(() => {});
    }
  };

  // Run a mutation that returns a ScheduleResult, store conflicts, then refresh.
  const mutate = async (fn: () => Promise<ScheduleResult>) => {
    set({ busy: true });
    try {
      const r = await fn();
      set({ conflicts: r.conflicts });
      await refreshData();
    } finally {
      set({ busy: false });
    }
    maybeSync();
  };

  return {
    loaded: false,
    busy: false,
    syncing: false,
    view: "calendar",
    settings: null,
    projects: [],
    tasks: [],
    events: [],
    blocks: [],
    eventTypes: [],
    bookings: [],
    conflicts: [],
    llm: null,
    calMode: "week",
    focusDateIso: null,
    habits: [],
    chatMessages: [],

    setChatMessages: (m) => set((s) => ({ chatMessages: typeof m === "function" ? m(s.chatMessages) : m })),

    setView: (v) => set({ view: v }),
    setCalMode: (m) => set({ calMode: m }),
    setFocusDate: (iso) => set({ focusDateIso: iso }),

    load: async () => {
      await refreshData();
      await get().refreshLlm();
      // Auto-start the local inference server on open if it isn't already up, so the app is
      // "AI ready" without a manual click. ensure_inference is safe to call blindly: it no-ops
      // when a server is already running and errors (no download) when no model exists yet —
      // in which case the chat setup card handles downloading.
      if (!get().llm?.reachable) {
        try {
          await api.ensureInference();
        } catch {
          /* no model yet → the setup card prompts a download */
        }
        await get().refreshLlm();
      }
    },

    refreshLlm: async () => {
      try {
        set({ llm: await api.llmStatus() });
      } catch {
        /* ignore */
      }
    },

    plan: async (text, history) => {
      set({ busy: true });
      try {
        const outcome = await api.planTasks(text, history);
        await refreshData();
        // plan_tasks reschedules internally; pull fresh conflicts.
        const r = await api.reschedule();
        set({ conflicts: r.conflicts });
        // If the AI created habits, refresh the Habits page data too.
        if (outcome.createdHabitNames.length) await get().loadHabits();
        return outcome;
      } finally {
        set({ busy: false });
        maybeSync();
      }
    },

    createTask: (title, minutes, deadline, priority, projectId = null) =>
      mutate(() => api.createTask(title, minutes, deadline, priority, projectId)),
    setTaskStatus: (id, status) => mutate(() => api.setTaskStatus(id, status)),
    deleteTask: (id) => mutate(() => api.deleteTask(id)),
    deleteProject: (id) => mutate(() => api.deleteProject(id)),
    setProjectArchived: (id, archived) => mutate(() => api.setProjectArchived(id, archived)),
    addEvent: (title, start, end, kind) => mutate(() => api.addEvent(title, start, end, kind)),
    deleteEvent: (id) => mutate(() => api.deleteEvent(id)),
    moveBlock: (id, start, end) => mutate(() => api.lockBlock(id, true, start, end)),
    unlockBlock: (id, start, end) => mutate(() => api.lockBlock(id, false, start, end)),
    reschedule: () => mutate(() => api.reschedule()),
    createBooking: (eventTypeId, name, email, start, end) =>
      mutate(() => api.createBooking(eventTypeId, name, email, start, end)),

    // Habit commands return the full recomputed list, so we just store the result.
    loadHabits: async () => set({ habits: await api.listHabits() }),
    createHabit: async (name, color, durationMinutes) => set({ habits: await api.createHabit(name, color, "daily", durationMinutes) }),
    updateHabit: async (id, name, color, durationMinutes) => set({ habits: await api.updateHabit(id, name, color, durationMinutes) }),
    toggleHabit: async (id, day = null) => set({ habits: await api.toggleHabit(id, day) }),
    deleteHabit: async (id) => set({ habits: await api.deleteHabit(id) }),
    // Scheduling a habit creates a calendar event + re-plans, so refresh app data via mutate.
    scheduleHabit: (id, day = null) => mutate(() => api.scheduleHabit(id, day)),

    saveSettings: async (s) => {
      await api.saveSettings(s);
      await refreshData();
      await get().reschedule();
      get().refreshLlm();
    },

    connectGoogle: async () => {
      const email = await api.connectGoogle();
      await refreshData(); // googleConnected now true
      try {
        await get().syncGoogle();
      } catch {
        /* surfaced in settings */
      }
      return email;
    },

    disconnectGoogle: async () => {
      await api.disconnectGoogle();
      await refreshData();
    },

    syncGoogle: async () => {
      if (get().syncing) return { pulled: 0, pushed: 0, removed: 0, blocksMirrored: 0 };
      set({ syncing: true });
      try {
        const summary = await api.syncGoogle();
        const r = await api.reschedule();
        set({ conflicts: r.conflicts });
        await refreshData();
        return summary;
      } finally {
        set({ syncing: false });
      }
    },
  };
});
