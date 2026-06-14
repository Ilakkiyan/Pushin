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
  type Label,
  type LabelInput,
  type LabelKind,
  type LlmStatus,
  type Page,
  type VaultAnswer,
  type PlanOutcome,
  type Project,
  type ScheduleResult,
  type Settings,
  type SyncSummary,
  type Task,
} from "../lib/ipc";

type View = "calendar" | "projects" | "habits" | "vault" | "graph" | "inbox" | "label" | "booking" | "settings";
type CalMode = "week" | "month";

// One turn in the chat transcript. Kept in the store (not ChatPane's local state) so the
// conversation survives navigating away from the calendar; it lives only for the session and
// is gone when the app closes.
export type ChatMsg = { role: "user" | "ai"; text: string };

interface State {
  loaded: boolean;
  busy: boolean;
  view: View;
  sidebarCollapsed: boolean;
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
  calColorByLabel: boolean;
  calLabelFilterIds: number[];
  // When the month view hands off to the week view, the day to open to.
  focusDateIso: string | null;
  habits: HabitStats[];

  // Vault: the page tree (lightweight rows) + which page is open in the editor.
  pages: Page[];
  currentPageId: number | null;
  // Inbox: unsorted quick captures + the quick-capture modal's open flag.
  inbox: Page[];
  captureOpen: boolean;

  // Labels: the cross-cutting taxonomy + which label's filtered view is open.
  labels: Label[];
  currentLabelId: number | null;

  // Chat transcript, persisted for the session so it isn't lost on page/settings changes.
  chatMessages: ChatMsg[];
  setChatMessages: (m: ChatMsg[] | ((prev: ChatMsg[]) => ChatMsg[])) => void;

  setView: (v: View) => void;
  setSidebarCollapsed: (c: boolean) => void;
  setCalMode: (m: CalMode) => void;
  setCalColorByLabel: (enabled: boolean) => void;
  toggleCalLabelFilter: (id: number) => void;
  clearCalLabelFilters: () => void;
  setFocusDate: (iso: string | null) => void;
  load: () => Promise<void>;
  refreshLlm: () => Promise<void>;

  loadHabits: () => Promise<void>;
  createHabit: (name: string, color: string, cadence: string, days: number[], intervalDays: number, durationMinutes: number) => Promise<void>;
  updateHabit: (id: number, name: string, color: string, cadence: string, days: number[], intervalDays: number, durationMinutes: number) => Promise<void>;
  toggleHabit: (id: number, day?: string | null) => Promise<void>;
  deleteHabit: (id: number) => Promise<void>;
  scheduleHabit: (id: number, day?: string | null) => Promise<void>;
  setHabitScheduled: (id: number, scheduled: boolean) => Promise<void>;

  // Save a durable fact to the vault (the chat "Remember this?" chip).
  addNote: (content: string) => Promise<void>;

  loadPages: () => Promise<void>;
  openPage: (id: number) => void;
  openDaily: (date: string) => Promise<void>;
  openEntityNote: (kind: "task" | "event", id: number, title: string) => Promise<void>;
  createPage: (parentId?: number | null) => Promise<Page>;
  savePage: (id: number, title: string, icon: string | null, content: string, contentJson: string | null, linkTitles: string[]) => Promise<void>;
  deletePage: (id: number) => Promise<void>;
  movePage: (id: number, parentId: number | null, sortOrder: number) => Promise<void>;
  askVault: (question: string) => Promise<VaultAnswer>;
  loadInbox: () => Promise<void>;
  captureNote: (text: string) => Promise<void>;
  keepInboxNote: (id: number) => Promise<void>;
  setCaptureOpen: (open: boolean) => void;

  loadLabels: () => Promise<void>;
  createLabel: (input: LabelInput) => Promise<void>;
  updateLabel: (id: number, input: LabelInput) => Promise<void>;
  deleteLabel: (id: number) => Promise<void>;
  mergeLabels: (from: number, into: number) => Promise<void>;
  quickLabel: (name: string, color: string) => Promise<Label[]>;
  setEntityLabels: (kind: LabelKind, entityId: number, labelIds: number[]) => Promise<void>;
  openLabel: (id: number) => void;

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
  cancelBooking: (id: number) => Promise<void>;

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
    sidebarCollapsed: false,
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
    calColorByLabel: false,
    calLabelFilterIds: [],
    focusDateIso: null,
    habits: [],
    pages: [],
    currentPageId: null,
    inbox: [],
    captureOpen: false,
    labels: [],
    currentLabelId: null,
    chatMessages: [],

    setChatMessages: (m) => set((s) => ({ chatMessages: typeof m === "function" ? m(s.chatMessages) : m })),

    setView: (v) => set({ view: v }),
    setSidebarCollapsed: (c) => set({ sidebarCollapsed: c }),
    setCalMode: (m) => set({ calMode: m }),
    setCalColorByLabel: (enabled) => set({ calColorByLabel: enabled }),
    toggleCalLabelFilter: (id) =>
      set((s) => ({
        calLabelFilterIds: s.calLabelFilterIds.includes(id)
          ? s.calLabelFilterIds.filter((x) => x !== id)
          : [...s.calLabelFilterIds, id],
      })),
    clearCalLabelFilters: () => set({ calLabelFilterIds: [] }),
    setFocusDate: (iso) => set({ focusDateIso: iso }),

    load: async () => {
      await refreshData();
      get().loadPages().catch(() => {});
      get().loadInbox().catch(() => {});
      get().loadLabels().catch(() => {});
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
      // Once the chat AI is set up, bring Hermes' memory engine online too (auto-downloads the
      // tiny embedding model on first run + spawns its server). Best-effort and in the background —
      // if it's not ready, recall just uses keyword search. Skipped until a chat model exists so we
      // don't download anything before the user has opted into the AI.
      if (get().llm?.reachable) {
        api.ensureEmbeddings().catch(() => {});
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
    cancelBooking: (id) => mutate(() => api.cancelBooking(id)),

    // Habit commands return the full recomputed list, so we just store the result.
    loadHabits: async () => set({ habits: await api.listHabits() }),
    createHabit: async (name, color, cadence, days, intervalDays, durationMinutes) => set({ habits: await api.createHabit(name, color, cadence, days, intervalDays, durationMinutes) }),
    updateHabit: async (id, name, color, cadence, days, intervalDays, durationMinutes) => set({ habits: await api.updateHabit(id, name, color, cadence, days, intervalDays, durationMinutes) }),
    toggleHabit: async (id, day = null) => set({ habits: await api.toggleHabit(id, day) }),
    deleteHabit: async (id) => set({ habits: await api.deleteHabit(id) }),
    // Scheduling a habit creates a calendar event + re-plans, so refresh app data via mutate.
    scheduleHabit: (id, day = null) => mutate(() => api.scheduleHabit(id, day)),
    // Toggling a habit across the planning period changes both the calendar (mutate) and the
    // habits' scheduled-day counts (loadHabits) — refresh both.
    setHabitScheduled: async (id, scheduled) => {
      await mutate(() => api.setHabitScheduled(id, scheduled));
      await get().loadHabits();
    },

    // Hermes note commands return the full list, so we just store the result. Recall is read-only.
    addNote: async (content) => {
      await api.hermesAddNote(content);
    },

    // Vault pages. The tree is lightweight (no bodies); the editor fetches a full page via getPage.
    loadPages: async () => set({ pages: await api.listPages() }),
    openPage: (id) => set({ currentPageId: id, view: "vault" }),
    // Open (creating on first access) the note for a calendar day, then refresh the tree so it
    // appears under the Journal section.
    openDaily: async (date) => {
      const page = await api.dailyNote(date);
      set({ pages: await api.listPages(), currentPageId: page.id, view: "vault" });
    },
    // Open the page a task/event is linked to, creating + linking one (titled after the entity) on
    // first use — the bridge from the calendar into the vault.
    openEntityNote: async (kind, id, title) => {
      const existing = await api.entityPages(kind, id);
      const page = existing[0] ?? (await api.createPage(title, null));
      if (!existing.length) await api.linkPageEntity(page.id, kind, id);
      set({ pages: await api.listPages(), currentPageId: page.id, view: "vault" });
    },
    createPage: async (parentId = null) => {
      const page = await api.createPage("Untitled", parentId ?? null);
      set({ pages: await api.listPages(), currentPageId: page.id, view: "vault" });
      return page;
    },
    savePage: async (id, title, icon, content, contentJson, linkTitles) => {
      await api.updatePage(id, title, icon, content, contentJson, linkTitles);
      // Refresh the tree so the sidebar title/order stays in sync (cheap, no bodies).
      set({ pages: await api.listPages() });
    },
    deletePage: async (id) => {
      const pages = await api.deletePage(id);
      set((s) => ({ pages, currentPageId: s.currentPageId === id ? null : s.currentPageId }));
    },
    movePage: async (id, parentId, sortOrder) => set({ pages: await api.movePage(id, parentId, sortOrder) }),
    askVault: (question) => api.vaultAsk(question),
    loadInbox: async () => set({ inbox: await api.listInbox() }),
    captureNote: async (text) => {
      await api.captureNote(text);
      set({ inbox: await api.listInbox() });
    },
    keepInboxNote: async (id) => {
      await api.keepInboxNote(id);
      set({ inbox: await api.listInbox() });
      await get().loadPages();
    },
    setCaptureOpen: (open) => set({ captureOpen: open }),

    // Labels. Mutating commands return the refreshed list, so we just store it.
    loadLabels: async () => set({ labels: await api.listLabels() }),
    createLabel: async (input) => set({ labels: await api.createLabel(input) }),
    updateLabel: async (id, input) => set({ labels: await api.updateLabel(id, input) }),
    deleteLabel: async (id) => {
      const labels = await api.deleteLabel(id);
      set((s) => ({ labels, currentLabelId: s.currentLabelId === id ? null : s.currentLabelId }));
    },
    mergeLabels: async (from, into) => set({ labels: await api.mergeLabels(from, into) }),
    quickLabel: async (name, color) => {
      const labels = await api.quickLabel(name, color);
      set({ labels });
      return labels;
    },
    setEntityLabels: async (kind, entityId, labelIds) => {
      await api.setEntityLabels(kind, entityId, labelIds);
      set({ labels: await api.listLabels() }); // refresh counts
      if (kind === "task") {
        const r = await api.reschedule();
        set({ conflicts: r.conflicts });
        await refreshData();
        maybeSync();
      }
    },
    openLabel: (id) => set({ currentLabelId: id, view: "label" }),

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
