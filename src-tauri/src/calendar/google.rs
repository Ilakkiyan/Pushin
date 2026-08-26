//! Google Calendar two-way sync.
//!
//! - OAuth 2.0 + PKCE via the system browser and a loopback redirect (works with a
//!   "Desktop app" OAuth client — no server needed).
//! - Token refresh.
//! - Pull (incremental via `syncToken`) + push (full mirror of local events AND task
//!   blocks) against the user's PRIMARY calendar.
//!
//! Tokens currently live in the app-data SQLite (moving them to the OS keychain is a
//! hardening follow-up). Pushin-created block events are tagged with an
//! `extendedProperties.private` marker so they can be reconciled without duplicating.

use crate::db;
use crate::scheduler::{fmt_dt, parse_dt};
use anyhow::{anyhow, bail, Result};
use base64::Engine as _;
use chrono::{Duration, Local, TimeZone, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/calendar openid email";
const API: &str = "https://www.googleapis.com/calendar/v3";
const PUSHIN_KEY: &str = "pushinKind"; // extendedProperties.private marker on our block events
/// The mirrored block's `uuid` — its identity across every one of the user's devices. Lets the
/// mirror reconcile by row instead of deleting and recreating, so two devices syncing the same
/// account converge instead of deleting each other's block events.
const PUSHIN_REF: &str = "pushinRef";

/// The Calendar API base + token endpoint, overridable in tests so httpmock can stand in for Google.
/// Zero overhead in release (the `#[cfg(test)]` block is compiled out — it's just the const).
#[cfg(test)]
mod test_override {
    use std::sync::Mutex;
    pub static API: Mutex<Option<String>> = Mutex::new(None);
    pub static TOKEN: Mutex<Option<String>> = Mutex::new(None);
}
fn api_base() -> String {
    #[cfg(test)]
    if let Some(b) = test_override::API.lock().unwrap().clone() {
        return b;
    }
    API.to_string()
}
fn token_url() -> String {
    #[cfg(test)]
    if let Some(b) = test_override::TOKEN.lock().unwrap().clone() {
        return b;
    }
    TOKEN_URL.to_string()
}

// ---------------- OAuth (PKCE loopback) ----------------

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn pkce_verifier() -> String {
    let mut b = [0u8; 48];
    getrandom::getrandom(&mut b).expect("rng");
    b64url(&b)
}
fn pkce_challenge(v: &str) -> String {
    let mut h = Sha256::new();
    h.update(v.as_bytes());
    b64url(&h.finalize())
}

pub struct Connected {
    pub email: String,
    pub calendar_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub token_expiry: String, // RFC3339 UTC
}

/// Run the full OAuth consent flow: open the browser, catch the loopback redirect,
/// exchange the code for tokens, and resolve the account email.
pub async fn connect(app: &AppHandle, http: &reqwest::Client, client_id: &str, client_secret: &str) -> Result<Connected> {
    if client_id.trim().is_empty() {
        bail!("Add your Google OAuth Client ID in Settings first.");
    }
    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let url = reqwest::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect.as_str()),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )?;

    app.opener()
        .open_url(url.to_string(), None::<&str>)
        .map_err(|e| anyhow!("couldn't open the browser for Google sign-in: {e}"))?;

    let code = await_code(listener).await?;

    let resp: Value = http
        .post(TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let access = resp["access_token"].as_str().ok_or_else(|| anyhow!("no access_token returned"))?.to_string();
    let refresh = resp["refresh_token"]
        .as_str()
        .ok_or_else(|| anyhow!("no refresh_token returned — remove Pushin under your Google account's app permissions and reconnect"))?
        .to_string();
    let expires_in = resp["expires_in"].as_i64().unwrap_or(3600);
    let expiry = (Utc::now() + Duration::seconds(expires_in - 60)).to_rfc3339();

    // The primary calendar's id is the account email.
    let cal: Value = http
        .get(format!("{API}/calendars/primary"))
        .bearer_auth(&access)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let email = cal["id"].as_str().unwrap_or("primary").to_string();

    Ok(Connected {
        email,
        calendar_id: "primary".into(),
        access_token: access,
        refresh_token: refresh,
        token_expiry: expiry,
    })
}

/// Wait (up to 3 min) for Google to redirect to our loopback listener with the auth code.
async fn await_code(listener: TcpListener) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let target = req.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("");
                let parsed = reqwest::Url::parse(&format!("http://localhost{target}")).ok();
                let code = parsed
                    .as_ref()
                    .and_then(|u| u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()));
                let denied = parsed
                    .as_ref()
                    .map(|u| u.query_pairs().any(|(k, _)| k == "error"))
                    .unwrap_or(false);

                let body = "<html><body style=\"font-family:system-ui;background:#0b0d12;color:#e5e7eb;text-align:center;padding-top:80px\"><h2>📌 Pushin is connected to Google Calendar.</h2><p>You can close this tab.</p></body></html>";
                let _ = stream.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes(),
                );
                if let Some(c) = code {
                    return Ok(c);
                }
                if denied {
                    bail!("Google authorization was denied.");
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    bail!("timed out waiting for Google authorization");
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

async fn refresh_access(http: &reqwest::Client, client_id: &str, client_secret: &str, refresh_token: &str) -> Result<(String, String)> {
    let resp: Value = http
        .post(token_url())
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let access = resp["access_token"].as_str().ok_or_else(|| anyhow!("no access_token on refresh"))?.to_string();
    let expires_in = resp["expires_in"].as_i64().unwrap_or(3600);
    let expiry = (Utc::now() + Duration::seconds(expires_in - 60)).to_rfc3339();
    Ok((access, expiry))
}

fn token_expired(expiry_iso: &Option<String>) -> bool {
    match expiry_iso.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()) {
        Some(exp) => Utc::now() >= exp.with_timezone(&Utc),
        None => true,
    }
}

// ---------------- Calendar API ----------------

#[derive(Debug)]
struct GEvent {
    id: String,
    summary: String,
    start: Option<String>, // naive-local ISO
    end: Option<String>,
    cancelled: bool,
    etag: Option<String>,
    is_pushin: bool,
    /// For a mirrored block event, the source block's `uuid` (absent on events written by a build
    /// before the mirror was uuid-keyed — those are treated as stale and cleaned up).
    pushin_ref: Option<String>,
}

/// Parse a Google start/end object ({dateTime,timeZone} or {date}) to a naive-local ISO.
fn parse_g_time(v: &Value) -> Option<String> {
    if let Some(dt) = v["dateTime"].as_str() {
        return parse_dt(dt).map(fmt_dt);
    }
    if let Some(d) = v["date"].as_str() {
        return parse_dt(d).map(fmt_dt);
    }
    None
}

/// Convert our naive-local ISO to RFC3339 with the machine's current offset (Google needs a zone).
fn to_rfc3339(iso: &str) -> String {
    parse_dt(iso)
        .and_then(|n| Local.from_local_datetime(&n).single())
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| iso.to_string())
}

fn event_body(title: &str, start: &str, end: &str, pushin_kind: Option<&str>, pushin_ref: Option<&str>) -> Value {
    let mut body = json!({
        "summary": title,
        "start": { "dateTime": to_rfc3339(start) },
        "end": { "dateTime": to_rfc3339(end) },
    });
    if let Some(kind) = pushin_kind {
        let mut private = json!({ PUSHIN_KEY: kind });
        if let Some(r) = pushin_ref {
            private[PUSHIN_REF] = json!(r);
        }
        body["extendedProperties"] = json!({ "private": private });
    }
    body
}

async fn list_events(
    http: &reqwest::Client,
    access: &str,
    cal_id: &str,
    time_min: &str,
    time_max: &str,
    sync_token: Option<&str>,
) -> Result<(Vec<GEvent>, Option<String>)> {
    let base = api_base();
    let mut all = Vec::new();
    let mut page: Option<String> = None;
    let mut next_sync: Option<String> = None;
    loop {
        let mut q: Vec<(String, String)> = vec![
            ("singleEvents".into(), "true".into()),
            ("showDeleted".into(), "true".into()),
            ("maxResults".into(), "250".into()),
        ];
        match sync_token {
            Some(t) => q.push(("syncToken".into(), t.to_string())),
            None => {
                q.push(("timeMin".into(), time_min.to_string()));
                q.push(("timeMax".into(), time_max.to_string()));
            }
        }
        if let Some(p) = &page {
            q.push(("pageToken".into(), p.clone()));
        }
        let url = reqwest::Url::parse_with_params(&format!("{base}/calendars/{cal_id}/events"), &q)?;
        let resp = http.get(url).bearer_auth(access).send().await?;
        if resp.status().as_u16() == 410 {
            bail!("SYNC_TOKEN_EXPIRED");
        }
        let v: Value = resp.error_for_status()?.json().await?;
        for item in v["items"].as_array().cloned().unwrap_or_default() {
            let id = item["id"].as_str().unwrap_or("").to_string();
            if id.is_empty() {
                continue;
            }
            all.push(GEvent {
                id,
                summary: item["summary"].as_str().unwrap_or("(busy)").to_string(),
                start: parse_g_time(&item["start"]),
                end: parse_g_time(&item["end"]),
                cancelled: item["status"].as_str() == Some("cancelled"),
                etag: item["etag"].as_str().map(String::from),
                is_pushin: item["extendedProperties"]["private"][PUSHIN_KEY].is_string(),
                pushin_ref: item["extendedProperties"]["private"][PUSHIN_REF]
                    .as_str()
                    .map(String::from),
            });
        }
        next_sync = v["nextSyncToken"].as_str().map(String::from).or(next_sync);
        match v["nextPageToken"].as_str() {
            Some(p) => page = Some(p.to_string()),
            None => break,
        }
    }
    Ok((all, next_sync))
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(http: &reqwest::Client, access: &str, cal_id: &str, title: &str, start: &str, end: &str, kind: Option<&str>, reference: Option<&str>) -> Result<(String, Option<String>)> {
    let v: Value = http
        .post(format!("{}/calendars/{cal_id}/events", api_base()))
        .bearer_auth(access)
        .json(&event_body(title, start, end, kind, reference))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok((v["id"].as_str().unwrap_or_default().to_string(), v["etag"].as_str().map(String::from)))
}

async fn patch_event(http: &reqwest::Client, access: &str, cal_id: &str, ext_id: &str, title: &str, start: &str, end: &str) -> Result<Option<String>> {
    let resp = http
        .patch(format!("{}/calendars/{cal_id}/events/{ext_id}", api_base()))
        .bearer_auth(access)
        .json(&event_body(title, start, end, None, None))
        .send()
        .await?;
    // 404/410 → the event vanished on Google; not fatal.
    if resp.status().as_u16() == 404 || resp.status().as_u16() == 410 {
        return Ok(None);
    }
    let v: Value = resp.error_for_status()?.json().await?;
    Ok(v["etag"].as_str().map(String::from))
}

async fn delete_event(http: &reqwest::Client, access: &str, cal_id: &str, ext_id: &str) -> Result<()> {
    let resp = http.delete(format!("{}/calendars/{cal_id}/events/{ext_id}", api_base())).bearer_auth(access).send().await?;
    let code = resp.status().as_u16();
    if !resp.status().is_success() && code != 404 && code != 410 {
        bail!("Google delete failed ({code})");
    }
    Ok(())
}

// ---------------- Multi-device reconcilers ----------------
//
// Every paired device syncs with the SAME Google calendar (device sync replicates events and
// blocks, and the shared `google_link` hands each device the same account). Both of the deterministic
// planners below exist so that concurrency between devices is safe: they are pure functions of
// (local rows, what Google currently holds), so two devices running them converge on the same
// calendar instead of duplicating or deleting each other's work. Unit-tested below.

/// Match a local event that has no `external_id` yet against what Google already holds.
///
/// Closes the multi-device push race: device A creates an event and pushes it; device B may receive
/// the event over the P2P mesh *before* A's `external_id` reaches it, and would then insert a second
/// copy into Google. Adopting an exact title+start+end match instead makes that a no-op. `used`
/// carries ids already claimed earlier in this pass so two local rows can't adopt one Google event.
fn adopt_existing(
    title: &str,
    start: &str,
    end: &str,
    window: &[GEvent],
    used: &mut std::collections::HashSet<String>,
) -> Option<String> {
    window
        .iter()
        .find(|g| {
            !g.is_pushin
                && !g.cancelled
                && g.summary == title
                && g.start.as_deref() == Some(start)
                && g.end.as_deref() == Some(end)
                && !used.contains(&g.id)
        })
        .map(|g| {
            used.insert(g.id.clone());
            g.id.clone()
        })
}

/// One block we want mirrored onto Google, keyed by the block's cross-device `uuid`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MirrorTarget {
    uuid: String,
    title: String,
    start: String,
    end: String,
}

/// A single step of the block mirror.
#[derive(Debug, PartialEq, Eq)]
enum MirrorOp {
    /// Create the mirrored event for `targets[i]`.
    Insert(usize),
    /// Update the Google event `ext_id` to match `targets[i]`.
    Patch { ext_id: String, target: usize },
    /// Remove a mirrored event that no longer corresponds to a live block.
    Delete(String),
}

/// Diff the blocks we want mirrored against the mirrored events Google currently holds.
///
/// Replaces the old "delete every Pushin block event, then recreate them all" mirror, which two
/// devices could not run concurrently without deleting each other's work. Keyed by block `uuid`
/// (which device sync replicates), this is idempotent: a second run over an already-correct calendar
/// plans nothing, and a run that finds two events for one block deletes the extra — so even a true
/// insert race self-heals on the next sync.
fn plan_block_mirror(targets: &[MirrorTarget], existing: &[GEvent]) -> Vec<MirrorOp> {
    use std::collections::HashMap;
    let mut by_ref: HashMap<&str, Vec<&GEvent>> = HashMap::new();
    let mut ops = Vec::new();
    for g in existing.iter().filter(|g| g.is_pushin && !g.cancelled) {
        match g.pushin_ref.as_deref() {
            // Written before the mirror was uuid-keyed: no way to match it to a block, so retire it.
            None => ops.push(MirrorOp::Delete(g.id.clone())),
            Some(r) => by_ref.entry(r).or_default().push(g),
        }
    }
    for (i, t) in targets.iter().enumerate() {
        match by_ref.remove(t.uuid.as_str()) {
            None => ops.push(MirrorOp::Insert(i)),
            Some(mut group) => {
                // Deterministic winner so both devices keep the same copy of a raced double-insert.
                group.sort_by(|a, b| a.id.cmp(&b.id));
                let keep = group.remove(0);
                for dup in group {
                    ops.push(MirrorOp::Delete(dup.id.clone()));
                }
                let stale = keep.summary != t.title
                    || keep.start.as_deref() != Some(t.start.as_str())
                    || keep.end.as_deref() != Some(t.end.as_str());
                if stale {
                    ops.push(MirrorOp::Patch { ext_id: keep.id.clone(), target: i });
                }
            }
        }
    }
    // Whatever is left points at a block that no longer exists.
    let mut orphans: Vec<String> =
        by_ref.into_values().flatten().map(|g| g.id.clone()).collect();
    orphans.sort();
    ops.extend(orphans.into_iter().map(MirrorOp::Delete));
    ops
}

// ---------------- Sync engine ----------------

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    pub pulled: usize,
    pub pushed: usize,
    pub removed: usize,
    pub blocks_mirrored: usize,
}

/// Run a full two-way sync. Pulls Google → Pushin (incremental), then pushes local
/// events + task blocks → Google (primary calendar). Returns a small summary.
pub async fn sync(db_mutex: &Mutex<Connection>, http: &reqwest::Client) -> Result<SyncSummary> {
    // 1. Snapshot account + credentials.
    let (account, settings) = {
        let conn = db_mutex.lock().unwrap();
        (db::get_google_account(&conn)?, db::get_settings(&conn)?)
    };
    let account = account.ok_or_else(|| anyhow!("Google Calendar isn't connected."))?;
    let cal_id = account.calendar_id.clone();
    let refresh = account.refresh_token.clone().ok_or_else(|| anyhow!("missing refresh token — reconnect Google"))?;

    // 2. Ensure a valid access token.
    let access = if token_expired(&account.token_expiry) {
        let (a, exp) = refresh_access(http, &settings.google_client_id, &settings.google_client_secret, &refresh).await?;
        let conn = db_mutex.lock().unwrap();
        db::update_google_tokens(&conn, account.id, &a, &exp)?;
        a
    } else {
        account.access_token.clone().ok_or_else(|| anyhow!("missing access token — reconnect Google"))?
    };

    let mut summary = SyncSummary::default();
    let now = Utc::now();
    let time_min = (now - Duration::days(1)).to_rfc3339();
    let time_max = (now + Duration::days(settings.horizon_days.max(7))).to_rfc3339();

    // 3. PULL (incremental; fall back to a full window if the sync token expired).
    let (events, next_sync) = match list_events(http, &access, &cal_id, &time_min, &time_max, account.sync_token.as_deref()).await {
        Ok(r) => r,
        Err(e) if e.to_string().contains("SYNC_TOKEN_EXPIRED") => {
            {
                let conn = db_mutex.lock().unwrap();
                db::update_google_sync_token(&conn, account.id, None)?;
            }
            list_events(http, &access, &cal_id, &time_min, &time_max, None).await?
        }
        Err(e) => return Err(e),
    };
    {
        let conn = db_mutex.lock().unwrap();
        for ev in &events {
            if ev.is_pushin {
                continue; // our own block events — don't pull them back in as events
            }
            if ev.cancelled {
                db::delete_events_by_external(&conn, &ev.id)?;
                summary.removed += 1;
                continue;
            }
            let (Some(start), Some(end)) = (ev.start.clone(), ev.end.clone()) else { continue };
            match db::find_event_by_external(&conn, &ev.id)? {
                Some(local) => db::update_event_synced(&conn, local.id, &ev.summary, &start, &end, ev.etag.as_deref())?,
                None => {
                    db::insert_google_event(&conn, &ev.summary, &start, &end, &ev.id, ev.etag.as_deref())?;
                }
            }
            summary.pulled += 1;
        }
        db::update_google_sync_token(&conn, account.id, next_sync.as_deref())?;
    }

    // 4. Snapshot the whole window once. Both push phases below reconcile against it rather than
    //    assuming this device is the only one writing to the calendar (see the reconcilers above).
    let (window, _) = list_events(http, &access, &cal_id, &time_min, &time_max, None).await?;

    // 5. PUSH local events (source = manual) → Google.
    let to_push: Vec<(i64, String, String, String, Option<String>)> = {
        let conn = db_mutex.lock().unwrap();
        db::list_events(&conn)?
            .into_iter()
            .filter(|e| e.source == "manual")
            .map(|e| (e.id, e.title, e.start, e.end, e.external_id))
            .collect()
    };
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (id, title, start, end, ext) in to_push {
        // An event a peer pushed already exists on Google; adopt its id instead of duplicating it.
        let ext = ext.or_else(|| adopt_existing(&title, &start, &end, &window, &mut claimed));
        match ext {
            None => {
                let (gid, etag) = insert_event(http, &access, &cal_id, &title, &start, &end, None, None).await?;
                let conn = db_mutex.lock().unwrap();
                db::mark_event_pushed(&conn, id, &gid, etag.as_deref())?;
            }
            Some(gid) => {
                let etag = patch_event(http, &access, &cal_id, &gid, &title, &start, &end).await?;
                let conn = db_mutex.lock().unwrap();
                db::mark_event_pushed(&conn, id, &gid, etag.as_deref())?;
            }
        }
        summary.pushed += 1;
    }

    // 6. MIRROR task blocks, reconciling by block uuid so concurrent devices converge.
    let targets: Vec<MirrorTarget> = {
        let conn = db_mutex.lock().unwrap();
        let titles: std::collections::HashMap<i64, String> = db::list_tasks(&conn)?.into_iter().map(|t| (t.id, t.title)).collect();
        db::list_blocks_with_uuid(&conn)?
            .into_iter()
            .map(|(uuid, b)| MirrorTarget {
                uuid,
                title: titles.get(&b.task_id).cloned().unwrap_or_else(|| "Focus".into()),
                start: b.start,
                end: b.end,
            })
            .collect()
    };
    for op in plan_block_mirror(&targets, &window) {
        match op {
            MirrorOp::Insert(i) => {
                let t = &targets[i];
                insert_event(http, &access, &cal_id, &t.title, &t.start, &t.end, Some("block"), Some(&t.uuid)).await?;
                summary.blocks_mirrored += 1;
            }
            MirrorOp::Patch { ext_id, target } => {
                let t = &targets[target];
                patch_event(http, &access, &cal_id, &ext_id, &t.title, &t.start, &t.end).await?;
                summary.blocks_mirrored += 1;
            }
            MirrorOp::Delete(ext_id) => delete_event(http, &access, &cal_id, &ext_id).await?,
        }
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gev(id: &str, summary: &str, start: &str, end: &str) -> GEvent {
        GEvent {
            id: id.into(),
            summary: summary.into(),
            start: Some(start.into()),
            end: Some(end.into()),
            cancelled: false,
            etag: None,
            is_pushin: false,
            pushin_ref: None,
        }
    }
    /// A mirrored block event as it comes back from Google.
    fn mirrored(id: &str, block_uuid: &str, summary: &str, start: &str, end: &str) -> GEvent {
        GEvent { is_pushin: true, pushin_ref: Some(block_uuid.into()), ..gev(id, summary, start, end) }
    }
    fn target(uuid: &str, title: &str, start: &str, end: &str) -> MirrorTarget {
        MirrorTarget { uuid: uuid.into(), title: title.into(), start: start.into(), end: end.into() }
    }

    const S: &str = "2026-06-14T09:00:00";
    const E: &str = "2026-06-14T10:00:00";

    #[test]
    fn block_mirror_is_a_no_op_when_google_already_matches() {
        // The property that makes concurrent multi-device sync safe: a device that finds the
        // calendar already correct must not touch it (the old mirror deleted + recreated blindly).
        let targets = vec![target("u1", "Write essay", S, E)];
        let existing = vec![mirrored("g1", "u1", "Write essay", S, E)];
        assert_eq!(plan_block_mirror(&targets, &existing), vec![]);
    }

    #[test]
    fn block_mirror_inserts_patches_and_removes_orphans() {
        let targets = vec![
            target("u1", "Write essay", S, E),                       // already mirrored, unchanged
            target("u2", "Renamed", S, E),                           // mirrored under an old title
            target("u3", "Brand new", S, E),                         // never mirrored
        ];
        let existing = vec![
            mirrored("g1", "u1", "Write essay", S, E),
            mirrored("g2", "u2", "Old title", S, E),
            mirrored("g3", "u-gone", "Deleted block", S, E),         // block no longer exists
        ];
        let ops = plan_block_mirror(&targets, &existing);
        assert!(ops.contains(&MirrorOp::Insert(2)), "u3 gets created: {ops:?}");
        assert!(
            ops.contains(&MirrorOp::Patch { ext_id: "g2".into(), target: 1 }),
            "u2 is patched in place, not recreated: {ops:?}"
        );
        assert!(ops.contains(&MirrorOp::Delete("g3".into())), "orphan is removed: {ops:?}");
        assert_eq!(ops.len(), 3, "u1 is left alone: {ops:?}");
    }

    #[test]
    fn block_mirror_heals_a_raced_double_insert_and_retires_unreferenced_events() {
        // Two devices mirroring the same block at the same instant can both insert. The next run
        // must converge on ONE event — deterministically the same one on either device.
        let targets = vec![target("u1", "Write essay", S, E)];
        let existing = vec![
            mirrored("g_b", "u1", "Write essay", S, E),
            mirrored("g_a", "u1", "Write essay", S, E),
            GEvent { is_pushin: true, ..gev("legacy", "Old block event", S, E) }, // pre-uuid marker
        ];
        let ops = plan_block_mirror(&targets, &existing);
        assert!(ops.contains(&MirrorOp::Delete("g_b".into())), "keeps the lowest id: {ops:?}");
        assert!(ops.contains(&MirrorOp::Delete("legacy".into())), "retires unkeyed: {ops:?}");
        assert!(!ops.iter().any(|o| matches!(o, MirrorOp::Insert(_))), "no new copy: {ops:?}");
        assert!(!ops.contains(&MirrorOp::Delete("g_a".into())), "survivor is kept: {ops:?}");
    }

    #[test]
    fn adopt_existing_claims_a_peers_event_instead_of_duplicating_it() {
        // Device A pushed "Standup"; we received it over P2P before its external_id arrived.
        let window = vec![gev("gA", "Standup", S, E)];
        let mut used = std::collections::HashSet::new();
        assert_eq!(adopt_existing("Standup", S, E, &window, &mut used).as_deref(), Some("gA"));
        // A second local row with the same title must NOT claim the same Google event.
        assert_eq!(adopt_existing("Standup", S, E, &window, &mut used), None);
    }

    #[test]
    fn adopt_existing_ignores_non_matches_block_mirrors_and_cancellations() {
        let mut used = std::collections::HashSet::new();
        let shifted = vec![gev("gA", "Standup", "2026-06-14T11:00:00", E)];
        assert_eq!(adopt_existing("Standup", S, E, &shifted, &mut used), None, "time must match too");

        let ours = vec![mirrored("gB", "u1", "Standup", S, E)];
        assert_eq!(adopt_existing("Standup", S, E, &ours, &mut used), None, "never adopt our own block");

        let gone = vec![GEvent { cancelled: true, ..gev("gC", "Standup", S, E) }];
        assert_eq!(adopt_existing("Standup", S, E, &gone, &mut used), None, "never adopt a cancellation");
    }

    #[test]
    fn pkce_s256_matches_rfc7636_vector() {
        // RFC 7636 Appendix B: verifier → S256 challenge = base64url(sha256(verifier)), no padding.
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(pkce_challenge(v), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_verifier_is_url_safe_long_and_random() {
        let v = pkce_verifier();
        assert!(v.len() >= 43, "RFC 7636 requires 43..128 chars");
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "url-safe, unpadded");
        assert_ne!(pkce_verifier(), pkce_verifier(), "fresh randomness each call");
    }

    // One serial test owns the global API/TOKEN override and drives the Calendar leaf functions
    // against a mocked Google API — covering token refresh, incremental pull + 410 fallback, and the
    // push verbs. (The full `sync()` orchestrator additionally needs a seeded account/token in the DB
    // + keychain; that end-to-end test is a documented follow-up.)
    #[tokio::test]
    async fn calendar_api_against_mocked_google() {
        use httpmock::prelude::*;
        let server = MockServer::start_async().await;
        *test_override::API.lock().unwrap() = Some(server.base_url());
        *test_override::TOKEN.lock().unwrap() = Some(format!("{}/token", server.base_url()));
        let http = reqwest::Client::new();

        // --- token refresh ---
        server.mock(|w, t| {
            w.method(POST).path("/token");
            t.status(200).json_body(serde_json::json!({ "access_token": "acc", "expires_in": 3600 }));
        });
        let (acc, _expiry) = refresh_access(&http, "id", "sec", "refresh").await.unwrap();
        assert_eq!(acc, "acc");

        // --- incremental pull: parses items + nextSyncToken ---
        server.mock(|w, t| {
            w.method(GET).path("/calendars/primary/events").query_param_exists("timeMin");
            t.status(200).json_body(serde_json::json!({
                "items": [ { "id": "e1", "summary": "Lunch", "status": "confirmed",
                             "start": { "dateTime": "2026-06-14T12:00:00Z" }, "end": { "dateTime": "2026-06-14T13:00:00Z" } } ],
                "nextSyncToken": "tok123"
            }));
        });
        let (evs, next) = list_events(&http, "acc", "primary", "2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z", None).await.unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "Lunch");
        assert_eq!(next.as_deref(), Some("tok123"));

        // --- 410 on a stale sync token → SYNC_TOKEN_EXPIRED (drives the full-window refetch) ---
        let stale = server.mock(|w, t| {
            w.method(GET).path("/calendars/primary/events").query_param("syncToken", "stale");
            t.status(410);
        });
        let err = list_events(&http, "acc", "primary", "x", "y", Some("stale")).await.unwrap_err();
        assert!(err.to_string().contains("SYNC_TOKEN_EXPIRED"));
        stale.assert();

        // --- push: insert returns (id, etag) ---
        server.mock(|w, t| {
            w.method(POST).path("/calendars/primary/events");
            t.status(200).json_body(serde_json::json!({ "id": "new1", "etag": "\"abc\"" }));
        });
        let (id, etag) = insert_event(&http, "acc", "primary", "Block", "2026-06-14T09:00:00", "2026-06-14T10:00:00", Some("block"), Some("blk-uuid")).await.unwrap();
        assert_eq!(id, "new1");
        assert_eq!(etag.as_deref(), Some("\"abc\""));

        // --- patch tolerates a vanished event (404 → None) ---
        server.mock(|w, t| {
            w.method(httpmock::Method::PATCH).path("/calendars/primary/events/gone");
            t.status(404);
        });
        assert!(patch_event(&http, "acc", "primary", "gone", "T", "s", "e").await.unwrap().is_none());

        // --- delete tolerates 404/410 ---
        server.mock(|w, t| {
            w.method(DELETE).path("/calendars/primary/events/x");
            t.status(404);
        });
        assert!(delete_event(&http, "acc", "primary", "x").await.is_ok());

        *test_override::API.lock().unwrap() = None;
        *test_override::TOKEN.lock().unwrap() = None;
    }
}
