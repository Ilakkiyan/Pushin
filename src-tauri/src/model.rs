//! Domain types shared across the Rust core and (via serde) the frontend.
//! All datetimes are naive-local ISO strings ("YYYY-MM-DDTHH:MM:SS").

use serde::{Deserialize, Serialize};

pub const DT_FMT: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub created_at: String,
    /// NULL while active; ISO timestamp once completed (moved to the Completed bin).
    pub archived_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub project_id: Option<i64>,
    pub title: String,
    pub notes: String,
    pub estimated_minutes: i64,
    pub deadline: Option<String>,
    pub earliest_start: Option<String>,
    pub priority: i64, // 1 low .. 4 urgent
    pub min_chunk_minutes: i64,
    pub max_chunk_minutes: i64,
    pub status: String, // todo|scheduled|in_progress|done
    pub created_at: String,
    /// Populated on read; ids this task depends on.
    #[serde(default)]
    pub depends_on: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub start: String,
    pub end: String,
    pub kind: String,   // fixed|busy
    pub source: String, // manual|import|google
    pub created_at: String,
    pub provider: Option<String>,
    pub external_id: Option<String>,
    pub account_id: Option<i64>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: i64,
    pub task_id: i64,
    pub start: String,
    pub end: String,
    pub locked: bool,
    pub provider: Option<String>,
    pub external_id: Option<String>,
    pub sync_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventType {
    pub id: i64,
    pub name: String,
    pub duration_minutes: i64,
    pub buffer_minutes: i64,
    pub color: String,
    pub slug: String,
    pub share_token: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Booking {
    pub id: i64,
    pub event_type_id: i64,
    pub event_id: Option<i64>,
    pub invitee_name: String,
    pub invitee_email: String,
    pub start: String,
    pub end: String,
    pub status: String,
    pub created_at: String,
}

/// A tracked habit. Recurrence is `cadence` + its parameters:
/// - "daily"   → every day.
/// - "weekly"  → only the weekdays in `days` (1=Mon..7=Sun).
/// - "interval"→ every `interval_days` days, anchored at `created_at` (2 = every other day).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub cadence: String,
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default = "default_interval_days")]
    pub interval_days: i64,
    pub duration_minutes: i64,
    pub archived: bool,
    pub created_at: String,
    /// Learned preferred time-of-day in minutes since midnight (set by dragging the habit on the
    /// calendar). `None` = no preference → the scheduler best-fits it into any free gap.
    #[serde(default)]
    pub preferred_minute: Option<i64>,
}

fn default_interval_days() -> i64 {
    1
}

/// An AI-tracked memory fact (from the chat "Remember this?" chip). Kept out of the user's vault tree
/// and surfaced in Settings ▸ AI Memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: i64,
    pub content: String,
    pub created_at: String,
}

/// One day in a habit's history (for the consistency heatmap). `day` is "YYYY-MM-DD".
/// `due` = the habit was expected that day (per its cadence); `done` = it was completed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitDay {
    pub day: String,
    pub done: bool,
    pub due: bool,
}

/// A habit plus the derived streak/consistency metrics the UI renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitStats {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub cadence: String,
    #[serde(default)]
    pub days: Vec<u8>,
    #[serde(default = "default_interval_days")]
    pub interval_days: i64,
    pub duration_minutes: i64,
    pub created_at: String,
    pub done_today: bool,
    pub current_streak: i64,
    pub longest_streak: i64,
    pub completion_rate: f64, // fraction of the last 30 days completed (0..1)
    pub total_done: i64,
    /// How many days from today forward this habit is dropped onto the calendar. 0 = not on the
    /// calendar; drives the "Add to calendar" toggle. Populated by `commands::habit_stats`.
    pub scheduled_days: i64,
    pub history: Vec<HabitDay>, // contiguous days, oldest → today, for the heatmap
}

/// A connected Google account + its OAuth tokens and incremental sync state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAccount {
    pub id: i64,
    pub email: String,
    pub calendar_id: String,
    pub sync_token: Option<String>,
    #[serde(skip_serializing)]
    pub access_token: Option<String>,
    #[serde(skip_serializing)]
    pub refresh_token: Option<String>,
    pub token_expiry: Option<String>,
    pub connected_at: String,
}

/// A note in Hermes, the on-device memory layer. `indexed` = an embedding exists for semantic
/// recall; `score` is populated only on recall results (relevance of this note to the query).
/// The embedding vector itself stays in the DB and is never serialized to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

/// One attendee in a meeting brief: the person plus a quick relationship summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttendeeBrief {
    pub person: Person,
    pub total_meetings: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_met: Option<String>,
}

/// The Meeting Companion's pre-meeting brief: the event, who's attending (with history), and any
/// notes linked to it. Assembled deterministically from bookings + people + entity links.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBrief {
    pub event: Event,
    pub attendees: Vec<AttendeeBrief>,
    pub linked_pages: Vec<Page>,
}

/// A focus session on a task (time-tracking). `end` is None while running; `minutes` is the elapsed
/// time (0 while running, derived on read).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusSession {
    pub id: i64,
    pub task_id: i64,
    pub start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    pub minutes: i64,
}

/// A person in the relationship layer (private CRM). Auto-created from booking invitees (and later
/// event attendees / `[[mentions]]`); recalled as `EntityKind::Person`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub notes: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The kind of entity an index/recall row refers to. The Context Engine treats tasks, events,
/// pages (and later people/goals) uniformly; this discriminates the polymorphic `entity_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    Task,
    Event,
    Page,
    Person,
    Goal,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Task => "task",
            EntityKind::Event => "event",
            EntityKind::Page => "page",
            EntityKind::Person => "person",
            EntityKind::Goal => "goal",
        }
    }

    pub fn from_str(s: &str) -> Option<EntityKind> {
        match s {
            "task" => Some(EntityKind::Task),
            "event" => Some(EntityKind::Event),
            "page" => Some(EntityKind::Page),
            "person" => Some(EntityKind::Person),
            "goal" => Some(EntityKind::Goal),
            _ => None,
        }
    }
}

/// The common currency of cross-entity recall: one ranked candidate, independent of source table.
/// `embedding` carries the raw LE-f32 bytes (None = not indexed); `score` is set by ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextItem {
    pub kind: EntityKind,
    pub id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(skip)]
    pub embedding: Option<Vec<u8>>,
}

/// A vault page — a Notion-style document with an Obsidian-style place in the page tree. Backed by
/// the same `notes` table as Hermes (so embeddings/recall keep working over `content`, the derived
/// plaintext). `content_json` is the BlockNote block array (None on legacy notes → rendered as a
/// plain paragraph doc). `indexed`/`score` mirror `Note`: `score` is set only on recall results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_json: Option<String>,
    pub sort_order: f64,
    pub archived: bool,
    /// Set when this page IS a calendar day's note ('YYYY-MM-DD'); None for normal pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_date: Option<String>,
    /// True while the page is an unsorted quick-capture in the Inbox.
    pub inbox: bool,
    pub created_at: String,
    pub updated_at: String,
    pub indexed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

/// A label — Pushin's flat, cross-cutting taxonomy applied to any entity (task/event/habit/page/
/// project), the layer above the rigid structural types. A label is "actionable" when it carries
/// scheduling prefs (a preferred time-of-day window, min/max block, batching) the scheduler honors;
/// all `pref_*` left empty = a purely organizational label. `count` is populated by `list_labels`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    pub archived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pref_window_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pref_window_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pref_min_chunk: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pref_max_chunk: Option<i64>,
    pub pref_batch: bool,
    pub created_at: String,
    /// How many entities carry this label (filled by `list_labels`; 0 elsewhere).
    #[serde(default)]
    pub count: i64,
}

/// Create/update payload for a label (no id / count / created_at).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelInput {
    pub name: String,
    pub color: String,
    pub icon: Option<String>,
    pub group_name: Option<String>,
    pub pref_window_start: Option<String>,
    pub pref_window_end: Option<String>,
    pub pref_min_chunk: Option<i64>,
    pub pref_max_chunk: Option<i64>,
    #[serde(default)]
    pub pref_batch: bool,
}

/// A reference from a page to another entity (a task or event) — the join that turns the calendar
/// into an index into the vault. The frontend resolves `id` to a title from its loaded store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityRef {
    pub kind: String, // "task" | "event"
    pub id: i64,
}

/// A markdown file found by the vault importer — its derived title + raw markdown. The frontend
/// converts the markdown to BlockNote blocks (so formatting survives) and creates the page.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDoc {
    pub title: String,
    pub markdown: String,
}

/// An answer from "ask your vault" (local RAG): the generated answer plus the page ids it cited.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAnswer {
    pub answer: String,
    pub citations: Vec<i64>,
}

/// One node in the vault connection graph (a page) plus its link degree (used to size the node).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<i64>,
    pub degree: u32,
}

/// A directed wikilink edge between two pages in the connection graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source: i64,
    pub target: i64,
}

/// The whole vault graph: every (non-archived) page and the resolved links between them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// A recurring personal commitment the scheduler must keep free — a bedtime routine, a
/// daily lunch, a standing gym slot, "no work after 6pm", etc. Times are wall-clock "HH:MM";
/// if `end` <= `start` the window runs overnight (e.g. 22:00→06:00). An empty `days` means
/// every day. `blocked` time and `routine` time are the same to the scheduler (both reserved);
/// `kind` is only a UI label.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commitment {
    pub id: String,
    pub name: String,
    pub start: String, // "HH:MM"
    pub end: String,   // "HH:MM"
    #[serde(default)]
    pub days: Vec<u8>, // 1=Mon .. 7=Sun; empty = every day
    #[serde(default)]
    pub kind: String, // "routine" | "blocked" (UI label only)
}

/// User settings; persisted as a single JSON row (key = "app").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub timezone: String,
    pub work_start: String,   // "09:00"
    pub work_end: String,     // "17:00"
    pub work_days: Vec<u8>,   // 1=Mon .. 7=Sun
    pub horizon_days: i64,
    pub buffer_minutes: i64,
    pub default_min_chunk: i64,
    pub default_max_chunk: i64,
    pub model_id: String,
    pub llm_base_url: String, // e.g. http://127.0.0.1:8080
    pub google_connected: bool,
    #[serde(default)]
    pub google_client_id: String,
    #[serde(default)]
    pub google_client_secret: String,

    // --- Personalization (collected by the first-run modal, editable in Settings) ---
    // All `#[serde(default)]` so existing settings rows upgrade cleanly: an old user gets
    // `onboarded=false` (sees the modal once) and `sleep_enabled=false` (no surprise blocking).
    /// Whether the first-run personalization modal has been completed/dismissed.
    #[serde(default)]
    pub onboarded: bool,
    /// Keep the user's sleep window free (and tell the LLM about it).
    #[serde(default)]
    pub sleep_enabled: bool,
    #[serde(default)]
    pub sleep_start: String, // bedtime, "HH:MM"
    #[serde(default)]
    pub sleep_end: String, // wake time, "HH:MM"
    /// Recurring blocked time / routines the scheduler plans around.
    #[serde(default)]
    pub commitments: Vec<Commitment>,

    /// Hermes (memory layer): the embedding model name sent to Pushin's managed embeddings server
    /// (`model_manager::embed_base_url()`), which is auto-downloaded and run on-device — no setup.
    /// Defaults to the bundled `EMBED_MODEL` (the request name is cosmetic to llama-server). Empty =
    /// semantic off (recall falls back to keyword search).
    #[serde(default = "default_embed_model")]
    pub embed_model: String,
    /// Folder the vault is mirrored to as markdown files (two-way Obsidian-style). Absolute path;
    /// None = no file vault yet (vault stays SQLite-only). Device-local — paths differ per machine.
    #[serde(default)]
    pub vault_dir: Option<String>,
    /// "About you" profile from setup: selected archetype keys + a free-form blurb. Fed into the AI's
    /// system prompt so it understands the user from day one (and grows from there).
    #[serde(default)]
    pub archetypes: Vec<String>,
    #[serde(default)]
    pub about_me: String,
}

impl Settings {
    /// A short user-profile blurb for the AI's system prompt (empty when nothing's filled in). Maps
    /// archetype keys to readable labels and appends the free-form "about me" text.
    pub fn profile_prompt(&self) -> String {
        let labels: Vec<&str> = self
            .archetypes
            .iter()
            .map(|k| match k.as_str() {
                "builder" => "a builder/founder",
                "student" => "a student",
                "creator" => "a creator",
                "operator" => "an operator/manager",
                "freelancer" => "a freelancer",
                "parent" => "a parent/caregiver",
                _ => "",
            })
            .filter(|s| !s.is_empty())
            .collect();
        let about = self.about_me.trim();
        if labels.is_empty() && about.is_empty() {
            return String::new();
        }
        let mut s = String::from("\n\nAbout the user");
        if !labels.is_empty() {
            s.push_str(&format!(" (they describe themselves as {})", labels.join(", ")));
        }
        s.push(':');
        if about.is_empty() {
            s.push('.');
        } else {
            s.push(' ');
            s.push_str(about);
        }
        s
    }
}

/// Keep in sync with `model_manager::EMBED_MODEL.id`.
fn default_embed_model() -> String {
    "bge-small-en-v1.5-q8_0".into()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            timezone: "local".into(),
            work_start: "09:00".into(),
            work_end: "17:00".into(),
            work_days: vec![1, 2, 3, 4, 5],
            horizon_days: 14,
            buffer_minutes: 0,
            default_min_chunk: 30,
            default_max_chunk: 120,
            // Default to the 7B: the 3B misroutes edits/recurrence and relative dates too often
            // (it's the documented reliability ceiling). The 7B is the "most reliable" model; users
            // on light hardware can still pick the 3B/1.5B in Settings. ~4.7GB first-run download.
            model_id: "qwen2.5-7b-instruct-q4_k_m".into(),
            llm_base_url: "http://127.0.0.1:8080".into(),
            google_connected: false,
            google_client_id: String::new(),
            google_client_secret: String::new(),
            onboarded: false,
            sleep_enabled: true,
            sleep_start: "23:00".into(),
            sleep_end: "07:00".into(),
            commitments: Vec::new(),
            embed_model: default_embed_model(),
            vault_dir: None,
            archetypes: Vec::new(),
            about_me: String::new(),
        }
    }
}

/// A scheduling conflict surfaced to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Conflict {
    #[serde(rename_all = "camelCase")]
    DependencyCycle { task_ids: Vec<i64> },
    #[serde(rename_all = "camelCase")]
    Unschedulable { task_id: i64, title: String, remaining_minutes: i64 },
    #[serde(rename_all = "camelCase")]
    DeadlineMiss { task_id: i64, title: String, scheduled_end: String, deadline: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleResult {
    pub blocks: Vec<Block>,
    pub conflicts: Vec<Conflict>,
}
