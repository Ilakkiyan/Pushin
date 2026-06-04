import { create } from "zustand";
import {
  api,
  type AppData,
  type Block,
  type Booking,
  type CalEvent,
  type Conflict,
  type EventType,
  type LlmStatus,
  type PlanOutcome,
  type Project,
  type ScheduleResult,
  type Settings,
  type SyncSummary,
  type Task,
} from "../lib/ipc";

type View = "calendar" | "booking" | "settings";

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

  setView: (v: View) => void;
  load: () => Promise<void>;
  refreshLlm: () => Promise<void>;

  plan: (text: string, history: { role: string; content: string }[]) => Promise<PlanOutcome>;
  createTask: (title: string, minutes: number, deadline: string | null, priority: number) => Promise<void>;
  setTaskStatus: (id: number, status: string) => Promise<void>;
  deleteTask: (id: number) => Promise<void>;
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

    setView: (v) => set({ view: v }),

    load: async () => {
      await refreshData();
      get().refreshLlm();
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
        return outcome;
      } finally {
        set({ busy: false });
        maybeSync();
      }
    },

    createTask: (title, minutes, deadline, priority) =>
      mutate(() => api.createTask(title, minutes, deadline, priority, null)),
    setTaskStatus: (id, status) => mutate(() => api.setTaskStatus(id, status)),
    deleteTask: (id) => mutate(() => api.deleteTask(id)),
    addEvent: (title, start, end, kind) => mutate(() => api.addEvent(title, start, end, kind)),
    deleteEvent: (id) => mutate(() => api.deleteEvent(id)),
    moveBlock: (id, start, end) => mutate(() => api.lockBlock(id, true, start, end)),
    unlockBlock: (id, start, end) => mutate(() => api.lockBlock(id, false, start, end)),
    reschedule: () => mutate(() => api.reschedule()),
    createBooking: (eventTypeId, name, email, start, end) =>
      mutate(() => api.createBooking(eventTypeId, name, email, start, end)),

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
