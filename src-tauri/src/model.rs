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
            model_id: "qwen2.5-3b-instruct-q4_k_m".into(),
            llm_base_url: "http://127.0.0.1:8080".into(),
            google_connected: false,
            google_client_id: String::new(),
            google_client_secret: String::new(),
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
