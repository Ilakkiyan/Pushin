# Pushin roadmap — the "one solution for all productivity needs"

The thesis: the moat isn't any single feature — it's a **shared context layer** that every
feature reads from and writes to, so the on-device LLM always has your whole life loaded.
Pushin already has the right primitives (SQLite as truth, `hermes` recall + embeddings, the
parser, the deterministic scheduler, the `entity_links`/`labels` graph). This roadmap turns
those into a spine, then hangs features off it.

Honors the locked product decisions: **on-device only** inference, **desktop-first**,
**LLM parses / deterministic solver schedules**, **privacy**.

---

## 0. Keystone — the Context Engine (the connective tissue)

Everything else is a client of this. One module that answers: *"given this intent, what does
the model need to know?"*

- **Universal embeddings** — extend the embedding index beyond vault pages to tasks, events,
  people, goals, bookings via a polymorphic `entity_index` table (mirrors `entity_labels`/
  `entity_links`). `hermes::rank_notes` generalizes to `rank_items` over a common `ContextItem`.
- **Unified graph** — `page_links` + `entity_links` + `entity_labels` treated as one knowledge
  graph; "neighbors of X" becomes a primitive.
- **Context assembler** — given an intent (`plan`/`ask`/`brief`/`label`/`review`), retrieves
  semantic recall (cross-entity) + graph neighbors + recent activity + durable user-memory facts
  (`extract_memories`), then assembles a **token-budgeted** prompt. This is *the* shared context.
- **Deterministic-first**: the engine retrieves; the LLM only extracts/synthesizes; the scheduler
  still solves.

Detailed Phase-1 implementation plan: see the Context Engine plan (grounded in `hermes.rs`/`db.rs`).

---

## The feature set — one closed loop: Capture → Organize → Plan → Execute → Reflect

### Capture
1. **Universal Capture & Router** — one surface (text, voice→on-device STT later, web clipper,
   read-only email/calendar ingest) where the LLM routes each item to task/event/note/person/goal.
   Extends `QuickCapture`/Inbox + the parser. *Reads:* —. *Writes:* all entity kinds. *LLM:* parse+route.

### Organize
2. **People / Relationship layer (private CRM)** — first-class `people` entities auto-created from
   booking invitees, event attendees, `[[mentions]]`; each gets a page with interaction history and
   owed follow-ups. Makes the booking flow feed the rest of the app. *LLM:* extract.
3. **Goals & Projects (outcome hierarchy)** — goals → projects → tasks; the LLM decomposes a goal
   into milestones; progress rolls up. Extends `projects`/`tasks`/`task_deps`. *LLM:* decompose.
4. **Auto-labeling & entity linking** — keyword + embedding post-pass that tags and cross-links
   entities with confirm-chips (the documented labeling TODO). Populates the graph the engine reads.

### Plan
5. **Planning Rituals** — AM briefing (engine assembles today from calendar + due tasks + recalled
   notes + people you're meeting) and PM reflection that rolls incomplete tasks forward into the
   daily note. *LLM:* synthesize.
6. **Adaptive Scheduler + Energy model** — learns real task durations and best time-of-day windows
   from tracked actuals, feeding estimates + label prefs into `schedule_with_prefs`. Closes the loop
   on the scheduler's guessed 60-min defaults. *LLM:* none (deterministic).
7. **Natural-language Action Bar** — ⌘K graduates from navigate/search to **act** ("reschedule my
   week around the dentist"). Parser + scheduler + engine behind one input. *LLM:* parse.

### Execute
8. **Meeting Companion** — pre-meeting brief (everything about attendees + topic via the engine),
   a linked notes page during, post-meeting action-item extraction → tasks. Ties calendar ↔ people ↔
   vault ↔ tasks. *LLM:* brief + extract.
9. **Focus Mode & time tracking** — focus sessions on a task; actuals feed feature 6. The data
   exhaust is the product's intelligence. *LLM:* none.
10. **Proactive Assistant** — surfaces conflicts, overdue tasks, stale goals, follow-ups owed, prep
    needed. **Deterministic rules detect; the LLM only phrases.** Native notifications.

### Reflect
11. **Ask-your-life RAG** — extends ask-your-vault to query across all entities with citations.
    The engine's read side as a first-class feature. *LLM:* synthesize + cite.
12. **Reviews & Analytics** — LLM-generated weekly/quarterly retrospectives over the unified data:
    where time went, goal progress, neglected projects/people. *LLM:* synthesize.
13. **Templates & Workflows** — recurring structures ("project kickoff" spawns pages + tasks +
    events) and simple automations ("urgent task due <24h → surface it"). Deterministic where possible.

**Enabling infra — Encrypted cross-device sync:** generalize the booking tunnel/relay learnings into
a private E2E-encrypted backbone so "one solution" survives leaving the desktop.

---

## How they compound (proof it's one product, not 13 apps)

| Feature | Reads shared layer | Writes back | LLM role |
|---|---|---|---|
| Universal Capture | — | tasks/events/notes/people/goals | parse + route |
| People layer | bookings, events, mentions | person entities, follow-ups | extract |
| Goals/Projects | tasks, labels | project DAG, milestones | decompose |
| Auto-label | all entities, embeddings | labels, entity_links | extract |
| Planning Ritual | calendar, tasks, recall, people | daily note, rollovers | synthesize |
| Adaptive scheduler | focus actuals, labels | estimates, schedule | none |
| Action Bar | everything | mutations | parse |
| Meeting Companion | people, vault, calendar | notes, action tasks | brief + extract |
| Proactive Assistant | everything | notifications | phrase only |
| Ask-your-life | everything (RAG) | — | synthesize + cite |

**A day in the life:** Forward a meeting invite → *Capture* routes it to an event and creates the
*Person*. Morning *Briefing* notes you're meeting them and surfaces the *Meeting Companion* brief
from past notes. *Focus* on prep teaches the *Adaptive Scheduler* prep takes you 25 min, not 60.
Post-meeting, action items become *tasks* under the right *Goal*. The evening *Reflection* rolls the
unfinished one forward; Sunday's *Review* says the goal is on track. Every step reads and writes the
same spine.

---

## Sequencing

1. **Context Engine + universal embeddings** — dependency for everything, low-risk (extends `hermes`).
2. **People layer + Auto-labeling** — populate the graph the engine needs.
3. **Planning Rituals + Action Bar + Ask-your-life** — high daily value, mostly orchestration.
4. **Meeting Companion + Focus/Adaptive scheduler** — the execution loop + the data that makes
   scheduling smart.
5. **Proactive + Reviews + Templates** — best once there's rich data to act on.
6. **Encrypted sync** — when going multi-device.

## Risks (so this stays credible)
- **The 3B reliability ceiling is the real constraint.** Extract/route features inherit gotcha #1 —
  keep them deterministic-recoverable + confirm-chip gated; lean on the 7B. Synthesis features are
  more forgiving.
- **Keep the scheduler deterministic** — the LLM feeds preferences, never solves constraints.
- **Email/voice ingest parses on-device** to honor the privacy thesis; any cloud STT/email API is
  opt-in and clearly marked.
- **Scope:** ~a year of work. The Context Engine is the only truly load-bearing piece — engine +
  3–4 loop features already make the product *feel* unified.
