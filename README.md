<div align="center">

# 📌 Pushin

**A local‑AI, Motion‑style calendar.** Describe your day in plain language — a small language model
running **100% on your device** turns it into tasks and events, and a deterministic Rust
auto‑scheduler packs everything into your calendar around your fixed commitments.

No cloud. No account. Nothing leaves your machine.

</div>

---

## Table of contents

- [What is Pushin?](#what-is-pushin)
- [Features](#features)
- [How it works](#how-it-works)
- [Quick start (10–15 minutes)](#quick-start-1015-minutes)
  - [1. Install the prerequisites](#1-install-the-prerequisites)
  - [2. Get the code & dependencies](#2-get-the-code--dependencies)
  - [3. Run it](#3-run-it)
  - [4. Turn on the AI](#4-turn-on-the-ai)
- [Using Pushin](#using-pushin)
- [Google Calendar sync (optional)](#google-calendar-sync-optional)
- [Downloads & building installers](#downloads--building-installers)
- [Development](#development)
- [Project layout](#project-layout)
- [Troubleshooting](#troubleshooting)
- [Privacy](#privacy)
- [Tech stack](#tech-stack)

---

## What is Pushin?

Most planners make *you* do the scheduling. Pushin flips that around:

1. **You talk to it.** "I have a dentist appointment Friday at 2, and I need to finish the slides
   (about 3 hours) before Monday."
2. **A tiny on‑device LLM extracts structure** — fixed events vs. work tasks, durations, deadlines.
   It only does the *parsing*; it never does the math.
3. **A deterministic Rust scheduler does the planning** — it places your tasks into real free time
   around your fixed events, respecting deadlines, dependencies, and priorities, and flags anything
   that won't fit.

The result is a calendar that fills itself in, works offline, and keeps your data on your machine.

---

## Features

| | |
|---|---|
| 🗣️ **Natural‑language planning** | Type tasks and events like you'd text a friend; the model turns them into a structured plan. |
| 🧠 **Deterministic auto‑scheduler** | Dependency‑aware EDF + priority packing with conflict detection — the core IP, in Rust, with unit tests. |
| 📅 **Week view** | Full 24‑hour week grid with drag‑to‑move and pin‑to‑lock; re‑plans around your changes. |
| 🗓️ **Month view** | Google‑Calendar‑style month grid with per‑day event chips; click a day to jump to its week. |
| 🔥 **Habit tracker** | Build habits with streaks, a consistency heatmap, and one‑click "add to today's calendar" that slots a habit into a free gap. |
| ✅ **Task list** | Auto‑scheduled work blocks with status, priority, and deadlines. |
| 🔗 **Two‑way Google Calendar sync** | Mirror events and task blocks to your primary calendar (optional). |
| 🔒 **100% on‑device** | Inference runs locally via llama.cpp or Ollama. The app works fully offline. |

---

## How it works

```
            ┌─────────────────────────────────────────────┐
            │  React UI  (chat · week · month · habits ·   │
            │            tasks · booking · settings)       │
            └───────────────────────┬─────────────────────┘
                                    │  Tauri IPC (typed commands)
            ┌───────────────────────▼─────────────────────┐
            │  Rust core                                   │
            │   • parser     NL → events/tasks (dates &    │
            │                ranges resolved in Rust)      │
            │   • scheduler  free‑slot packing + conflicts │
            │   • habits     streaks / consistency / slots │
            │   • db         SQLite (source of truth)      │
            │   • calendar   two‑way Google sync           │
            └─────────┬───────────────────────┬───────────┘
                      │ spawns                 │ HTTPS (optional)
                      ▼                        ▼
        llama.cpp `llama-server`        Google Calendar API
        (local, OpenAI‑compatible)
```

**Design rule:** the LLM *parses*, Rust *schedules*. Small models are great at pulling structure
out of a sentence and unreliable at arithmetic, so all date/time/duration resolution is done
deterministically in Rust — and covered by unit tests.

---

## Quick start (10–15 minutes)

> **TL;DR**
> ```bash
> git clone https://github.com/Ilakkiyan/Pushin.git
> cd Pushin
> npm install
> npm run tauri dev
> ```
> Then click **Download model** in the app's setup card on first launch.

Most of the 10–15 minutes is installing Rust and the Tauri system dependencies (one time). The
app itself builds and launches in a couple of minutes. The on‑device model is a separate **one‑time
~2 GB download** that happens inside the app on first run (or instantly if you already use Ollama).

### 1. Install the prerequisites

You need **Node 18+**, **Rust (stable)**, and your OS's Tauri build tools.

<details open>
<summary><b>macOS</b></summary>

```bash
# Xcode command‑line tools (compiler + headers)
xcode-select --install

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node (via Homebrew, or download from nodejs.org)
brew install node
```
</details>

<details>
<summary><b>Windows</b></summary>

1. **Microsoft C++ Build Tools** — install "Desktop development with C++" from the
   [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/).
2. **WebView2 Runtime** — already on Windows 11; on Windows 10 grab it
   [here](https://developer.microsoft.com/microsoft-edge/webview2/).
3. **Rust** — install via [rustup](https://rustup.rs/).
4. **Node 18+** — from [nodejs.org](https://nodejs.org/).
</details>

<details>
<summary><b>Linux (Debian/Ubuntu)</b></summary>

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Node 18+ (use your distro's package or nvm)
```
For other distros, see the [Tauri prerequisites guide](https://tauri.app/start/prerequisites/).
</details>

Verify:
```bash
node --version   # v18 or newer
cargo --version  # any stable
```

### 2. Get the code & dependencies

```bash
git clone https://github.com/Ilakkiyan/Pushin.git
cd Pushin
npm install
```

### 3. Run it

```bash
npm run tauri dev
```

The first launch compiles the Rust core (a few minutes — subsequent launches are instant) and
opens the Pushin window. It also creates its SQLite database in your OS app‑data folder and seeds
a default booking type.

### 4. Turn on the AI

Pushin needs a local, OpenAI‑compatible inference server. Pick **one**:

<details open>
<summary><b>Option A — built‑in (easiest, nothing to install)</b></summary>

On first launch the chat panel shows a **setup card**. Click to **download a model** (Qwen2.5 3B,
~2 GB) — Pushin fetches the model *and* the llama.cpp engine for your OS automatically and starts
the server on `http://127.0.0.1:8080`. The status pill in the top‑right turns green ("AI ready")
when it's up.

> The 3B model is the default. For more reliable multi‑step parsing, choose the **7B** model
> (~4.7 GB) in **Settings**. There's also a lightweight **1.5B**.
</details>

<details>
<summary><b>Option B — Ollama (if you already use it)</b></summary>

```bash
ollama serve
ollama pull qwen2.5:3b
```
Then in **Settings → Inference**, set the server URL to `http://127.0.0.1:11434` and the model to
`qwen2.5:3b`.
</details>

That's it — type into the chat box and watch your calendar fill in.

---

## Using Pushin

**Chat (right panel).** Describe events and work in plain language:
- *"Lunch with mom Friday 12–2 and a graduation party from 6–10."* → two events.
- *"I need to study for the exam, about 4 hours, due Thursday."* → a task the scheduler places.
- *"Make the meeting today 2 hours instead of 1."* → edits the existing event.
- *"Move the dentist to tomorrow at 3pm."* → reschedules it.

**Week** — the full 24‑hour grid. Drag a task block to move it; double‑click to pin it (pinned
blocks survive re‑planning). Click empty space to add a fixed/busy event.

**Month** — a Google‑Calendar‑style overview. Each day shows little chips for its events and task
blocks; click any day to jump to that week.

**Habits** — add a habit with a color and a duration. Check it off for the day, see your **current
streak**, **longest streak**, and **30‑day consistency**, and click squares in the heatmap to
backfill past days. Hit **Add to today** to slot the habit into a free gap on your calendar (it
lands near the end of the day, tucked between existing commitments) — the scheduler then plans your
tasks around it.

**Tasks** — everything the scheduler is managing, with priority, deadline, and status.

**Booking** — a local mock‑up of a public booking page that reuses the scheduler's free‑slot logic.

**Settings** — working hours, model choice, inference server URL, and Google Calendar connection.

---

## Google Calendar sync (optional)

Two‑way sync with your **primary** calendar: Google events are pulled in (the scheduler plans
around them) and your events + auto‑scheduled task blocks are mirrored out. It uses your own free
Google OAuth client (~10 minutes, one time):

1. In the [Google Cloud Console → Credentials](https://console.cloud.google.com/apis/credentials),
   create or pick a project.
2. **Enable the Google Calendar API** (APIs & Services → Library → "Google Calendar API" → Enable).
3. **OAuth consent screen** — set it up (External is fine) and add your own Google address under
   *Test users* (while the app is in "testing", only listed users can authorize).
4. **Create credentials → OAuth client ID → Application type: Desktop app.** Copy the **Client ID**
   and **Client secret**.
5. In Pushin → **Settings → Google Calendar**, paste them and click **Connect**. Approve in the
   browser window that opens, and you're synced.

> Loopback redirect + PKCE are used (no redirect URI to register for Desktop clients). Tokens are
> stored in the local SQLite database for now — moving them to the OS keychain is a planned
> hardening step. Pushin‑created blocks are tagged so they reconcile without duplicating.

---

## Downloads & building installers

Pre‑built, one‑click installers are produced by CI and attached to each
[GitHub Release](https://github.com/Ilakkiyan/Pushin/releases):

| OS | Installer |
|----|-----------|
| **Windows** | `.msi` and a one‑click NSIS `.exe` |
| **macOS** | `.dmg` (separate Apple‑Silicon and Intel builds) |
| **Linux** | `.AppImage` and `.deb` |

Download the file for your OS and run it. (Builds are currently **unsigned**, so on first launch
macOS Gatekeeper / Windows SmartScreen may ask you to confirm.)

**Cutting a release.** Each installer must be built on its own OS — a `.dmg` can only be produced
on macOS, a Windows `.exe` only on Windows — so [`.github/workflows/release.yml`](.github/workflows/release.yml)
fans out to per‑OS GitHub runners via [`tauri-action`](https://github.com/tauri-apps/tauri-action).
Publish a release by pushing a version tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds every platform in parallel and creates a **draft** Release with the installers
attached — review it and click *Publish*. (You can also trigger it from the **Actions** tab.)

**Building locally** (your current OS only):
```bash
npm run tauri build
```

---

## Development

```bash
npm install            # install frontend dependencies
npm run tauri dev      # run the app with hot reload (Rust rebuilds, Vite HMR)
npm run build          # type‑check + bundle the frontend (tsc && vite build)

# Rust core
cd src-tauri
cargo test             # run the scheduler / parser / habit unit tests
cargo build            # compile the backend
```

> **Testing the model directly:** while the app is running, `llama-server` is live on
> `:8080`. You can `POST` to `/v1/chat/completions` with a `json_schema` body to validate parser
> behavior without the GUI — invaluable when tuning prompts.

---

## Project layout

```
Pushin/
├─ src/                     # React + TypeScript frontend
│  ├─ panes/                # CalendarPane (week), MonthPane, HabitsPane,
│  │                        #   ChatPane, TaskListPane, BookingPane, SettingsPane
│  ├─ components/           # TopBar, InferenceSetup, ConflictBanner
│  ├─ state/store.ts        # Zustand store (SQLite is the source of truth)
│  └─ lib/                  # ipc.ts (typed commands) · time.ts (date helpers)
├─ src-tauri/               # Rust backend
│  ├─ src/
│  │  ├─ commands.rs        # the Tauri IPC surface
│  │  ├─ parser.rs          # NL → structured plan (dates/ranges resolved here)
│  │  ├─ scheduler.rs       # the auto‑scheduler (core IP) + tests
│  │  ├─ habits.rs          # streak/consistency math + calendar slot finder + tests
│  │  ├─ db.rs              # SQLite persistence + migrations
│  │  ├─ model.rs           # shared domain types
│  │  ├─ model_manager.rs   # model + engine auto‑download, llama‑server lifecycle
│  │  └─ calendar/          # Google two‑way sync
│  └─ migrations/           # versioned SQL schema
└─ .github/workflows/       # release pipeline (exe / dmg / AppImage)
```

The app's data lives **outside** the repo, in your OS app‑data folder under `com.pushin.app/`:
the SQLite database (`pushin.db`), downloaded models (`models/*.gguf`), and the llama.cpp engine
(`bin/`).

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| **"AI offline"** pill stays amber | Open the chat setup card and download a model, or start Ollama and set its URL in Settings. |
| Model download is slow | The 3B model is ~2 GB; it's a one‑time download. The 1.5B model is smaller if you're bandwidth‑limited. |
| Parsing is inconsistent | Small models are prompt‑sensitive — switch to the **7B** model in Settings for more reliable multi‑step parsing. |
| `npm run tauri dev` fails to compile Rust | Re‑check the OS prerequisites above (especially the C++ build tools / `webkit2gtk` dev package). |
| Build error about `webkit2gtk` on Linux | Install `libwebkit2gtk-4.1-dev` (Tauri 2 uses 4.1, not 4.0). |
| Google "Connect" fails | Make sure the Calendar API is enabled and your address is added as a **test user** on the consent screen. |

---

## Privacy

Pushin is **on‑device by design**. The language model runs locally (llama.cpp or Ollama bound to
`127.0.0.1`), and your tasks, events, and habits live in a local SQLite database. The **only**
network calls Pushin makes are the optional one‑time model download and — if *you* connect it —
Google Calendar sync.

---

## Tech stack

**Shell:** Tauri 2 · **Frontend:** React 18 + TypeScript + Vite + Tailwind + Zustand ·
**Backend:** Rust (rusqlite, reqwest, chrono) · **Inference:** llama.cpp `llama-server` /
Ollama (OpenAI‑compatible, `response_format: json_schema`) · **Models:** Qwen2.5 1.5B / 3B / 7B
(4‑bit GGUF).

Targets macOS (arm64), Windows (x64/arm64), and Linux (x64/arm64).
