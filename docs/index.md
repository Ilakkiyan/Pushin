---
layout: home

hero:
  name: Pushin
  text: Local-first AI calendar and second brain.
  tagline: Describe your day in plain language. Pushin parses it locally, schedules deterministically, and keeps your notes connected.
  image:
    src: /app-overview.svg
    alt: Pushin calendar and notes overview
  actions:
    - theme: brand
      text: Get Started
      link: /getting-started/
    - theme: alt
      text: User Guide
      link: /user-guide/planning
    - theme: alt
      text: Developer Guide
      link: /developer-guide/architecture

features:
  - title: 100% local-first
    details: Inference, scheduling, notes, embeddings, and SQLite storage stay on your machine.
  - title: Deterministic scheduling
    details: The LLM extracts structure; Rust does the date math and calendar packing.
  - title: Calendar plus vault
    details: Week/month planning, task blocks, habits, daily notes, wikilinks, graph, and semantic recall in one app.
---

## Quick Links

- [Install prerequisites](/getting-started/prerequisites)
- [Run Pushin from source](/getting-started/run-from-source)
- [Download installers](/getting-started/releases)
- [Set up the local AI](/getting-started/ai-setup)
- [Connect Google Calendar](/user-guide/google-calendar)
- [Contribute or run tests](/developer-guide/testing-releases)

## Privacy Model

Pushin is designed around local ownership. There is no Pushin server, no account requirement, and no cloud fallback. Optional Google Calendar sync talks directly from your machine to Google using credentials you control.

## Repository

The app and these docs live in the same repository: [github.com/Ilakkiyan/Pushin](https://github.com/Ilakkiyan/Pushin).
