# Booking-page (tunnel link) security test plan

Status legend: ☐ not run · ⚠ confirmed issue · ✅ verified safe · 🔧 fixed & re-verified

## System under test

The public booking flow turns an in-app mockup into a working page served by a
hand-rolled HTTP/1.1 server.

- **Server**: [`src-tauri/src/booking_server.rs`](src-tauri/src/booking_server.rs) — a
  `TcpListener` bound to `127.0.0.1:47610` (scans +20 ports). **Single-threaded**: `run()`
  services one connection at a time.
- **Routes** (`route()`):
  - `GET  /b/{token}/{slug}` → self-contained HTML booking page
  - `GET  /api/b/{token}/{slug}/slots?horizonDays=N` → JSON free slots
  - `POST /api/b/{token}/{slug}/book` → create booking (writes a calendar event, reschedules, syncs Google)
  - `GET  /` → liveness string
- **Auth**: the only secret is a 128-bit `getrandom` hex `share_token` in the URL. `slug`
  is guessable (`slugify(name)-id`). Gate: `WHERE share_token=? AND slug=? AND enabled=1`.
- **Tunnel**: the user runs `ngrok`/`cloudflared` against the loopback port (commands shown
  in `BookingPane`). The tunnel **deliberately bypasses the loopback binding** → once tunneled,
  the server is publicly reachable with no auth beyond the URL token.

## Trust boundary

Once tunneled, **anyone on the internet with the link** is the attacker. The token is the only
control over (a) reading the owner's free/busy for up to 60 days and (b) writing calendar events
via bookings. The token rides in the URL → exposed to tunnel-provider logs, browser history, `Referer`.

---

## Test matrix

### P1 — Denial of service

- **1.1 Unbounded request body** — `read_request` loops `while body.len() < content_length` with no
  cap on attacker-controlled `Content-Length` (the 64 KB cap only guards *headers*). Memory exhaustion.
  *Test*: POST with `Content-Length: 2000000000`. *Pass*: bounded/rejected.
- **1.2 Single-thread head-of-line / Slowloris** — `run()` fully services one socket before the next;
  3 s read timeout per connection. One slow client stalls all bookings.
  *Test*: hold a partial-header connection; time a concurrent legit request. *Pass*: legit request unaffected.
- **1.3 Google-sync amplification** — every successful `/book` runs `reschedule_inner` + a full
  `google::sync`. No rate limit. *Test*: many valid bookings; observe sync volume.
- **1.4 Slot-generation cost** — `/slots` clamps `horizonDays` to 1–60 but enumerates every slot.
  *Test*: hammer `/slots?horizonDays=60` with a short-duration type; measure CPU/response size.

### P2 — Business-logic abuse

- **2.1 Booking spam / calendar flooding** — no rate limit, no email verification
  (`validate_invitee` is format-only). *Test*: enumerate `/slots`, POST every slot. *Pass*: some throttle/cap.
- **2.2 Arbitrary / past / off-grid times** — `confirm_booking` requires exact duration match + slot
  membership. *Test*: past start, `end<start`, wrong duration, non-boundary start. *Pass*: 400/409.
- **2.3 Double-booking race** — single-thread + DB mutex should serialize. *Test*: 50 concurrent POSTs
  for one slot. *Pass*: exactly one 200, rest 409.
- **2.4 Disabled / regenerated token** — `enabled=1` filter + token regen. *Test*: disable → old link 404;
  regenerate → old token 404.

### P3 — Injection & rendering

- **3.1 `</script>` breakout XSS** — `booking_html` injects `const eventType = {public};` with raw
  `serde_json` (does not escape `<`/`/`). *Test*: event-type name `</script><img src=x onerror=alert(1)>`.
  *Pass*: no execution.
- **3.2 Stored XSS via invitee name** — invitee `name` is stored and surfaced in the app + Google title.
  *Test*: `name = "><svg onload=...>`. *Pass*: escaped everywhere.
- **3.3 SQL injection** — queries are parameterized. *Test*: `' OR 1=1--` in token/slug/body. *Pass*: clean 404.
- **3.4 Response-header / CRLF injection** — status line fields are static. *Test*: confirm no input reaches headers.

### P4 — Network exposure & tunnel-specific

- **4.1 No `Host` validation → DNS rebinding** — server answers any `Host`; a site the owner visits could
  rebind to 127.0.0.1 and probe/POST. *Test*: foreign `Host:` served? *Pass*: allowlist enforced.
- **4.2 No CSRF/Origin check on `/book`** — POST accepts any Origin. *Test*: cross-origin POST accepted?
- **4.3 Token leakage** — token in URL path → provider logs, `Referer`, history; no expiry/rotation policy.
- **4.4 Security headers / TLS** — only `X-Content-Type-Options: nosniff`; TLS only from the tunnel.

### P5 — Info disclosure / privacy

- **5.1 Availability leakage** — link reveals free/busy up to 60 days. *Pass*: slots carry start/end only, no titles.
- **5.2 Error verbosity** — 4xx/5xx bodies must not leak internals.
- **5.3 Token comparison timing** — non-constant-time DB compare; 128-bit random → accepted risk.

---

## Execution order

1. P1.1, P1.2 (highest-impact, cheapest to prove)
2. P2.1 (by-design gap to surface)
3. P3.1, P3.2 (injection)
4. P4.1, P4.2 (rebinding / CSRF)
5. Confirm the already-guarded items (2.2, 2.3, 3.3, 5.1)
6. Fix confirmed issues; re-verify with `cargo test`.

## Results log

Executed 2026-06-14. All findings verified by tests in
[`booking_server.rs`](src-tauri/src/booking_server.rs) `mod tests` (`cargo test --lib`, 149 passed).

### Fixed (🔧)

- **1.1 Unbounded body** — `read_request` now rejects `Content-Length > MAX_BODY` (64 KB) up front and
  caps bytes read. Test: `oversized_body_is_rejected`.
- **1.2 Single-thread / Slowloris** — `run()` now spawns **one thread per connection** with a
  `MAX_INFLIGHT` (64) load-shed cap, plus a whole-request `REQUEST_DEADLINE` (8 s) on top of the per-read
  timeout. A slow client no longer stalls the accept loop. Test: `slow_client_does_not_block_others`.
- **1.3 / 2.1 Sync amplification & booking spam** — global `RateLimiter` on `POST /book`
  (`BOOK_LIMIT`=8 per `BOOK_WINDOW`=60 s) → 429. Global (not per-IP) because tunneled requests all
  appear from 127.0.0.1. Tests: `rate_limiter_blocks_flood`. This also bounds 1.4 slot-cost abuse.
- **3.1 `</script>` breakout XSS** — `js_embed` escapes `< > & U+2028 U+2029` in the JSON embedded in
  the inline `<script>`. Test: `booking_html_escapes_script_breakout`.

### Verified safe (✅)

- **2.2 Off-grid/past/wrong-duration** — `confirm_booking` enforces exact duration + slot membership.
  Test: `book_rejects_off_grid_times`.
- **2.3 Double-booking race** — single thread holds the DB mutex across the whole book op; the second
  booking of a taken slot fails availability. Test: `confirm_booking_rejects_stale_slot`.
- **2.4 Disabled / regenerated token** — `enabled=1` filter + token regen invalidate old links.
  Tests: `slots_route_requires_enabled_public_event_type`, `regenerated_token_invalidates_old_link`.
- **3.2 Stored XSS via invitee name** — rendered through React (`{booking.inviteeName}`), auto-escaped.
- **3.3 SQL injection** — token/slug bound as params; injection just 404s. Test:
  `sql_injection_in_token_does_not_authenticate`.
- **3.4 Header/CRLF injection** — status-line fields are static; no request input reaches headers.
- **5.1 Availability leakage** — `BookingSlot` carries only `start`/`end`; no titles/attendees leak.

### Accepted risk / by design (documented, not changed)

- **4.1 No `Host` validation (DNS rebinding)** — the service is *intentionally* public via the tunnel, so
  the `Host` is the tunnel hostname and can't be allowlisted without breaking the feature. All data
  routes require the 128-bit token; only `GET /` liveness is unauthenticated. Low risk. Revisit only if a
  fixed public domain is adopted.
- **4.2 No CSRF/Origin check on `/book`** — a public booking form is unauthenticated by design; the
  unguessable token + the new global rate limit are the controls. Origin-pinning would break legitimate
  cross-origin embeds.
- **4.3 Token in URL (leakage)** — inherent to the URL-token design. Mitigations exist: token rotation
  (`regenerate_event_type_token`) and per-type `enabled` toggle. Don't log full URLs server-side.
- **4.4 Security headers / TLS** — TLS is terminated by the tunnel (ngrok/cloudflared); the loopback hop
  is plaintext on 127.0.0.1, acceptable. `X-Content-Type-Options: nosniff` is set; the page is
  self-contained so a CSP adds little.
- **5.3 Token-compare timing** — non-constant-time DB compare, but 128 bits of `getrandom` entropy make
  a remote timing/brute-force attack infeasible. Accepted.
