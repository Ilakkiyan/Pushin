// Typed wrappers over the Rust command surface. Types mirror the serde (camelCase) structs.
import { invoke } from "@tauri-apps/api/core";

/** A recurring blocked time / routine the scheduler keeps free. `end <= start` runs overnight;
 *  empty `days` means every day. `kind` is a UI label only ("routine" | "blocked"). */
export interface Commitment {
  id: string;
  name: string;
  start: string; // "HH:MM"
  end: string; // "HH:MM"
  days: number[]; // 1=Mon..7=Sun; empty = every day
  kind: string;
}

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
  // Personalization (first-run modal + Settings).
  onboarded: boolean;
  sleepEnabled: boolean;
  sleepStart: string; // bedtime "HH:MM"
  sleepEnd: string; // wake time "HH:MM"
  commitments: Commitment[];
  embedModel: string; // Hermes embedding model ("" = keyword-only recall)
  vaultDir?: string | null; // folder the vault is mirrored to as markdown files (null = SQLite-only)
  // "About you" profile from setup — selected archetype keys + a free-form blurb, fed to the AI.
  archetypes: string[];
  aboutMe: string;
}

/** A `.md` change the Rust watcher saw on disk (two-way vault, files→DB). Emitted as `vault-changed`. */
export interface VaultChange {
  relPath: string;
  content: string;
  kind: "update" | "remove";
}

export interface SyncSummary {
  pulled: number;
  pushed: number;
  removed: number;
  blocksMirrored: number;
}

/** A device sharing this private sync mesh. */
export interface SyncPeer {
  nodeId: string;
  name: string;
  lastSeen: string | null;
  lastAckedHlc: string;
}

/** State of the device-sync mesh for this device. */
export interface SyncStatus {
  enabled: boolean; // this device belongs to a network (has the shared key)
  running: boolean; // the mesh engine is bound and serving
  nodeId: string;
  deviceName: string;
  useRelay: boolean;
  peers: SyncPeer[];
}

export interface Project {
  id: number;
  name: string;
  color: string;
  createdAt: string;
  archivedAt: string | null; // null = active; timestamp = completed (in the bin)
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
  slug: string;
  shareToken: string;
  enabled: boolean;
}

export interface Booking {
  id: number;
  eventTypeId: number;
  eventId: number | null;
  inviteeName: string;
  inviteeEmail: string;
  start: string;
  end: string;
  status: string;
  createdAt: string;
}

/** A focus session on a task (time-tracking). `end` is undefined while running. */
export interface FocusSession {
  id: number;
  taskId: number;
  start: string;
  end?: string;
  minutes: number;
}

/** A person in the relationship layer (private CRM). Auto-created from booking invitees. */
export interface Person {
  id: number;
  name: string;
  email?: string;
  notes: string;
  createdAt: string;
  updatedAt: string;
}

/** One attendee in a meeting brief: the person + a quick relationship summary. */
export interface AttendeeBrief {
  person: Person;
  totalMeetings: number;
  lastMet?: string;
}

/** The Meeting Companion's deterministic pre-meeting brief. */
export interface MeetingBrief {
  event: CalEvent;
  attendees: AttendeeBrief[];
  linkedPages: Page[];
}

/** The morning Daily Briefing — today's agenda, assembled deterministically (no LLM). */
export interface Briefing {
  date: string;
  weekday: string;
  events: CalEvent[];
  dueTasks: Task[];
  focusMinutes: number;
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
  createdEventIds: number[];
  projectNames: string[];
  createdEventTitles: string[];
  updatedEventTitles: string[];
  removedEventTitles: string[];
  createdHabitNames: string[];
  clarifications: string[];
  /** Vault notes auto-recalled to inform this plan (shown in chat for transparency). */
  recalledNotes?: string[];
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

export interface ModelRecommendation {
  modelId: string;
  reason: string; // e.g. "16 GB RAM is a good match for the 7B, plus a GPU for acceleration"
  ramGb: number | null;
  hasGpu: boolean;
}

export interface Memory {
  id: number;
  content: string;
  createdAt: string;
}

export interface BookingSlot {
  start: string;
  end: string;
}

export interface BookingServerStatus {
  running: boolean;
  localUrl: string | null;
  host: string;
  port: number | null;
}

export interface HabitDay {
  day: string; // "YYYY-MM-DD"
  done: boolean;
  due: boolean; // expected that day per the habit's cadence
}

export interface HabitStats {
  id: number;
  name: string;
  color: string;
  cadence: string; // "daily" | "weekly" | "interval"
  days: number[]; // weekdays 1=Mon..7=Sun (cadence="weekly")
  intervalDays: number; // step for cadence="interval" (2 = every other day)
  durationMinutes: number;
  createdAt: string;
  doneToday: boolean;
  currentStreak: number;
  longestStreak: number;
  completionRate: number; // 0..1 over the last 30 days
  totalDone: number;
  scheduledDays: number; // # of upcoming days this habit is on the calendar (0 = not scheduled)
  history: HabitDay[]; // contiguous days, oldest → today
}

/** A Hermes memory note. `indexed` = an embedding exists (semantic recall available); `score` is
 *  set only on recall results (higher = more relevant). */
export interface Note {
  id: number;
  content: string;
  createdAt: string;
  updatedAt: string;
  indexed: boolean;
  score?: number;
}

export interface RecallResult {
  mode: "semantic" | "keyword";
  notes: Note[];
}

/** A vault page — a Notion-style document with an Obsidian-style place in the page tree. `content`
 *  is the rendered plaintext (recall/search index); `contentJson` is the BlockNote block array
 *  (undefined on legacy notes → opened as a plain paragraph doc). `score` is set only on recall. */
export interface Page {
  id: number;
  title: string;
  icon?: string;
  parentId?: number;
  content: string;
  contentJson?: string;
  sortOrder: number;
  archived: boolean;
  /** Set when this page IS a calendar day's note ('YYYY-MM-DD'). */
  dailyDate?: string;
  /** True while the page is an unsorted quick-capture in the Inbox. */
  inbox: boolean;
  createdAt: string;
  updatedAt: string;
  indexed: boolean;
  score?: number;
}

/** A reference to an entity (used by page links and label taggings). */
export interface EntityRef {
  kind: "task" | "event" | "habit" | "page" | "project";
  id: number;
}

/** A label — Pushin's cross-cutting taxonomy over any entity. `pref*` set = an "actionable" label
 *  whose scheduling prefs the scheduler honors. `count` = how many entities carry it (list only). */
export interface Label {
  id: number;
  name: string;
  color: string;
  icon?: string;
  groupName?: string;
  archived: boolean;
  prefWindowStart?: string;
  prefWindowEnd?: string;
  prefMinChunk?: number;
  prefMaxChunk?: number;
  prefBatch: boolean;
  createdAt: string;
  count: number;
}

/** Create/update payload for a label. */
export interface LabelInput {
  name: string;
  color: string;
  icon?: string | null;
  groupName?: string | null;
  prefWindowStart?: string | null;
  prefWindowEnd?: string | null;
  prefMinChunk?: number | null;
  prefMaxChunk?: number | null;
  prefBatch: boolean;
}

/** Entity kinds a label can be applied to. */
export type LabelKind = "task" | "event" | "habit" | "page" | "project" | "person";
export type EntityLabelMap = Record<number, Label[]>;

/** A Markdown file found by the vault importer (title + raw markdown). */
export interface ImportDoc {
  title: string;
  markdown: string;
}

/** An answer from "ask your vault" (local RAG): the generated answer + the page ids it cited. */
export interface VaultAnswer {
  answer: string;
  citations: number[];
}

export interface GraphNode {
  id: number;
  title: string;
  parentId?: number;
  degree: number;
}

export interface GraphEdge {
  source: number;
  target: number;
}

export interface PageGraph {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

// ---- commands ----
export const api = {
  loadAll: () => invoke<AppData>("load_all"),
  reschedule: () => invoke<ScheduleResult>("reschedule"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  /** Mirror a page to `<vault_dir>/<relPath>` as markdown (no-op if no vault folder is set). */
  vaultWrite: (pageId: number, relPath: string, markdown: string) =>
    invoke<void>("vault_write", { pageId, relPath, markdown }),
  /** The page id currently mapped to `relPath`, or null (file→page lookup for the watcher). */
  vaultPageForPath: (relPath: string) => invoke<number | null>("vault_page_for_path", { relPath }),
  /** Map an externally-created file to a (just-created) page, without writing the file back. */
  vaultLinkPath: (pageId: number, relPath: string) => invoke<void>("vault_link_path", { pageId, relPath }),
  /** A file was deleted on disk: unlink the page→file mapping (the page survives). */
  vaultUnlinkPath: (relPath: string) => invoke<void>("vault_unlink_path", { relPath }),
  /** (Re)start the vault file watcher to match the current vault folder (boot / after a change). */
  vaultRefreshWatch: () => invoke<void>("vault_refresh_watch"),

  planTasks: (text: string, history: { role: string; content: string }[]) =>
    invoke<PlanOutcome>("plan_tasks", { text, history }),

  extractMemories: (text: string) => invoke<string[]>("extract_memories", { text }),

  createTask: (title: string, estimatedMinutes: number, deadline: string | null, priority: number, projectId: number | null) =>
    invoke<ScheduleResult>("create_task", { title, estimatedMinutes, deadline, priority, projectId }),
  setTaskStatus: (id: number, status: string) => invoke<ScheduleResult>("set_task_status", { id, status }),
  deleteTask: (id: number) => invoke<ScheduleResult>("delete_task", { id }),

  deleteProject: (id: number) => invoke<ScheduleResult>("delete_project", { id }),
  setProjectArchived: (id: number, archived: boolean) =>
    invoke<ScheduleResult>("set_project_archived", { id, archived }),

  addEvent: (title: string, start: string, end: string, kind: string) =>
    invoke<ScheduleResult>("add_event", { title, start, end, kind }),
  deleteEvent: (id: number) => invoke<ScheduleResult>("delete_event", { id }),
  lockBlock: (id: number, locked: boolean, start: string, end: string) =>
    invoke<ScheduleResult>("lock_block", { id, locked, start, end }),

  listEventTypes: () => invoke<EventType[]>("list_event_types"),
  createEventType: (name: string, durationMinutes: number, bufferMinutes: number, color: string) =>
    invoke<number>("create_event_type", { name, durationMinutes, bufferMinutes, color }),
  updateEventType: (id: number, name: string, durationMinutes: number, bufferMinutes: number, color: string, enabled: boolean) =>
    invoke<EventType>("update_event_type", { id, name, durationMinutes, bufferMinutes, color, enabled }),
  regenerateEventTypeToken: (id: number) => invoke<EventType>("regenerate_event_type_token", { id }),
  deleteEventType: (id: number) => invoke<void>("delete_event_type", { id }),
  bookingServerStatus: () => invoke<BookingServerStatus>("booking_server_status"),
  startBookingServer: (port?: number | null) => invoke<BookingServerStatus>("start_booking_server", { port: port ?? null }),
  stopBookingServer: () => invoke<BookingServerStatus>("stop_booking_server"),
  bookingSlots: (eventTypeId: number, horizonDays: number) =>
    invoke<BookingSlot[]>("booking_slots", { eventTypeId, horizonDays }),
  createBooking: (eventTypeId: number, name: string, email: string, start: string, end: string) =>
    invoke<ScheduleResult>("create_booking", { eventTypeId, name, email, start, end }),
  cancelBooking: (id: number) => invoke<ScheduleResult>("cancel_booking", { id }),

  listHabits: () => invoke<HabitStats[]>("list_habits"),
  createHabit: (name: string, color: string, cadence: string, days: number[], intervalDays: number, durationMinutes: number) =>
    invoke<HabitStats[]>("create_habit", { name, color, cadence, days, intervalDays, durationMinutes }),
  updateHabit: (id: number, name: string, color: string, cadence: string, days: number[], intervalDays: number, durationMinutes: number) =>
    invoke<HabitStats[]>("update_habit", { id, name, color, cadence, days, intervalDays, durationMinutes }),
  toggleHabit: (id: number, day: string | null) => invoke<HabitStats[]>("toggle_habit", { id, day }),
  deleteHabit: (id: number) => invoke<HabitStats[]>("delete_habit", { id }),
  scheduleHabit: (id: number, day: string | null) => invoke<ScheduleResult>("schedule_habit", { id, day }),
  // Drag a habit block on the calendar → move this occurrence, learn its preferred time, re-place future ones.
  moveHabit: (eventId: number, newStart: string) => invoke<ScheduleResult>("move_habit", { eventId, newStart }),

  // Hermes (memory layer): save a durable fact + semantic recall over the vault.
  hermesAddNote: (content: string) => invoke<void>("hermes_add_note", { content }),
  hermesRecall: (query: string, k?: number) => invoke<RecallResult>("hermes_recall", { query, k: k ?? null }),
  // AI memory facts (private backend store, shown in Settings ▸ AI Memory — not in the vault tree).
  listMemories: () => invoke<Memory[]>("list_memories"),
  deleteMemory: (id: number) => invoke<void>("delete_memory", { id }),

  // Vault pages (Notion-style documents + Obsidian-style links/graph)
  listPages: () => invoke<Page[]>("list_pages"),
  getPage: (id: number) => invoke<Page>("get_page", { id }),
  createPage: (title: string, parentId: number | null, content?: string) =>
    invoke<Page>("create_page", { title, parentId, content: content ?? null }),
  updatePage: (id: number, title: string, icon: string | null, content: string, contentJson: string | null, linkTitles: string[]) =>
    invoke<Page>("update_page", { id, title, icon, content, contentJson, linkTitles }),
  deletePage: (id: number) => invoke<Page[]>("delete_page", { id }),
  movePage: (id: number, parentId: number | null, sortOrder: number) =>
    invoke<Page[]>("move_page", { id, parentId, sortOrder }),
  pageBacklinks: (id: number) => invoke<Page[]>("page_backlinks", { id }),
  searchPages: (query: string) => invoke<Page[]>("search_pages", { query }),
  unlinkedMentions: (id: number) => invoke<Page[]>("unlinked_mentions", { id }),
  pageGraph: () => invoke<PageGraph>("page_graph"),
  vaultAsk: (question: string) => invoke<VaultAnswer>("vault_ask", { question }),
  /** Deharnessed general assistant (the "second brain" chat) — free-form reply grounded in the vault. */
  assistantChat: (message: string, history: { role: string; content: string }[]) =>
    invoke<string>("assistant_chat", { message, history }),
  /** Classify a message → "plan" (calendar planner) or "chat" (assistant), for the AI pane's Auto mode. */
  routeIntent: (message: string) => invoke<"plan" | "chat">("route_intent", { message }),
  dailyBriefing: (date?: string) => invoke<Briefing>("daily_briefing", { date: date ?? null }),
  meetingBrief: (eventId: number) => invoke<MeetingBrief>("meeting_brief", { eventId }),
  extractActionItems: (notes: string) => invoke<string[]>("extract_action_items", { notes }),
  // Focus / time-tracking
  startFocus: (taskId: number) => invoke<FocusSession>("start_focus", { taskId }),
  stopFocus: (id: number) => invoke<void>("stop_focus", { id }),
  activeFocus: () => invoke<FocusSession | null>("active_focus"),
  taskFocusMinutes: (taskId: number) => invoke<number>("task_focus_minutes", { taskId }),

  // People (relationship layer)
  listPeople: () => invoke<Person[]>("list_people"),
  getPerson: (id: number) => invoke<Person>("get_person", { id }),
  createPerson: (name: string, email: string | null, notes: string) => invoke<Person>("create_person", { name, email, notes }),
  updatePerson: (id: number, name: string, email: string | null, notes: string) => invoke<Person>("update_person", { id, name, email, notes }),
  deletePerson: (id: number) => invoke<void>("delete_person", { id }),
  dailyNote: (date: string) => invoke<Page>("daily_note", { date }),
  linkPageEntity: (pageId: number, kind: string, entityId: number) =>
    invoke<void>("link_page_entity", { pageId, kind, entityId }),
  unlinkPageEntity: (pageId: number, kind: string, entityId: number) =>
    invoke<void>("unlink_page_entity", { pageId, kind, entityId }),
  pageEntities: (pageId: number) => invoke<EntityRef[]>("page_entities", { pageId }),
  entityPages: (kind: string, entityId: number) => invoke<Page[]>("entity_pages", { kind, entityId }),

  // Labels (cross-cutting taxonomy)
  listLabels: () => invoke<Label[]>("list_labels"),
  createLabel: (input: LabelInput) => invoke<Label[]>("create_label", { input }),
  updateLabel: (id: number, input: LabelInput) => invoke<Label[]>("update_label", { id, input }),
  deleteLabel: (id: number) => invoke<Label[]>("delete_label", { id }),
  mergeLabels: (from: number, into: number) => invoke<Label[]>("merge_labels", { from, into }),
  setEntityLabels: (kind: LabelKind, entityId: number, labelIds: number[]) =>
    invoke<void>("set_entity_labels", { kind, entityId, labelIds }),
  labelsFor: (kind: LabelKind, entityId: number) => invoke<Label[]>("labels_for", { kind, entityId }),
  suggestLabels: (kind: LabelKind, entityId: number) => invoke<Label[]>("suggest_labels", { kind, entityId }),
  labelsForEntities: (kind: LabelKind, ids: number[]) =>
    invoke<EntityLabelMap>("labels_for_entities", { kind, ids }),
  quickLabel: (name: string, color: string) => invoke<Label[]>("quick_label", { name, color }),
  entitiesForLabel: (labelId: number) => invoke<EntityRef[]>("entities_for_label", { labelId }),
  readMarkdownDir: (path: string) => invoke<ImportDoc[]>("read_markdown_dir", { path }),
  captureNote: (text: string) => invoke<void>("capture_note", { text }),
  listInbox: () => invoke<Page[]>("list_inbox"),
  keepInboxNote: (id: number) => invoke<void>("keep_inbox_note", { id }),
  setHabitScheduled: (id: number, scheduled: boolean) => invoke<ScheduleResult>("set_habit_scheduled", { id, scheduled }),

  connectGoogle: () => invoke<string>("connect_google"),
  disconnectGoogle: () => invoke<void>("disconnect_google"),
  syncGoogle: () => invoke<SyncSummary>("sync_google"),

  llmStatus: () => invoke<LlmStatus>("llm_status"),
  listModels: () => invoke<ModelInfo[]>("list_models"),
  modelPresent: (id: string) => invoke<boolean>("model_present", { id }),
  recommendModel: () => invoke<ModelRecommendation>("recommend_model"),
  downloadModel: (id: string, sha256?: string) => invoke<string>("download_model", { id, sha256: sha256 ?? null }),
  ensureInference: () => invoke<string>("ensure_inference"),
  // Hermes: auto-download the embedding model + start the embeddings server (idempotent).
  ensureEmbeddings: () => invoke<string>("ensure_embeddings"),

  // Device sync: a private peer-to-peer mesh (Iroh) carrying a changeset log over SQLite.
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncCreateInvite: () => invoke<string>("sync_create_invite"),
  syncJoin: (ticket: string) => invoke<SyncStatus>("sync_join", { ticket }),
  syncNow: () => invoke<number>("sync_now"),
  syncRemovePeer: (nodeId: string) => invoke<void>("sync_remove_peer", { nodeId }),
  syncSetDeviceName: (name: string) => invoke<void>("sync_set_device_name", { name }),
  syncSetRelay: (useRelay: boolean) => invoke<void>("sync_set_relay", { useRelay }),
  syncLeave: () => invoke<void>("sync_leave"),
};
