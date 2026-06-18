<div align="center">

# 📌 Pushin

**A local‑AI, Motion‑style calendar — and a local‑first second brain.** Describe your day in plain
language and a small language model running **100% on your device** turns it into tasks and events,
which a deterministic Rust auto‑scheduler packs into your calendar around your fixed commitments.
Then keep everything else — notes, ideas, knowledge — in a **Notion‑style document vault** with
**Obsidian‑style `[[wikilinks]]` and a connection graph**, all in the same app.

No cloud. No account. Nothing leaves your machine.

**Docs:** [ilakkiyan.github.io/Pushin](https://ilakkiyan.github.io/Pushin/)

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
| 📓 **Notion‑style document vault** | A block editor (headings, lists, quotes, slash menu) with nested pages, organized in a sidebar tree. Notes are first‑class documents, not scratch text. |
| 🕸️ **Obsidian‑style links & graph** | Type `[[` to link any page to another; each page shows its **backlinks**, and a force‑directed **connection graph** visualizes how your knowledge ties together. |
| 🔮 **On‑device semantic memory (Hermes)** | Notes, tasks, events, and people are embedded locally so recall and search find what you mean, not just keyword matches — with a keyword fallback that always works. |
| 📆 **Daily notes** | One page per calendar day, opened straight from the week/month grid — the bridge between your time and your knowledge. |
| 🪢 **Notes ↔ tasks & events** | Link any task or event to a page; the calendar becomes an index into your knowledge, and pages show their linked work. |
| ✨ **AI that uses your notes** | The planner auto‑recalls relevant notes (e.g. "Sarah prefers afternoons"), offers to remember durable facts from chat, and can **answer questions over your vault** with citations — all on‑device. |
| 🧩 **One shared context (Context Engine)** | Notes, tasks, events, and people are indexed into one cross‑entity recall layer, so the AI surfaces what's relevant no matter where it lives. |
| 👥 **People (private CRM)** | Anyone who books time becomes a person record with their notes and meeting history — relationships, on‑device. |
| ☀️ **Daily briefing** | A morning at‑a‑glance banner above the calendar: today's events, what's due, and how much focus time is already blocked. |
| ⏱️ **Focus timer + adaptive scheduling** | Track real time on a task; the scheduler learns how long things actually take you and biases future estimates. |
| 🤝 **Meeting companion** | Open a meeting to see who's attending and your history with them, then turn its notes into action‑item tasks you confirm. |
| 🏷️ **Auto‑labeling** | Pushin suggests existing labels that match an item's text — one tap to apply, never automatic. |
| 📅 **Public booking page** | A Calendly‑style page served from your real availability by a local server + a tunnel you run. |
| 📥 **Quick capture + Inbox** | `Cmd/Ctrl+Shift+N` to jot anything into an Inbox; sort it later into a task, event, or note. One box, zero decisions. |
| 📦 **Import your vault** | Bring in an Obsidian / Markdown folder — files become pages, `[[links]]` become connections. |
| ⌘ **Command palette + action bar** | `Cmd/Ctrl‑K` for semantic search, jump to any page/view, **ask your vault**, or **run a natural‑language command** (create/move/cancel from anywhere). |
| 📅 **Week view** | Full 24‑hour week grid with drag‑to‑move and pin‑to‑lock; re‑plans around your changes. |
| 🗓️ **Month view** | Google‑Calendar‑style month grid with per‑day event chips; click a day to jump to its week. |
| 🔥 **Habit tracker** | Build habits with streaks, a consistency heatmap, and one‑click "add to today's calendar" that slots a habit into a free gap. |
| 🌙 **Personalized routine** | A first‑run welcome captures your sleep, working hours, and recurring blocked time (meals, gym, commute); the scheduler keeps them free and the AI plans around them. |
| ✅ **Task list** | Auto‑scheduled work blocks with status, priority, and deadlines. |
| 🔗 **Two‑way Google Calendar sync** | Mirror events and task blocks to your primary calendar (optional). |
| 🪟 **Polished desktop shell** | A collapsible left sidebar and a custom frameless title bar that auto‑hides when maximized/fullscreen (reveals on a top‑edge hover; F11 toggles fullscreen). |
| 🔒 **100% on‑device** | Both the language model and the embedding model run locally via llama.cpp or Ollama. The app works fully offline. |

---

## How it works

```
            ┌─────────────────────────────────────────────┐
            │  React UI  (sidebar · chat · week · month ·  │
            │   habits · vault editor · graph · ⌘K palette)│
            └───────────────────────┬─────────────────────┘
                                    │  Tauri IPC (typed commands)
            ┌───────────────────────▼─────────────────────┐
            │  Rust core                                   │
            │   • parser     NL → events/tasks (dates &    │
            │                ranges resolved in Rust)      │
            │   • scheduler  free‑slot packing + conflicts │
            │   • habits     streaks / consistency / slots │
            │   • hermes     vault pages: embeddings +     │
            │                cosine/keyword recall + links │
            │   • db         SQLite (source of truth)      │
            │   • calendar   two‑way Google sync           │
            └─────────┬───────────────────────┬───────────┘
                      │ spawns (×2)            │ HTTPS (optional)
                      ▼                        ▼
   llama.cpp `llama-server`              Google Calendar API
   (chat :8080 + embeddings :8181,
    local, OpenAI‑compatible)
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

On first launch the chat panel shows a **setup card** listing three models — pick one and click
**Download**. Pushin fetches the model *and* the llama.cpp engine for your OS automatically and starts
the server on `http://127.0.0.1:8080`. The status pill at the bottom of the sidebar turns green
("AI ready") when it's up. (A second, tiny embedding model for the vault's semantic memory downloads
automatically the first time too — ~37 MB, no setup.)

> Three tiers, pick for your machine:
> - **Lite — Qwen2.5 3B** (~2 GB): lightest and fastest; the default download, runs almost anywhere.
> - **Recommended — Qwen2.5 7B** (~4.7 GB): the most reliable multi‑step parsing; needs ~6 GB RAM.
> - **Most powerful — Qwen2.5 14B** (~9 GB): highest accuracy for a strong machine (~12 GB RAM), slowest.
>
> You can switch model any time in **Settings**.
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

Everything is reachable from the **left sidebar** (collapse it with the toggle for more room), or
instantly via the **`Cmd/Ctrl‑K` command palette** — search any page, jump to any view, or create a
page without leaving the keyboard.

**Chat (right panel).** Describe events and work in plain language:
- *"Lunch with mom Friday 12–2 and a graduation party from 6–10."* → two events.
- *"I need to study for the exam, about 4 hours, due Thursday."* → a task the scheduler places.
- *"Make the meeting today 2 hours instead of 1."* → edits the existing event.
- *"Move the dentist to tomorrow at 3pm."* → reschedules it.

**Week** — the full 24‑hour grid. Drag a task block to move it; double‑click to pin it (pinned
blocks survive re‑planning). Click empty space to add a fixed/busy event. Your **reserved time**
(sleep + routines from Settings) is shaded behind the grid so the gaps read as the time you actually
have free.

**Month** — a Google‑Calendar‑style overview. Each day shows little chips for its events and task
blocks; click any day to jump to that week.

**Habits** — add a habit with a color and a duration. Check it off for the day, see your **current
streak**, **longest streak**, and **30‑day consistency**, and click squares in the heatmap to
backfill past days. Hit **Add to today** to slot the habit into a free gap on your calendar (it
lands near the end of the day, tucked between existing commitments) — the scheduler then plans your
tasks around it.

**Tasks** — everything the scheduler is managing, with priority, deadline, and status.

**Notes (the vault)** — your second brain. Hit **+** in the sidebar's Pages tree to create a page,
then write in a **block editor**: type `/` for headings, lists, quotes, and more; nest pages under
each other to build a tree. Type **`[[`** to link to another page (pick an existing one or create it
on the fly) — the link becomes a clickable chip, and the target page lists every page that links to
it under **Linked references**. Pages are embedded on‑device, so recall and search understand
meaning, not just exact words.

**Daily notes & links** — hover any day in the week/month grid and click the note icon (or "Today's
note" in the sidebar) to open that day's page. Link a task or event to a page from its **Notes**
action; the page lists its linked work under "Linked tasks & events".

**Inbox (quick capture)** — press `Cmd/Ctrl+Shift+N` anywhere to jot a thought into the Inbox without
deciding what it is. Later, from the **Inbox**, hit **Plan with AI** to turn it into a task/event,
**Keep as note**, or delete it.

**Ask your vault & import** — in the `Cmd/Ctrl‑K` palette, type a question and choose **Ask your
vault** for an on‑device answer with citations; search is semantic when the memory engine is up. Bring
existing notes in with the **import** button (↓) in the sidebar's Pages header — pick an Obsidian or
Markdown folder and your files + `[[links]]` come across.

**Graph** — an Obsidian‑style **connection graph** of your whole vault: every page is a node (bigger
= more links), every `[[link]]` is an edge. Click a node to open that page. It's the bird's‑eye view
of how your notes connect.

**Booking** — a tunnel-ready public booking page served by Pushin while the app is running. Share a
tokenized event-type link through ngrok or Cloudflare Tunnel; confirmed bookings become fixed events
and the scheduler replans around them.

**Settings** — working hours, **your routine** (sleep window + recurring blocked time like meals,
gym, or the commute), model choice, inference server URL, and Google Calendar connection. The
scheduler keeps your routine free and the AI plans around it — the same questions are asked once in a
first‑run welcome, and you can change them here anytime.

---

## Sync across your devices (optional)

Run Pushin on more than one computer and keep them in sync **without any cloud or account**. Your
devices form a small **private peer‑to‑peer network** (built on [Iroh](https://www.iroh.computer/)):
data flows **directly device‑to‑device, end‑to‑end encrypted**, joined by a single shared key — true
to Pushin's local‑first, on‑device spirit.

**How to pair two devices**

1. On the first device, open **Settings ▸ Devices & sync** and click **Create invite code**. Copy the
   code that appears.
2. On the second device, open the same screen, paste the code into **Join a network**, and click
   **Join**. That's it — the two devices now sync automatically (and on demand via **Sync now**).
3. Add more devices the same way (create an invite on any paired device).

**Good to know**

- **What syncs:** your tasks, events, projects, habits, vault pages + links, labels, people, and focus
  sessions. Device‑specific settings (your AI model, Google connection) stay local to each device.
- **Conflicts** are resolved last‑writer‑wins (the most recent edit to an item wins). Deletes propagate.
- **Privacy:** connections are end‑to‑end encrypted. By default Pushin uses public relay servers to help
  your devices find each other across networks (they only ever see encrypted traffic). For maximum
  privacy you can switch to **LAN/direct‑only** in the same settings — devices then connect only when
  reachable directly (e.g. on the same Wi‑Fi).
- **Leaving:** **Leave this sync network** forgets the shared key and your paired devices on that device.
- This feature is new; the underlying logic is unit‑tested, but the real proof is two machines talking —
  if a device doesn't appear, try **Sync now**, and keep both apps open.

---

## Google Calendar sync (optional)

Two‑way sync with your **primary** calendar: Google events are pulled in (the scheduler plans
around them) and your events + auto‑scheduled task blocks are mirrored out.

There are three one‑time stages: **A.** create your own Google OAuth client, **B.** connect it in
Pushin, and **C.** flip it to "production" so it keeps working. Budget ~10–15 minutes total. Read the
short safety note first — the setup *looks* scarier than it is, and knowing why each step exists makes
it obvious what's safe.

### Is this safe? (read this first)

Yes. Here's the whole trust model in four points:

- **There are no Pushin servers.** Your computer talks to Google directly. Nothing routes through us
  or anyone else — we never see your calendar, your Google account, or your credentials.
- **You create your own Google "app."** Rather than trust a stranger's app with your calendar, you
  spend a few minutes making a personal OAuth client that only *you* own and control. That single fact
  is the reason for every step below.
- **You'll hit two scary‑looking screens — both are normal and expected:**
  - **"Google hasn't verified this app"** during sign‑in. That's *only* because the app is yours and
    you (correctly) never submitted it to Google for review. You click **Advanced → Go to Pushin
    (unsafe)** to continue. This is the single most common point of confusion — it is not a real
    warning about Pushin.
  - **"See, edit, share, and permanently delete all the calendars you can access"** on the permission
    screen. This is the *only* Calendar permission Google offers — there is no "just my primary
    calendar" checkbox. Pushin only ever touches your **primary** calendar, and only on your machine.
- **The "Client secret" isn't really a secret here.** Desktop apps can't keep secrets, so Google's
  flow uses **PKCE** — a one‑time random proof your machine generates per sign‑in — as the actual
  security. The "secret" is just an identifier that never leaves your computer.

### Part A — Create your Google OAuth client (one time)

Do this in the [Google Cloud Console](https://console.cloud.google.com/). Google has reorganized
this UI a few times; menu names below are the current ones, with the older labels in parentheses.

**1. Create (or select) a project.**
   - Open the **project picker** in the top bar → **New Project** → give it any name (e.g. "Pushin")
     → **Create**. Wait for it to finish, then make sure that project is selected in the top bar.
   - *(If you already have a project you don't mind reusing, just select it.)*

**2. Enable the Google Calendar API.**
   - Go to **APIs & Services → Library**
     ([direct link](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com)),
     search **"Google Calendar API"**, open it, and click **Enable**.
   - If you skip this, connecting "succeeds" but every sync fails with a *Calendar API has not been
     used / is disabled* error.

**3. Configure the consent screen** (**APIs & Services → OAuth consent screen**, newer Console calls
   this **Google Auth Platform**).
   - **User type / Audience:** choose **External**. *(Internal only exists if you're on Google
     Workspace and only allows people in your org.)*
   - **App information:** an **App name** (e.g. "Pushin") and your email as the **User support email**
     and **Developer contact**. Everything else can stay blank.
   - **Test users** (under **Audience** in the new UI): click **Add users** and add **your own Gmail
     address** — the exact account whose calendar you'll sync. While the app is in **Testing** mode,
     only accounts listed here are allowed to authorize.
   - You do **not** need to add a logo or submit the app for Google's verification review. (You *will*
     publish it to "production" in **Part C** — that's a one‑click toggle for your own use, not the
     verification review. Skipping Part C just means re‑connecting weekly; see below.)

**4. Create the OAuth client.**
   - Go to **APIs & Services → Credentials → Create credentials → OAuth client ID**
     (newer UI: **Google Auth Platform → Clients → Create client**).
   - **Application type: Desktop app.** This is important — Pushin uses a loopback redirect that only
     Desktop clients allow, so there's **no redirect URI to fill in**. (Web‑application clients will
     *not* work here.)
   - Give it a name and click **Create**.

**5. Copy the Client ID and Client secret.**
   - The instant you click **Create**, a dialog pops up with your **Client ID** (ends in
     `.apps.googleusercontent.com`) and **Client secret** (starts with `GOCSPX-`). Copy both — you'll
     paste them into Pushin in Part B.
   - **Closed the dialog too soon?** Nothing is lost; these values don't change. Go back to
     **APIs & Services → Credentials** (newer UI: **Google Auth Platform → Clients**) and **click your
     OAuth client's name** to reopen its detail page — the Client ID and secret are both shown there.
     The **⬇ Download JSON** button gives you a file containing both as well.

### Part B — Connect in Pushin

**1.** Open **Settings → Google Calendar**, paste the **Client ID** and **Client secret**, and click
   **Connect Google Calendar**. Pushin saves them and opens your browser.

**2.** Pick the Google account you added as a test user.

**3.** You'll see **"Google hasn't verified this app."** This is normal for an unpublished personal
   app. Click **Advanced** (bottom‑left) → **Go to Pushin (unsafe)**. It is safe: this is the client
   *you* just created, and the whole exchange happens locally on your machine.

**4.** Approve the requested access — Pushin asks for **See, edit, share, and permanently delete all
   the calendars you can access** (the Calendar scope) plus your email/profile. Click **Continue /
   Allow**.

**5.** The browser tab shows **"📌 Pushin is connected to Google Calendar"** — close it. Back in
   Pushin, Settings now shows a green **● Connected** pill. Click **Sync now** for the first sync;
   after that Pushin syncs automatically as you make changes.

### Part C — Make it permanent (publish your app)

A brand‑new OAuth app starts in **Testing** mode, and Google **revokes its access after 7 days** — so
sync will quietly stop working about once a week and you'll have to reconnect. To make it permanent,
take 30 seconds to move the app to **Production**.

> **This does *not* mean publishing Pushin to the world, submitting anything to Google, or waiting for
> a review.** "Production" here is just a status flag on *your own* OAuth app that lifts the 7‑day
> clock. Your data and the whole flow stay exactly as private as before.

**1.** Go to **APIs & Services → OAuth consent screen** (newer UI: **Google Auth Platform → Audience**).

**2.** Under **Publishing status: Testing**, click **Publish app**, then **Confirm** in the dialog.

**3.** The status now reads **In production** — and refresh tokens stop expiring weekly. Done.

**About the verification prompt.** Because the Calendar scope is "sensitive," Google may show a note
that the app "requires verification." **You do not need to complete it for personal use** — you can
ignore/dismiss the prompt. Verification only does two things: it removes the "Google hasn't verified
this app" warning, and it lets you exceed 100 users. So your app simply stays **In production
(unverified)**: you'll click past that one warning screen the *first* time you connect (Part B,
step 3), but your sign‑in no longer expires every 7 days.

> **"…so other people can use it."** Pushin isn't a hosted service — each person runs their own copy
> with their own OAuth client (Parts A–C above). If you want to let a *handful* of others use *your*
> client, an **unverified Production** app allows up to **100 users** total; while in **Testing** you'd
> instead add each of them under **Test users**. Only opening it to the general public — removing the
> warning screen or going past 100 users — requires Google's verification review.

### Good to know

- **Loopback + PKCE.** Pushin listens on `http://127.0.0.1:<random‑port>` for the one‑time redirect,
  so there's nothing to register and no secret ever leaves your machine in a URL.
- **Scopes requested:** `…/auth/calendar` (full read/write on your calendars), `openid`, and `email`.
- **Keep it permanent.** A **Testing**‑mode app loses access every 7 days; publishing it to
  **Production** (Part C) stops that, with no Google review required for personal use.
- **Where tokens live:** in Pushin's local SQLite DB for now (moving them to the OS keychain is a
  planned hardening step). Disconnect from Settings to remove them.
- **No duplicates:** Pushin tags the task blocks it pushes so re‑syncs reconcile them instead of
  creating copies.

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

# Frontend tests (Vitest + Testing‑Library + jsdom)
npm test               # unit + component + IPC‑contract tests
npm run coverage       # the same, with a coverage report
npm run test:e2e       # Playwright mocked‑IPC end‑to‑end (drives the real React app)

# Rust core
cd src-tauri
cargo test --lib       # scheduler / parser / habits / db / hermes / booking + httpmock integration
cargo build            # compile the backend
```

> **Layered test suite** (CI: [`.github/workflows/test.yml`](.github/workflows/test.yml)): Rust unit +
> `httpmock` integration (LLM client, embeddings, Google Calendar leaf fns), Vitest unit/component +
> an **IPC contract test** (catches command drift), and Playwright **mocked‑IPC E2E**. The live model
> battery (`cargo test --test llm_eval -- --ignored`, needs a running `:8080`) stays a manual gate.

> **Testing the model directly:** while the app is running, `llama-server` is live on
> `:8080`. You can `POST` to `/v1/chat/completions` with a `json_schema` body to validate parser
> behavior without the GUI — invaluable when tuning prompts.

---

## Project layout

```
Pushin/
├─ src/                     # React + TypeScript frontend
│  ├─ panes/                # CalendarPane (week), MonthPane, HabitsPane, ChatPane,
│  │                        #   TaskListPane, VaultPane (editor), GraphPane, BookingPane, SettingsPane
│  ├─ components/           # Sidebar, TitleBar, VaultTree, PageEditor (BlockNote),
│  │                        #   CommandPalette, InferenceSetup, ConflictBanner
│  ├─ state/store.ts        # Zustand store (SQLite is the source of truth)
│  └─ lib/                  # ipc.ts (typed commands) · time.ts · blocks.ts / editorSchema.tsx (editor)
├─ src-tauri/               # Rust backend
│  ├─ src/
│  │  ├─ commands.rs        # the Tauri IPC surface
│  │  ├─ parser.rs          # NL → structured plan (dates/ranges resolved here)
│  │  ├─ scheduler.rs       # the auto‑scheduler (core IP) + tests
│  │  ├─ habits.rs          # streak/consistency math + calendar slot finder + tests
│  │  ├─ hermes.rs          # vault memory: on‑device embeddings + cosine/keyword recall
│  │  ├─ db.rs              # SQLite persistence + migrations (incl. pages + links) + tests
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
| Model download is slow | It's a one‑time download. The **lite 3B** (~2 GB) is the smallest — start there if you're bandwidth‑limited and upgrade to the 7B/14B later. |
| Parsing is inconsistent | Small models are prompt‑sensitive — switch to the **7B** model in Settings for more reliable multi‑step parsing. |
| `npm run tauri dev` fails to compile Rust | Re‑check the OS prerequisites above (especially the C++ build tools / `webkit2gtk` dev package). |
| Build error about `webkit2gtk` on Linux | Install `libwebkit2gtk-4.1-dev` (Tauri 2 uses 4.1, not 4.0). |
| "Google hasn't verified this app" during sign‑in | Expected for a personal unpublished app. Click **Advanced → Go to Pushin (unsafe)** — it's your own client. |
| Sign‑in shows "Access blocked / app is being tested" | The account you're signing in with isn't a **test user**. Add it under the consent screen's **Test users / Audience**. |
| Connect succeeds but **Sync now** errors with *Calendar API disabled* | You skipped Part A‑2 — **enable the Google Calendar API** in the Library, then Sync again. |
| `Error 400: redirect_uri_mismatch` | Your OAuth client is the wrong type. It must be **Desktop app**, not Web application — recreate it. |
| "no refresh_token returned" on Connect | Google only returns one once. **Disconnect**, then remove Pushin at [your Google account's app permissions](https://myaccount.google.com/permissions), and **Connect** again. |
| Sync silently stops working after about a week | Your app is still in **Testing** mode (tokens expire every 7 days). Publish it to **Production** — see [Part C](#part-c--make-it-permanent-publish-your-app). |

---

## Privacy

Pushin is **on‑device by design**. The language model runs locally (llama.cpp or Ollama bound to
`127.0.0.1`), and your tasks, events, and habits live in a local SQLite database. The **only**
network calls Pushin makes are the optional one‑time model download and — if *you* connect it —
Google Calendar sync.

---

## Tech stack

**Shell:** Tauri 2 (frameless custom title bar) · **Frontend:** React 19 + TypeScript + Vite +
Tailwind + Zustand · **Editor & graph:** BlockNote (block editor) + react‑force‑graph (connection
graph) · **Backend:** Rust (rusqlite, reqwest, chrono) · **Inference:** llama.cpp `llama-server` /
Ollama (OpenAI‑compatible, `response_format: json_schema`) · **Models:** Qwen2.5 3B / 7B / 14B
(4‑bit GGUF) for chat + bge‑small‑en‑v1.5 (~37 MB) for on‑device embeddings.

Targets macOS (arm64), Windows (x64/arm64), and Linux (x64/arm64).
