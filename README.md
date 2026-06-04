# Pushin — local-AI calendar

A Motion-style calendar/task planner where you describe your work in plain language and a
**small LLM that runs 100% on your device** turns it into tasks, then a deterministic
auto-scheduler places them on your calendar around your fixed events, deadlines, and priorities.

- **On-device only** — the AI runs locally (llama.cpp / Ollama). Nothing leaves your machine.
- **LLM parses, solver schedules** — the model extracts structured tasks; a Rust scheduler does the
  constraint-solving (reliable even with 1–3B models).
- **Desktop-first** (Tauri 2 + React) with the frontend/solver structured to extend to PWA/mobile later.

## Architecture

```
React UI (chat · week-calendar · tasks · booking · settings)
  │ Tauri IPC
Rust core
  ├─ parser     natural language → structured plan (dates resolved in Rust, not the LLM)
  ├─ scheduler  dependency DAG + EDF/priority greedy + chunking + conflict detection  ← core IP
  ├─ model_manager  download/verify GGUF, start/detect local inference server
  ├─ llm        OpenAI-compatible client (127.0.0.1 only)
  ├─ calendar/  CalendarProvider trait → LocalProvider (live) + GoogleProvider (stub)
  ├─ booking    booking-page availability (reuses scheduler::free_slots)
  └─ db         SQLite (rusqlite), Rust-owned; frontend goes through typed commands
        ▼
llama-server / Ollama  ── loads a quantized model (Qwen2.5 3B / 1.5B)
```

## Prerequisites

- Node 18+ and Rust (stable) with the Tauri 2 prerequisites for your OS.
- A local, OpenAI-compatible inference server (one of):
  - **Ollama** (easiest): `ollama serve`, then set the server URL in Pushin → Settings to
    `http://127.0.0.1:11434` and pull a model (e.g. `ollama pull qwen2.5:3b`).
  - **llama.cpp `llama-server`**: build/install it, then either run it yourself on `:8080`, or put
    the binary in the app's data `bin/` folder and let Pushin start it. Pushin can also download a
    GGUF model for you from the in-app setup card.

## Develop

```bash
npm install
npm run tauri dev
```

On first launch Pushin creates its SQLite DB in the OS app-data dir and seeds a default booking
event type. If no inference server is reachable, the chat panel shows a setup card to download a
model and start/detect a server.

## Test & build

```bash
# Scheduler unit tests (the core IP)
cd src-tauri && cargo test

# Production build (.app/.dmg on macOS)
npm run tauri build
```

## Status / finishing steps

Implemented and working: on-device planning pipeline, auto-scheduler (15 passing tests), full-day
week-grid calendar with drag-to-move/pin and re-planning, conversational create/update/remove of
events, conflict surfacing, task list, settings, and **two-way Google Calendar sync**.

## Google Calendar sync (setup)

Two-way sync with your **primary** calendar: Google events are pulled in (the scheduler plans around
them) and your events + auto-scheduled task blocks are mirrored out. It needs your own free Google
OAuth client (≈10 min, one time):

1. Open [Google Cloud Console → Credentials](https://console.cloud.google.com/apis/credentials) and
   create (or pick) a project.
2. **Enable the Google Calendar API** (APIs & Services → Library → "Google Calendar API" → Enable).
3. **OAuth consent screen**: set it up (External is fine), and under *Audience/Test users* add your
   own Google address (while the app is in "testing", only listed test users can authorize).
4. **Create credentials → OAuth client ID → Application type: Desktop app.** Copy the **Client ID**
   and **Client secret**.
5. In Pushin → **Settings → Google Calendar**, paste the Client ID/secret and click **Connect**. A
   browser window opens for consent; approve it and you're synced.

Notes: loopback redirect + PKCE are used (no redirect URI to register for Desktop clients). Tokens
are stored in the app-data SQLite for now — moving them to the OS keychain is a hardening follow-up.
Pushin-created block events are tagged via `extendedProperties.private` so they reconcile without
duplicating.

## Documented follow-ups
- **Bundle `llama-server`** as a per-OS sidecar so no separate install is needed.
- **Google tokens → OS keychain**; smarter block-mirror diffing (currently delete+recreate each sync).
- **Mobile** (in-process inference; smaller default model).
- **Public booking page** — host a small relay so the in-app booking mockup becomes shareable.
