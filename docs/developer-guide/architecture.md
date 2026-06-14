# Architecture

Pushin is a Tauri 2 desktop app.

```text
React UI
  -> typed Tauri commands
Rust core
  -> SQLite, parser, scheduler, vault memory, Google sync
Local model servers
  -> chat on 8080, embeddings on 8181
```

## Core Rule

The LLM parses. Rust schedules.

Small local models are intentionally kept away from date math, conflict resolution, and dependency scheduling.

## Storage

SQLite is the source of truth for settings, projects, tasks, events, blocks, habits, pages, links, labels, Google sync state, and booking metadata.
