// Typed wrappers over the Rust command surface. Types mirror the serde (camelCase) structs.
import { invoke } from "@tauri-apps/api/core";

export interface Settings {
  timezone: string;
  workStart: string; // "09:00"
  workEnd: string; // "17:00"
  workDays: number[]; // 1=Mon..7=Sun
  horizonDays: number;
  bufferMinutes: number;
  defaultMinChunk: number;
  defaultMaxChunk: number;
  modelId: string;
  llmBaseUrl: string;
  googleConnected: boolean;
  googleClientId: string;
  googleClientSecret: string;
}

export interface SyncSummary {
  pulled: number;
  pushed: number;
  removed: number;
  blocksMirrored: number;
}

export interface Project {
  id: number;
  name: string;
  color: string;
  createdAt: string;
}

export interface Task {
  id: number;
  projectId: number | null;
  title: string;
  notes: string;
  estimatedMinutes: number;
  deadline: string | null;
  earliestStart: string | null;
  priority: number; // 1..4
  minChunkMinutes: number;
  maxChunkMinutes: number;
  status: "todo" | "scheduled" | "in_progress" | "done";
  createdAt: string;
  dependsOn: number[];
}

export interface CalEvent {
  id: number;
  title: string;
  start: string;
  end: string;
  kind: string; // fixed|busy
  source: string;
  createdAt: string;
  provider: string | null;
  externalId: string | null;
  accountId: number | null;
  etag: string | null;
}

export interface Block {
  id: number;
  taskId: number;
  start: string;
  end: string;
  locked: boolean;
  provider: string | null;
  externalId: string | null;
  syncState: string | null;
}

export interface EventType {
  id: number;
  name: string;
  durationMinutes: number;
  bufferMinutes: number;
  color: string;
}

export interface Booking {
  id: number;
  eventTypeId: number;
  inviteeName: string;
  inviteeEmail: string;
  start: string;
  end: string;
  status: string;
  createdAt: string;
}

export type Conflict =
  | { kind: "dependencyCycle"; taskIds: number[] }
  | { kind: "unschedulable"; taskId: number; title: string; remainingMinutes: number }
  | { kind: "deadlineMiss"; taskId: number; title: string; scheduledEnd: string; deadline: string };

export interface ScheduleResult {
  blocks: Block[];
  conflicts: Conflict[];
}

export interface AppData {
  settings: Settings;
  projects: Project[];
  tasks: Task[];
  events: CalEvent[];
  blocks: Block[];
  eventTypes: EventType[];
  bookings: Booking[];
}

export interface PlanOutcome {
  createdTaskIds: number[];
  projectNames: string[];
  createdEventTitles: string[];
  updatedEventTitles: string[];
  removedEventTitles: string[];
  clarifications: string[];
}

export interface ModelInfo {
  id: string;
  name: string;
  filename: string;
  url: string;
  sizeMb: number;
  note: string;
}

export interface LlmStatus {
  reachable: boolean;
  baseUrl: string;
  modelPresent: boolean;
  modelId: string;
  models: ModelInfo[];
}

export interface BookingSlot {
  start: string;
  end: string;
}

export interface HabitDay {
  day: string; // "YYYY-MM-DD"
  done: boolean;
}

export interface HabitStats {
  id: number;
  name: string;
  color: string;
  cadence: string;
  durationMinutes: number;
  createdAt: string;
  doneToday: boolean;
  currentStreak: number;
  longestStreak: number;
  completionRate: number; // 0..1 over the last 30 days
  totalDone: number;
  history: HabitDay[]; // contiguous days, oldest → today
}

// ---- commands ----
export const api = {
  loadAll: () => invoke<AppData>("load_all"),
  reschedule: () => invoke<ScheduleResult>("reschedule"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),

  planTasks: (text: string, history: { role: string; content: string }[]) =>
    invoke<PlanOutcome>("plan_tasks", { text, history }),

  createTask: (title: string, estimatedMinutes: number, deadline: string | null, priority: number, projectId: number | null) =>
    invoke<ScheduleResult>("create_task", { title, estimatedMinutes, deadline, priority, projectId }),
  setTaskStatus: (id: number, status: string) => invoke<ScheduleResult>("set_task_status", { id, status }),
  deleteTask: (id: number) => invoke<ScheduleResult>("delete_task", { id }),

  addEvent: (title: string, start: string, end: string, kind: string) =>
    invoke<ScheduleResult>("add_event", { title, start, end, kind }),
  deleteEvent: (id: number) => invoke<ScheduleResult>("delete_event", { id }),
  lockBlock: (id: number, locked: boolean, start: string, end: string) =>
    invoke<ScheduleResult>("lock_block", { id, locked, start, end }),

  listEventTypes: () => invoke<EventType[]>("list_event_types"),
  createEventType: (name: string, durationMinutes: number, bufferMinutes: number, color: string) =>
    invoke<number>("create_event_type", { name, durationMinutes, bufferMinutes, color }),
  deleteEventType: (id: number) => invoke<void>("delete_event_type", { id }),
  bookingSlots: (eventTypeId: number, horizonDays: number) =>
    invoke<BookingSlot[]>("booking_slots", { eventTypeId, horizonDays }),
  createBooking: (eventTypeId: number, name: string, email: string, start: string, end: string) =>
    invoke<ScheduleResult>("create_booking", { eventTypeId, name, email, start, end }),

  listHabits: () => invoke<HabitStats[]>("list_habits"),
  createHabit: (name: string, color: string, cadence: string, durationMinutes: number) =>
    invoke<HabitStats[]>("create_habit", { name, color, cadence, durationMinutes }),
  updateHabit: (id: number, name: string, color: string, durationMinutes: number) =>
    invoke<HabitStats[]>("update_habit", { id, name, color, durationMinutes }),
  toggleHabit: (id: number, day: string | null) => invoke<HabitStats[]>("toggle_habit", { id, day }),
  deleteHabit: (id: number) => invoke<HabitStats[]>("delete_habit", { id }),
  scheduleHabit: (id: number, day: string | null) => invoke<ScheduleResult>("schedule_habit", { id, day }),

  connectGoogle: () => invoke<string>("connect_google"),
  disconnectGoogle: () => invoke<void>("disconnect_google"),
  syncGoogle: () => invoke<SyncSummary>("sync_google"),
  syncCalendar: () => invoke<number>("sync_calendar"),

  llmStatus: () => invoke<LlmStatus>("llm_status"),
  listModels: () => invoke<ModelInfo[]>("list_models"),
  modelPresent: (id: string) => invoke<boolean>("model_present", { id }),
  downloadModel: (id: string, sha256?: string) => invoke<string>("download_model", { id, sha256: sha256 ?? null }),
  ensureInference: () => invoke<string>("ensure_inference"),
};
