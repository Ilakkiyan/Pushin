use crate::calendar::google;
use crate::schedule_service::reschedule_inner;
use crate::{booking, db};
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 47610;

/// Hard cap on a request body. A booking JSON is a few hundred bytes; anything
/// larger is abuse. Without this the body-read loop grows an unbounded `Vec`
/// straight from an attacker-controlled `Content-Length` (memory-exhaustion DoS).
const MAX_BODY: usize = 64 * 1024;
/// Whole-request budget. The per-read timeout alone does not stop a slowloris
/// that dribbles a byte just inside each timeout; this caps total time on a socket.
const REQUEST_DEADLINE: Duration = Duration::from_secs(8);
/// Per-read socket timeout.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Upper bound on connections handled at once. Each connection gets its own
/// thread so one slow client can't stall the accept loop; this bounds the blast
/// radius of many slow clients (thread-exhaustion DoS).
const MAX_INFLIGHT: usize = 64;
/// Global booking rate limit: at most this many `POST /book` in the window.
/// Through a tunnel every request appears to come from 127.0.0.1, so per-IP
/// limiting is meaningless — the limit must be global. Protects the calendar
/// from flooding and the Google sync from amplification.
const BOOK_LIMIT: usize = 8;
const BOOK_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookingServerStatus {
    pub running: bool,
    pub local_url: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

pub struct BookingServerHandle {
    status: BookingServerStatus,
    shutdown: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl BookingServerHandle {
    pub fn status(&self) -> BookingServerStatus {
        self.status.clone()
    }

    pub fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = self.thread.join();
    }
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct Response {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookRequest {
    name: String,
    email: String,
    start: String,
    end: String,
}

/// Fixed-window counter for booking attempts. Global (see `BOOK_LIMIT`).
#[derive(Default)]
struct RateLimiter {
    hits: VecDeque<Instant>,
}

impl RateLimiter {
    /// Records an attempt and returns whether it is allowed under the window.
    fn allow(&mut self, now: Instant, limit: usize, window: Duration) -> bool {
        while let Some(front) = self.hits.front() {
            if now.duration_since(*front) >= window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        if self.hits.len() >= limit {
            return false;
        }
        self.hits.push_back(now);
        true
    }
}

pub fn stopped_status() -> BookingServerStatus {
    BookingServerStatus {
        running: false,
        local_url: None,
        host: "127.0.0.1".into(),
        port: None,
    }
}

pub fn start(db: Arc<Mutex<Connection>>, http: reqwest::Client, requested_port: Option<u16>) -> Result<BookingServerHandle> {
    let (listener, port) = bind_listener(requested_port.unwrap_or(DEFAULT_PORT))?;
    listener.set_nonblocking(true)?;
    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    let status = BookingServerStatus {
        running: true,
        local_url: Some(format!("http://127.0.0.1:{port}")),
        host: "127.0.0.1".into(),
        port: Some(port),
    };
    let thread = thread::spawn(move || run(listener, shutdown_rx, db, http));
    Ok(BookingServerHandle { status, shutdown: shutdown_tx, thread })
}

fn bind_listener(preferred: u16) -> Result<(TcpListener, u16)> {
    for offset in 0..=20 {
        let port = preferred.saturating_add(offset);
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return Ok((listener, port));
        }
    }
    Err(anyhow!("could not bind booking server near port {preferred}"))
}

fn run(listener: TcpListener, shutdown_rx: mpsc::Receiver<()>, db: Arc<Mutex<Connection>>, http: reqwest::Client) {
    let limiter = Arc::new(Mutex::new(RateLimiter::default()));
    let inflight = Arc::new(AtomicUsize::new(0));
    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Shed load rather than spawning unbounded threads under a flood.
                if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
                    let _ = write_response(&mut stream, response_text(503, "Service Unavailable", "text/plain; charset=utf-8", "Busy"));
                    continue;
                }
                inflight.fetch_add(1, Ordering::Relaxed);
                let (db, http, limiter, inflight_c) = (Arc::clone(&db), http.clone(), Arc::clone(&limiter), Arc::clone(&inflight));
                // One thread per connection: a slow client blocks only its own thread,
                // never the accept loop or other bookings.
                thread::spawn(move || {
                    let response = match read_request(&mut stream) {
                        Ok(req) => route(req, &db, &http, &limiter),
                        Err(_) => response_text(400, "Bad Request", "text/plain; charset=utf-8", "Bad request"),
                    };
                    let _ = write_response(&mut stream, response);
                    inflight_c.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(35)),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut header_end = None;
    while header_end.is_none() && buf.len() < MAX_BODY {
        if Instant::now() >= deadline {
            return Err(anyhow!("request header timeout"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        header_end = find_header_end(&buf);
    }
    let header_end = header_end.ok_or_else(|| anyhow!("missing request headers"))?;
    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.lines();
    let first = lines.next().ok_or_else(|| anyhow!("missing request line"))?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    // Reject an oversized declared body up front — never allocate toward it.
    if content_length > MAX_BODY {
        return Err(anyhow!("request body too large"));
    }

    let body_start = header_end + 4;
    let mut body = buf.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        if Instant::now() >= deadline {
            return Err(anyhow!("request body timeout"));
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > MAX_BODY {
            return Err(anyhow!("request body too large"));
        }
    }
    body.truncate(content_length);

    let (path, query) = split_target(&target);
    Ok(Request { method, path, query, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn split_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|part| {
            let (k, v) = part.split_once('=').unwrap_or((part, ""));
            Some((percent_decode(k).ok()?, percent_decode(v).ok()?))
        })
        .collect();
    (path.to_string(), query)
}

fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3])?;
                out.push(u8::from_str_radix(hex, 16)?);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(String::from_utf8(out)?)
}

/// Lock the shared database, surviving a poisoned mutex.
///
/// This is the *same* `Arc<Mutex<Connection>>` the app commands use, and `AppState::db()` already
/// recovers from poison for exactly this reason: `lock().unwrap()` panics forever once any thread
/// has panicked while holding the lock. The booking server sat on the other side of that same mutex
/// still calling `.unwrap()`, so one panic anywhere in the app permanently 500'd every public
/// booking page — a stranger's link silently stopped working with no sign inside the app. SQLite
/// rolls back its own transaction, so the connection is still consistent; recovering loses at most
/// the operation that panicked.
fn lock_db(db: &Arc<Mutex<Connection>>) -> std::sync::MutexGuard<'_, Connection> {
    db.lock().unwrap_or_else(|e| e.into_inner())
}

fn route(req: Request, db: &Arc<Mutex<Connection>>, http: &reqwest::Client, limiter: &Arc<Mutex<RateLimiter>>) -> Response {
    let parts: Vec<&str> = req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), parts.as_slice()) {
        ("GET", ["b", token, slug]) => public_page(db, token, slug),
        ("GET", ["api", "b", token, slug, "slots"]) => public_slots(db, token, slug, &req.query),
        ("POST", ["api", "b", token, slug, "book"]) => {
            // Same poison-tolerance as the DB lock: a wedged limiter must not take the public
            // page down with it.
            if !limiter.lock().unwrap_or_else(|e| e.into_inner()).allow(Instant::now(), BOOK_LIMIT, BOOK_WINDOW) {
                return json_response(429, "Too Many Requests", json!({ "error": "Too many booking attempts. Please try again shortly." }));
            }
            public_book(db, http, token, slug, &req.body)
        }
        ("GET", []) => response_text(200, "OK", "text/plain; charset=utf-8", "Pushin booking server"),
        _ => response_text(404, "Not Found", "text/plain; charset=utf-8", "Not found"),
    }
}

fn public_page(db: &Arc<Mutex<Connection>>, token: &str, slug: &str) -> Response {
    let conn = lock_db(db);
    match db::public_event_type(&conn, token, slug) {
        Ok(Some(et)) => response_text(200, "OK", "text/html; charset=utf-8", &booking_html(token, slug, &et)),
        _ => response_text(404, "Not Found", "text/html; charset=utf-8", &not_found_html()),
    }
}

fn public_slots(db: &Arc<Mutex<Connection>>, token: &str, slug: &str, query: &HashMap<String, String>) -> Response {
    let conn = lock_db(db);
    let Ok(Some(et)) = db::public_event_type(&conn, token, slug) else {
        return json_response(404, "Not Found", json!({ "error": "Not found" }));
    };
    let horizon = query.get("horizonDays").and_then(|v| v.parse::<i64>().ok()).unwrap_or(14).clamp(1, 60);
    let settings = match db::get_settings(&conn) {
        Ok(s) => s,
        Err(_) => return json_response(500, "Internal Server Error", json!({ "error": "Could not load availability" })),
    };
    match booking::available_slots(&conn, &settings, &et, horizon) {
        Ok(slots) => json_response(200, "OK", json!({ "eventType": booking::public_event_type(&et), "slots": slots })),
        Err(_) => json_response(500, "Internal Server Error", json!({ "error": "Could not load availability" })),
    }
}

fn public_book(db: &Arc<Mutex<Connection>>, http: &reqwest::Client, token: &str, slug: &str, body: &[u8]) -> Response {
    let Ok(input) = serde_json::from_slice::<BookRequest>(body) else {
        return json_response(400, "Bad Request", json!({ "error": "Check the booking details and try again." }));
    };
    let (result, should_sync) = {
        let mut conn = lock_db(db);
        let et = match db::public_event_type(&conn, token, slug) {
            Ok(Some(et)) => et,
            _ => return json_response(404, "Not Found", json!({ "error": "Not found" })),
        };
        let settings = match db::get_settings(&conn) {
            Ok(settings) => settings,
            Err(_) => return json_response(500, "Internal Server Error", json!({ "error": "Could not create booking" })),
        };
        let should_sync = settings.google_connected;
        let result = booking::confirm_booking(&mut conn, &settings, &et, &input.name, &input.email, &input.start, &input.end)
            .and_then(|_| reschedule_inner(&mut conn, &settings).map(|_| ()));
        (result, should_sync)
    };
    match result {
        Ok(()) => {
            if should_sync {
                let db = Arc::clone(db);
                let http = http.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = google::sync(db.as_ref(), &http).await;
                });
            }
            json_response(200, "OK", json!({ "ok": true }))
        }
        Err(_) => json_response(409, "Conflict", json!({ "error": "That time is no longer available. Please pick another slot." })),
    }
}

fn json_response(status: u16, reason: &'static str, value: serde_json::Value) -> Response {
    Response {
        status,
        reason,
        content_type: "application/json; charset=utf-8",
        body: serde_json::to_vec(&value).unwrap_or_else(|_| b"{\"error\":\"Unexpected error\"}".to_vec()),
    }
}

fn response_text(status: u16, reason: &'static str, content_type: &'static str, body: &str) -> Response {
    Response { status, reason, content_type, body: body.as_bytes().to_vec() }
}

fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn not_found_html() -> String {
    "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Booking unavailable</title><style>body{font-family:Inter,system-ui,sans-serif;background:#090b10;color:#f7f7fb;display:grid;min-height:100vh;place-items:center;margin:0}main{max-width:420px;padding:28px}p{color:#9ca3af}</style></head><body><main><h1>Booking unavailable</h1><p>This booking link is no longer active.</p></main></body></html>".into()
}

/// Make a JSON string safe to embed inside an inline `<script>`. `serde_json`
/// does not escape `<`/`/`, so a value containing `</script>` would break out of
/// the tag; U+2028/U+2029 also terminate JS string literals. Escaping these as
/// `\uXXXX` keeps the JSON valid while preventing tag/line breakout.
fn js_embed(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn booking_html(token: &str, slug: &str, et: &crate::model::EventType) -> String {
    let public = js_embed(&serde_json::to_string(&booking::public_event_type(et)).unwrap_or_else(|_| "{}".into()));
    let title = escape_html(&et.name);
    let color = escape_html(&et.color);
    let token = escape_html(token);
    let slug = escape_html(slug);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Book {title}</title>
  <style>
    :root {{ color-scheme: dark; --accent: {color}; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #090b10; color: #f7f7fb; }}
    main {{ width: min(960px, 100%); margin: 0 auto; padding: 32px 18px 48px; }}
    header {{ border-bottom: 1px solid #242833; padding-bottom: 20px; margin-bottom: 22px; }}
    .badge {{ display: inline-flex; align-items: center; gap: 8px; color: #c4cad8; font-size: 13px; }}
    .dot {{ width: 10px; height: 10px; border-radius: 999px; background: var(--accent); }}
    h1 {{ font-size: clamp(30px, 6vw, 56px); line-height: 1; margin: 14px 0 10px; letter-spacing: 0; }}
    p {{ color: #9aa3b5; line-height: 1.55; }}
    .grid {{ display: grid; grid-template-columns: minmax(0, 1fr) 320px; gap: 22px; align-items: start; }}
    .days {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 14px; }}
    .day {{ border: 1px solid #242833; border-radius: 8px; padding: 12px; background: #10131a; min-height: 100px; }}
    .day h2 {{ font-size: 13px; color: #d9deea; margin: 0 0 10px; }}
    button {{ border: 0; border-radius: 7px; font: inherit; cursor: pointer; }}
    .slot {{ width: 100%; margin: 4px 0; padding: 8px 10px; background: #141925; color: #dce9ff; border: 1px solid color-mix(in srgb, var(--accent), #ffffff 12%); }}
    .slot:hover,.slot.active {{ background: color-mix(in srgb, var(--accent), #10131a 78%); }}
    aside {{ border: 1px solid #242833; border-radius: 8px; padding: 16px; background: #10131a; position: sticky; top: 18px; }}
    label {{ display: block; margin: 12px 0 6px; font-size: 13px; color: #c4cad8; }}
    input {{ width: 100%; border: 1px solid #2b3140; border-radius: 7px; padding: 10px 11px; background: #090b10; color: #f7f7fb; font: inherit; outline: none; }}
    input:focus {{ border-color: var(--accent); }}
    .primary {{ width: 100%; margin-top: 14px; padding: 11px; background: var(--accent); color: #041016; font-weight: 700; }}
    .primary:disabled {{ opacity: .45; cursor: not-allowed; }}
    .status {{ min-height: 22px; font-size: 13px; margin-top: 12px; color: #9aa3b5; }}
    .error {{ color: #fb7185; }}
    .success {{ color: #34d399; }}
    @media (max-width: 760px) {{ .grid {{ grid-template-columns: 1fr; }} aside {{ position: static; }} }}
  </style>
</head>
<body>
  <main>
    <header>
      <div class="badge"><span class="dot"></span><span id="duration"></span></div>
      <h1>{title}</h1>
      <p>Choose an open time and Pushin will reserve it on the calendar.</p>
    </header>
    <section class="grid">
      <div>
        <p id="loading">Loading open times...</p>
        <div id="days" class="days"></div>
      </div>
      <aside>
        <h2 style="margin:0 0 8px;font-size:18px">Your details</h2>
        <p id="picked">Pick a time to continue.</p>
        <label for="name">Name</label>
        <input id="name" autocomplete="name">
        <label for="email">Email</label>
        <input id="email" type="email" autocomplete="email">
        <button id="book" class="primary" disabled>Confirm booking</button>
        <div id="status" class="status"></div>
      </aside>
    </section>
  </main>
  <script>
    const eventType = {public};
    const token = "{token}";
    const slug = "{slug}";
    let selected = null;
    const daysEl = document.getElementById("days");
    const loadingEl = document.getElementById("loading");
    const pickedEl = document.getElementById("picked");
    const bookBtn = document.getElementById("book");
    const statusEl = document.getElementById("status");
    document.getElementById("duration").textContent = `${{eventType.durationMinutes}} minutes`;

    function fmtDate(iso) {{ return new Date(iso).toLocaleDateString([], {{ weekday: "short", month: "short", day: "numeric" }}); }}
    function fmtTime(iso) {{ return new Date(iso).toLocaleTimeString([], {{ hour: "numeric", minute: "2-digit" }}); }}
    function setStatus(text, kind = "") {{ statusEl.textContent = text; statusEl.className = `status ${{kind}}`; }}

    async function loadSlots() {{
      const res = await fetch(`/api/b/${{token}}/${{slug}}/slots?horizonDays=14`);
      if (!res.ok) throw new Error("slots");
      const data = await res.json();
      const groups = new Map();
      for (const slot of data.slots) {{
        const key = fmtDate(slot.start);
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(slot);
      }}
      loadingEl.remove();
      if (!groups.size) {{
        daysEl.innerHTML = "<p>No open times are available right now.</p>";
        return;
      }}
      daysEl.innerHTML = "";
      for (const [day, slots] of groups) {{
        const card = document.createElement("div");
        card.className = "day";
        card.innerHTML = `<h2>${{day}}</h2>`;
        for (const slot of slots.slice(0, 10)) {{
          const btn = document.createElement("button");
          btn.className = "slot";
          btn.textContent = fmtTime(slot.start);
          btn.onclick = () => {{
            selected = slot;
            document.querySelectorAll(".slot").forEach((el) => el.classList.remove("active"));
            btn.classList.add("active");
            pickedEl.textContent = `${{day}} at ${{fmtTime(slot.start)}}`;
            bookBtn.disabled = false;
            setStatus("");
          }};
          card.appendChild(btn);
        }}
        daysEl.appendChild(card);
      }}
    }}

    bookBtn.onclick = async () => {{
      if (!selected) return;
      bookBtn.disabled = true;
      setStatus("Confirming...");
      const res = await fetch(`/api/b/${{token}}/${{slug}}/book`, {{
        method: "POST",
        headers: {{ "Content-Type": "application/json" }},
        body: JSON.stringify({{
          name: document.getElementById("name").value,
          email: document.getElementById("email").value,
          start: selected.start,
          end: selected.end
        }})
      }});
      if (res.ok) {{
        setStatus("Booked. You're on the calendar.", "success");
        bookBtn.textContent = "Booked";
      }} else {{
        const data = await res.json().catch(() => ({{ error: "Could not book that time." }}));
        setStatus(data.error || "Could not book that time.", "error");
        bookBtn.disabled = false;
      }}
    }};

    loadSlots().catch(() => {{
      loadingEl.textContent = "Could not load open times. Please try again later.";
      loadingEl.className = "error";
    }});
  </script>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Settings;

    #[test]
    fn public_page_hides_unknown_tokens() {
        let db = Arc::new(Mutex::new(db::test_conn()));
        let r = public_page(&db, "bad", "bad");
        assert_eq!(r.status, 404);
    }

    #[test]
    fn slots_route_requires_enabled_public_event_type() {
        let conn = db::test_conn();
        let id = db::insert_event_type(&conn, "Intro call", 30, 0, "#0ea5e9").unwrap();
        let et = db::update_event_type(&conn, id, "Intro call", 30, 0, "#0ea5e9", false).unwrap();
        let db = Arc::new(Mutex::new(conn));
        let r = public_slots(&db, &et.share_token, &et.slug, &HashMap::new());
        assert_eq!(r.status, 404);
    }

    #[test]
    fn booking_route_creates_booking() {
        let conn = db::test_conn();
        let id = db::insert_event_type(&conn, "Intro call", 30, 0, "#0ea5e9").unwrap();
        let et = db::get_event_type(&conn, id).unwrap();
        let settings = Settings::default();
        let slot = booking::available_slots(&conn, &settings, &et, 7).unwrap().remove(0);
        let db = Arc::new(Mutex::new(conn));
        let body = serde_json::to_vec(&json!({
            "name": "Ava",
            "email": "ava@example.com",
            "start": slot.start,
            "end": slot.end
        }))
        .unwrap();
        let r = public_book(&db, &reqwest::Client::new(), &et.share_token, &et.slug, &body);
        assert_eq!(r.status, 200);
        assert_eq!(db::list_bookings(&db.lock().unwrap()).unwrap().len(), 1);
    }

    #[test]
    fn parses_query_values() {
        let (_, q) = split_target("/api/b/a/b/slots?horizonDays=14&name=Intro+Call");
        assert_eq!(q.get("horizonDays").map(String::as_str), Some("14"));
        assert_eq!(q.get("name").map(String::as_str), Some("Intro Call"));
    }

    // ---- Security tests (see docs/notes/SECURITY_TEST_PLAN.md) ----

    fn et_with(conn: &Connection, dur: i64) -> crate::model::EventType {
        let id = db::insert_event_type(conn, "Intro call", dur, 0, "#0ea5e9").unwrap();
        db::get_event_type(conn, id).unwrap()
    }

    // 2.2 — off-grid / past / wrong-duration / reversed bookings are rejected.
    #[test]
    fn book_rejects_off_grid_times() {
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let settings = Settings::default();
        let slot = booking::available_slots(&conn, &settings, &et, 7).unwrap().remove(0);
        let db = Arc::new(Mutex::new(conn));
        let http = reqwest::Client::new();

        // Wrong duration (end is one hour after start, type is 30 min).
        let bad_dur = serde_json::to_vec(&json!({"name":"A","email":"a@x.io","start":slot.start,"end":"2030-01-01T11:00:00"})).unwrap();
        assert_ne!(public_book(&db, &http, &et.share_token, &et.slug, &bad_dur).status, 200);

        // A start that is not a generated slot boundary.
        let off_grid = serde_json::to_vec(&json!({"name":"A","email":"a@x.io","start":"2030-01-01T10:07:00","end":"2030-01-01T10:37:00"})).unwrap();
        assert_eq!(public_book(&db, &http, &et.share_token, &et.slug, &off_grid).status, 409);

        // Reversed interval.
        let reversed = serde_json::to_vec(&json!({"name":"A","email":"a@x.io","start":slot.end,"end":slot.start})).unwrap();
        assert_ne!(public_book(&db, &http, &et.share_token, &et.slug, &reversed).status, 200);
    }

    // 3.3 — token/slug are bound as SQL params; injection just fails the lookup.
    #[test]
    fn sql_injection_in_token_does_not_authenticate() {
        let conn = db::test_conn();
        let _ = et_with(&conn, 30);
        let db = Arc::new(Mutex::new(conn));
        let r = public_page(&db, "' OR '1'='1", "' OR '1'='1' --");
        assert_eq!(r.status, 404);
        let s = public_slots(&db, "x'; DROP TABLE event_types;--", "y", &HashMap::new());
        assert_eq!(s.status, 404);
        // The table still exists / app still works.
        assert!(db::list_event_types(&db.lock().unwrap()).is_ok());
    }

    // 3.1 — event-type name cannot break out of the inline <script>.
    #[test]
    fn booking_html_escapes_script_breakout() {
        let conn = db::test_conn();
        let id = db::insert_event_type(&conn, "</script><img src=x onerror=alert(1)>", 30, 0, "#0ea5e9").unwrap();
        let et = db::get_event_type(&conn, id).unwrap();
        let html = booking_html(&et.share_token, &et.slug, &et);
        let script_start = html.find("const eventType").unwrap();
        // Nothing after the data assignment closes the tag prematurely.
        assert!(!html[script_start..].contains("</script><img"), "raw breakout present");
        assert!(html.contains("\\u003c/script\\u003e"), "expected escaped form in embedded JSON");
    }

    // 2.1 / 1.3 — the global booking limiter blocks a flood within the window.
    #[test]
    fn rate_limiter_blocks_flood() {
        let mut rl = RateLimiter::default();
        let now = Instant::now();
        for _ in 0..BOOK_LIMIT {
            assert!(rl.allow(now, BOOK_LIMIT, BOOK_WINDOW));
        }
        assert!(!rl.allow(now, BOOK_LIMIT, BOOK_WINDOW), "limit not enforced");
        // After the window slides, attempts are allowed again.
        assert!(rl.allow(now + BOOK_WINDOW, BOOK_LIMIT, BOOK_WINDOW));
    }

    // 2.4 — regenerating the token invalidates the old shareable link.
    #[test]
    fn regenerated_token_invalidates_old_link() {
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let old_token = et.share_token.clone();
        let updated = db::regenerate_event_type_token(&conn, et.id).unwrap();
        assert_ne!(updated.share_token, old_token);
        let db = Arc::new(Mutex::new(conn));
        assert_eq!(public_page(&db, &old_token, &et.slug).status, 404);
        assert_eq!(public_page(&db, &updated.share_token, &et.slug).status, 200);
    }

    // 1.1 — an oversized declared Content-Length is rejected without allocating toward it.
    #[test]
    fn oversized_body_is_rejected() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream).map(|_| ()).err().map(|e| e.to_string())
        });
        let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        client
            .write_all(b"POST /api/b/x/y/book HTTP/1.1\r\nContent-Length: 100000000\r\n\r\n{}")
            .unwrap();
        let err = handle.join().unwrap();
        assert!(err.map(|e| e.contains("too large")).unwrap_or(false), "oversized body not rejected early");
    }

    // 1.2 — a slow client occupies only its own thread; a concurrent request still completes.
    #[test]
    fn slow_client_does_not_block_others() {
        // Discover a free port (bind_listener echoes the requested port, so we
        // can't pass 0 and read it back).
        let free_port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let db = Arc::new(Mutex::new(db::test_conn()));
        let handle = start(db, reqwest::Client::new(), Some(free_port)).unwrap();
        let port = handle.status().port.unwrap();

        // A slow client: send a partial header line and never finish.
        let mut slow = TcpStream::connect(("127.0.0.1", port)).unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost: x").unwrap();

        // A normal client should still get a prompt response.
        let mut fast = TcpStream::connect(("127.0.0.1", port)).unwrap();
        fast.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        fast.write_all(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").unwrap();
        let mut resp = Vec::new();
        let _ = fast.read_to_end(&mut resp);
        let text = String::from_utf8_lossy(&resp);
        assert!(text.contains("Pushin booking server"), "fast client blocked by slow client: {text:?}");
        handle.stop();
    }

    // ---- Resilience ----

    // A panic anywhere in the app poisons the shared connection mutex. `AppState::db()` already
    // recovers from that; the booking server used to `.unwrap()` the same mutex, so one unrelated
    // panic silently 500'd every public booking link with nothing visible inside the app.
    #[test]
    fn a_poisoned_db_lock_does_not_take_the_public_page_down() {
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let db = Arc::new(Mutex::new(conn));

        // Poison the mutex the way a real bug would: panic while holding the guard.
        let poisoner = Arc::clone(&db);
        let _ = thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("simulated bug in an unrelated command");
        })
        .join();
        assert!(db.lock().is_err(), "test precondition: the mutex is poisoned");

        // Every public route still answers.
        assert_eq!(public_page(&db, &et.share_token, &et.slug).status, 200);
        assert_eq!(public_slots(&db, &et.share_token, &et.slug, &HashMap::new()).status, 200);
        assert_eq!(public_page(&db, "nope", "nope").status, 404, "and still 404s correctly");
    }

    // ---- Request parsing ----

    #[test]
    fn the_path_is_never_percent_decoded_but_query_values_are() {
        // Deliberate asymmetry, pinned here because reversing it would be a security regression:
        // slugs and share tokens are generated as `[a-z0-9-]` / hex, so the path never NEEDS
        // decoding — and decoding it would let `%2F` smuggle a path separator past the router.
        // Query values are user-facing text and must decode.
        let (path, q) = split_target("/b/tok/slug%2Fdeep?name=Ann%20Lee&note=100%25%20sure");
        assert_eq!(path, "/b/tok/slug%2Fdeep", "an encoded slash stays encoded and simply fails to route");
        assert_eq!(q.get("name").map(String::as_str), Some("Ann Lee"));
        assert_eq!(q.get("note").map(String::as_str), Some("100% sure"));

        // ...and a smuggled separator does not reach a route.
        let db = Arc::new(Mutex::new(db::test_conn()));
        let http = reqwest::Client::new();
        let limiter = Arc::new(Mutex::new(RateLimiter::default()));
        let req = Request { method: "GET".into(), path, query: HashMap::new(), body: Vec::new() };
        assert_eq!(route(req, &db, &http, &limiter).status, 404);
    }

    #[test]
    fn malformed_percent_escapes_in_a_query_are_not_fatal() {
        // A dangling `%` or non-hex pair must not panic or drop the rest of the query string.
        let (_, q) = split_target("/b/t/s?a=trailing%&b=bad%zz&c=fine");
        assert_eq!(q.get("c").map(String::as_str), Some("fine"), "later pairs survive an earlier bad escape");
        assert!(q.get("a").is_some() || q.get("c").is_some(), "decoding never wipes the whole map");
    }

    #[test]
    fn a_plus_in_a_query_value_is_a_space_but_a_plus_in_the_path_is_not() {
        let (path, q) = split_target("/b/a+b/slug?name=Ada+Lovelace");
        assert_eq!(q.get("name").map(String::as_str), Some("Ada Lovelace"));
        assert!(path.contains("a+b") || path.contains("a b"), "path: {path:?}");
    }

    #[test]
    fn a_target_with_no_query_yields_an_empty_map() {
        let (path, q) = split_target("/b/tok/slug");
        assert_eq!(path, "/b/tok/slug");
        assert!(q.is_empty());
        let (_, q2) = split_target("/b/tok/slug?");
        assert!(q2.is_empty());
        let (_, q3) = split_target("/b/tok/slug?flag&x=1");
        assert_eq!(q3.get("x").map(String::as_str), Some("1"), "a valueless key does not eat the next pair");
    }

    #[test]
    fn non_ascii_in_the_target_survives_decoding() {
        // UTF-8 percent-escapes are decoded as bytes then re-assembled — a naive char-wise decoder
        // panics here.
        let (_, q) = split_target("/b/t/s?name=Jos%C3%A9%20Garc%C3%ADa");
        assert_eq!(q.get("name").map(String::as_str), Some("Jos\u{e9} Garc\u{ed}a"));
    }

    // ---- Routing ----

    #[test]
    fn unknown_routes_404_and_the_root_is_a_liveness_probe() {
        let db = Arc::new(Mutex::new(db::test_conn()));
        let http = reqwest::Client::new();
        let limiter = Arc::new(Mutex::new(RateLimiter::default()));

        let req = |method: &str, path: &str| Request {
            method: method.to_string(),
            path: path.to_string(),
            query: HashMap::new(),
            body: Vec::new(),
        };

        assert_eq!(route(req("GET", "/"), &db, &http, &limiter).status, 200);
        assert_eq!(route(req("GET", "/admin"), &db, &http, &limiter).status, 404);
        assert_eq!(route(req("GET", "/../../etc/passwd"), &db, &http, &limiter).status, 404);
        assert_eq!(route(req("POST", "/b/tok/slug"), &db, &http, &limiter).status, 404, "GET-only page rejects POST");
        assert_eq!(route(req("DELETE", "/"), &db, &http, &limiter).status, 404);
    }

    #[test]
    fn a_non_json_booking_body_is_a_400_not_a_500() {
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let db = Arc::new(Mutex::new(conn));
        let http = reqwest::Client::new();
        for body in [&b""[..], b"not json", b"{", b"[]", b"{\"name\":123}"] {
            let r = public_book(&db, &http, &et.share_token, &et.slug, body);
            assert_eq!(r.status, 400, "body {:?} should be a clean 400", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn the_slots_horizon_is_clamped_to_a_sane_range() {
        // `horizonDays` is attacker-controlled; an unclamped value walks the scheduler over years
        // of days per request. Values outside 1..=60 must be clamped, not honoured or rejected.
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let db = Arc::new(Mutex::new(conn));

        let slots_for = |v: &str| {
            let mut q = HashMap::new();
            q.insert("horizonDays".to_string(), v.to_string());
            let r = public_slots(&db, &et.share_token, &et.slug, &q);
            assert_eq!(r.status, 200, "horizonDays={v} should still answer");
            let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            parsed["slots"].as_array().map(|a| a.len()).unwrap_or(0)
        };

        let huge = slots_for("100000");
        let sixty = slots_for("60");
        assert_eq!(huge, sixty, "an absurd horizon is clamped to 60 days");
        assert!(slots_for("0") <= slots_for("1"), "a zero horizon clamps up to 1 day, never negative");
        assert!(slots_for("-5") <= slots_for("1"), "a negative horizon clamps up to 1 day");
        // Garbage falls back to the default rather than erroring.
        let _ = slots_for("banana");
    }

    #[test]
    fn double_booking_the_same_slot_is_refused() {
        // The public page is the one surface a stranger drives; two people clicking the same time
        // must not both land on the calendar.
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let settings = Settings::default();
        let slot = booking::available_slots(&conn, &settings, &et, 7).unwrap().remove(0);
        let db = Arc::new(Mutex::new(conn));
        let http = reqwest::Client::new();
        let body = serde_json::to_vec(&json!({
            "name": "First", "email": "a@example.com", "start": slot.start, "end": slot.end
        }))
        .unwrap();

        assert_eq!(public_book(&db, &http, &et.share_token, &et.slug, &body).status, 200);
        let second = serde_json::to_vec(&json!({
            "name": "Second", "email": "b@example.com", "start": slot.start, "end": slot.end
        }))
        .unwrap();
        assert_ne!(public_book(&db, &http, &et.share_token, &et.slug, &second).status, 200, "slot taken");
        assert_eq!(db::list_bookings(&lock_db(&db)).unwrap().len(), 1);
    }

    #[test]
    fn a_booked_slot_disappears_from_the_public_slot_list() {
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let settings = Settings::default();
        let slot = booking::available_slots(&conn, &settings, &et, 7).unwrap().remove(0);
        let db = Arc::new(Mutex::new(conn));
        let http = reqwest::Client::new();
        let body =
            serde_json::to_vec(&json!({ "name": "A", "email": "a@example.com", "start": slot.start, "end": slot.end }))
                .unwrap();
        assert_eq!(public_book(&db, &http, &et.share_token, &et.slug, &body).status, 200);

        let r = public_slots(&db, &et.share_token, &et.slug, &HashMap::new());
        let parsed: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        let starts: Vec<&str> = parsed["slots"].as_array().unwrap().iter().filter_map(|s| s["start"].as_str()).collect();
        assert!(!starts.contains(&slot.start.as_str()), "the taken slot is still offered");
    }

    #[test]
    fn a_hostile_booker_name_cannot_inject_into_the_page() {
        // The name is stored and later rendered in-app; assert it round-trips as inert text.
        let conn = db::test_conn();
        let et = et_with(&conn, 30);
        let settings = Settings::default();
        let slot = booking::available_slots(&conn, &settings, &et, 7).unwrap().remove(0);
        let db = Arc::new(Mutex::new(conn));
        let http = reqwest::Client::new();
        let evil = "<script>alert(1)</script>";
        let body =
            serde_json::to_vec(&json!({ "name": evil, "email": "x@example.com", "start": slot.start, "end": slot.end }))
                .unwrap();
        assert_eq!(public_book(&db, &http, &et.share_token, &et.slug, &body).status, 200);

        let bookings = db::list_bookings(&lock_db(&db)).unwrap();
        assert_eq!(bookings.len(), 1);
        let html = booking_html(&et.share_token, &et.slug, &et);
        assert!(!html.contains("<script>alert(1)</script>"), "booker name leaked into the public page markup");
    }
}
