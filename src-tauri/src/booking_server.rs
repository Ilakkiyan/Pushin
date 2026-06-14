use crate::calendar::google;
use crate::schedule_service::reschedule_inner;
use crate::{booking, db};
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_PORT: u16 = 47610;

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
    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let response = match read_request(&mut stream) {
                    Ok(req) => route(req, &db, &http),
                    Err(_) => response_text(400, "Bad Request", "text/plain; charset=utf-8", "Bad request"),
                };
                let _ = write_response(&mut stream, response);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(35)),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut header_end = None;
    while header_end.is_none() && buf.len() < 64 * 1024 {
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

    let body_start = header_end + 4;
    let mut body = buf.get(body_start..).unwrap_or_default().to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
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

fn route(req: Request, db: &Arc<Mutex<Connection>>, http: &reqwest::Client) -> Response {
    let parts: Vec<&str> = req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    match (req.method.as_str(), parts.as_slice()) {
        ("GET", ["b", token, slug]) => public_page(db, token, slug),
        ("GET", ["api", "b", token, slug, "slots"]) => public_slots(db, token, slug, &req.query),
        ("POST", ["api", "b", token, slug, "book"]) => public_book(db, http, token, slug, &req.body),
        ("GET", []) => response_text(200, "OK", "text/plain; charset=utf-8", "Pushin booking server"),
        _ => response_text(404, "Not Found", "text/plain; charset=utf-8", "Not found"),
    }
}

fn public_page(db: &Arc<Mutex<Connection>>, token: &str, slug: &str) -> Response {
    let conn = db.lock().unwrap();
    match db::public_event_type(&conn, token, slug) {
        Ok(Some(et)) => response_text(200, "OK", "text/html; charset=utf-8", &booking_html(token, slug, &et)),
        _ => response_text(404, "Not Found", "text/html; charset=utf-8", &not_found_html()),
    }
}

fn public_slots(db: &Arc<Mutex<Connection>>, token: &str, slug: &str, query: &HashMap<String, String>) -> Response {
    let conn = db.lock().unwrap();
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
        let mut conn = db.lock().unwrap();
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

fn booking_html(token: &str, slug: &str, et: &crate::model::EventType) -> String {
    let public = serde_json::to_string(&booking::public_event_type(et)).unwrap_or_else(|_| "{}".into());
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
        let slot = booking::available_slots(&conn, &settings, &et, 2).unwrap().remove(0);
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
}
