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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Booking {
    pub id: i64,
    pub event_type_id: i64,
    pub invitee_name: String,
    pub invitee_email: String,
    pub start: String,
    pub end: String,
    pub status: String,
    pub created_at: String,
}

/// A tracked habit. `cadence` is "daily" for now (room to grow to weekly targets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub cadence: String,
    pub duration_minutes: i64,
    pub archived: bool,
    pub created_at: String,
}

/// One day in a habit's history (for the consistency heatmap). `day` is "YYYY-MM-DD".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitDay {
    pub day: String,
    pub done: bool,
}

/// A habit plus the derived streak/consistency metrics the UI renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitStats {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub cadence: String,
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
