import type { Page } from "@playwright/test";

/**
 * Install a fake Tauri IPC bridge into the page before the app loads. The real React app calls
 * `invoke(cmd, args)` → `window.__TAURI_INTERNALS__.invoke`, so we implement that against in-memory
 * state. Covers boot + the headline vault/inbox flows; `plugin:*` calls (window controls, events)
 * return safe defaults so the frameless TitleBar and listeners don't throw.
 */
export async function installMockBridge(page: Page) {
  await page.addInitScript(() => {
    /* eslint-disable @typescript-eslint/no-explicit-any */
    // Suppress the post-update "What's New" overlay in E2E — it appears asynchronously after
    // getVersion() resolves and intercepts pointer events, flaking slower flows. Seed lastSeenVersion
    // to match the mocked app version (below) so `last === v` and the intro never shows.
    try {
      localStorage.setItem("pushin:lastSeenVersion", "0.0.0-e2e");
    } catch {
      /* ignore */
    }
    const state: any = {
      nextId: 1,
      pages: [] as any[],
      inbox: [] as any[],
      people: [] as any[],
      icsSubs: [
        { id: 501, name: "Team calendar", url: "https://example.com/team.ics", color: "#38bdf8", lastSynced: "2026-07-15T09:00:00", createdAt: "" },
        { id: 502, name: "US Holidays", url: "https://example.com/holidays.ics", color: "#f59e0b", lastSynced: "2026-07-15T09:00:00", createdAt: "" },
      ] as any[],
      settings: {
        // `?new` in the URL → a fresh (un-onboarded) user, for capturing the WelcomeGuide.
        onboarded: !new URLSearchParams(window.location.search).has("new"),
        googleConnected: false,
        googleClientId: "",
        googleClientSecret: "",
        timezone: "UTC",
        workStart: "09:00",
        workEnd: "17:00",
        workDays: [1, 2, 3, 4, 5],
        horizonDays: 14,
        bufferMinutes: 0,
        defaultMinChunk: 30,
        defaultMaxChunk: 120,
        llmBaseUrl: "http://127.0.0.1:8080",
        commitments: [],
        sleepEnabled: false,
        sleepStart: "23:00",
        sleepEnd: "07:00",
        // On a BASE model on purpose, so Settings shows the "switch to the tuned model" nudge (Item A).
        modelId: "qwen2.5-7b-instruct-q4_k_m",
        embedModel: "bge-small-en-v1.5-q8_0",
        idleUnloadMinutes: 10,
        archetypes: [],
        aboutMe: "",
        vaultDir: null,
      },
    };

    // A seeded day of scheduled work so the calendar shows task blocks WITH their "why here" reasons
    // (Item C — scheduler explainability). Dated on `today` so it lands on the visible week.
    const today = new Date().toISOString().slice(0, 10);
    const at = (hm: string) => `${today}T${hm}:00`;
    const seedTasks = [
      { id: 1, projectId: null, title: "Draft outline", notes: "", estimatedMinutes: 90, deadline: null, earliestStart: null, priority: 2, minChunkMinutes: 30, maxChunkMinutes: 240, status: "scheduled", createdAt: "", dependsOn: [] },
      { id: 2, projectId: null, title: "Write thesis", notes: "", estimatedMinutes: 180, deadline: at("23:59"), earliestStart: null, priority: 3, minChunkMinutes: 30, maxChunkMinutes: 240, status: "scheduled", createdAt: "", dependsOn: [] },
      { id: 3, projectId: null, title: "Revise draft", notes: "", estimatedMinutes: 90, deadline: null, earliestStart: null, priority: 2, minChunkMinutes: 30, maxChunkMinutes: 240, status: "scheduled", createdAt: "", dependsOn: [1] },
      { id: 4, projectId: null, title: "Prep slides", notes: "", estimatedMinutes: 90, deadline: null, earliestStart: null, priority: 2, minChunkMinutes: 30, maxChunkMinutes: 240, status: "scheduled", createdAt: "", dependsOn: [] },
    ];
    const blk = (id: number, taskId: number, s: string, e: string) => ({ id, taskId, start: at(s), end: at(e), locked: false, provider: null, externalId: null, syncState: null });
    const seedBlocks = [
      blk(101, 1, "08:00", "09:30"), // Draft outline — earliest free slot
      blk(102, 2, "10:00", "11:30"), // Write thesis — has a deadline today
      blk(103, 3, "13:00", "14:30"), // Revise draft — after Draft outline
      blk(104, 2, "15:00", "16:30"), // Write thesis — continuation (part 2)
      blk(105, 4, "16:45", "18:15"), // Prep slides — right after a fixed event
    ];
    const seedEvents = [
      { id: 900, title: "Team meeting", start: at("15:00"), end: at("16:45"), kind: "fixed", source: "manual", createdAt: "", provider: null, externalId: null, accountId: null, etag: null },
    ];
    const seedReasons = [
      { blockId: 101, reason: { kind: "earliest" } },
      { blockId: 102, reason: { kind: "forDeadline", deadline: at("23:59") } },
      { blockId: 103, reason: { kind: "afterDependency", depTitle: "Draft outline" } },
      { blockId: 104, reason: { kind: "continuation", part: 2, of: 2 } },
      { blockId: 105, reason: { kind: "aroundCommitment" } },
    ];
    const tunedModels = [
      { id: "qwen2.5-7b-instruct-q4_k_m", name: "Qwen2.5 7B Instruct (base)", sizeMb: 4680 },
      { id: "pushin-arch7b-chat-tuned-q4_k_m", name: "Pushin 7B (tuned, recommended)", sizeMb: 4470 },
      { id: "pushin-arch3b-tuned-q4_k_m", name: "Pushin 3B (tuned)", sizeMb: 1841 },
    ];
    const titleOf = (p: any) => (p.title && p.title.trim()) || (p.content || "").split("\n")[0]?.slice(0, 80) || "Untitled";
    const lite = (p: any) => ({ ...p, content: "", contentJson: undefined, title: titleOf(p) });

    const handlers: Record<string, (a: any) => any> = {
      load_all: () => ({ settings: state.settings, projects: [], tasks: seedTasks, events: seedEvents, blocks: seedBlocks, eventTypes: [], bookings: [] }),
      reschedule: () => ({ conflicts: [] }),
      explain_schedule: () => seedReasons,
      save_settings: () => null,
      llm_status: () => ({ reachable: true, baseUrl: "http://127.0.0.1:8080", modelPresent: true, modelId: state.settings.modelId, models: tunedModels }),
      list_models: () => tunedModels,
      model_present: () => true,
      recommend_model: () => ({ modelId: "pushin-arch7b-chat-tuned-q4_k_m", reason: "16 GB RAM comfortably runs Pushin's tuned 7B — the most reliable", ramGb: 16, hasGpu: true }),
      ensure_inference: () => "ready",
      restart_inference: () => "ready",
      sleep_inference: () => true,
      ensure_embeddings: () => "ready",
      list_memories: () => [],
      sync_status: () => ({ joined: false, deviceName: "This device", peers: [], relay: null }),
      // Read-only .ics subscriptions (Stage 2 ingestion).
      list_ics_subscriptions: () => state.icsSubs,
      add_ics_subscription: ({ name, url, color }: any) => {
        const sub = { id: state.nextId++, name: name || "Subscribed calendar", url, color: color || "#64748b", lastSynced: "2026-07-15T00:00:00", createdAt: "" };
        state.icsSubs.push(sub);
        return sub;
      },
      refresh_ics_subscriptions: () => state.icsSubs.length * 3,
      remove_ics_subscription: ({ id }: any) => {
        state.icsSubs = state.icsSubs.filter((s: any) => s.id !== id);
        return null;
      },
      // ---- people (private CRM) ----
      // NOTE: these must exist. The unknown-command fallback resolves to `null`, and PeoplePane does
      // `api.listPeople().then(setPeople)` — a null payload resolves successfully, so its .catch never
      // fires and `people.find(...)` throws, unmounting the WHOLE app (there is no error boundary).
      list_people: () => state.people,
      create_person: ({ name, email, notes }: any) => {
        const person = { id: state.nextId++, name: name || "New person", email: email ?? null, notes: notes ?? "", createdAt: "" };
        state.people.push(person);
        return person;
      },
      update_person: () => null,
      delete_person: ({ id }: any) => {
        state.people = state.people.filter((p: any) => p.id !== id);
        return null;
      },
      list_habits: () => [],
      list_event_types: () => [],
      booking_server_status: () => ({ running: false, localUrl: null, host: "127.0.0.1", port: null }),
      start_booking_server: () => ({ running: true, localUrl: "http://127.0.0.1:47610", host: "127.0.0.1", port: 47610 }),
      stop_booking_server: () => ({ running: false, localUrl: null, host: "127.0.0.1", port: null }),
      booking_slots: () => [],
      list_labels: () => [],
      labels_for: () => [],
      labels_for_entities: () => ({}),
      // ---- vault ----
      list_pages: () => state.pages.filter((p: any) => !p.archived && !p.inbox).map(lite),
      get_page: ({ id }: any) => state.pages.find((p: any) => p.id === id) ?? null,
      create_page: ({ title, parentId, content }: any) => {
        const p = { id: state.nextId++, title: title || "Untitled", parentId: parentId ?? undefined, content: content || "", contentJson: undefined, sortOrder: 0, archived: false, inbox: false, indexed: false, createdAt: "", updatedAt: "" };
        state.pages.push(p);
        return p;
      },
      update_page: ({ id, title, content, contentJson }: any) => {
        const p = state.pages.find((x: any) => x.id === id);
        if (p) Object.assign(p, { title, content, contentJson });
        return p ?? null;
      },
      delete_page: ({ id }: any) => {
        state.pages = state.pages.filter((p: any) => p.id !== id);
        return state.pages.filter((p: any) => !p.inbox).map(lite);
      },
      move_page: () => state.pages.filter((p: any) => !p.inbox).map(lite),
      page_backlinks: () => [],
      page_entities: () => [],
      entity_pages: () => [],
      link_page_entity: () => null,
      unlinked_mentions: () => [],
      page_graph: () => ({ nodes: state.pages.filter((p: any) => !p.inbox).map((p: any) => ({ id: p.id, title: titleOf(p), degree: 0 })), edges: [] }),
      search_pages: ({ query }: any) => state.pages.filter((p: any) => titleOf(p).toLowerCase().includes((query || "").toLowerCase())).map(lite),
      daily_note: ({ date }: any) => {
        let p = state.pages.find((x: any) => x.dailyDate === date);
        if (!p) {
          p = { id: state.nextId++, title: date, dailyDate: date, content: "", sortOrder: 0, archived: false, inbox: false, indexed: false, createdAt: "", updatedAt: "" };
          state.pages.push(p);
        }
        return p;
      },
      // ---- inbox ----
      list_inbox: () => state.inbox.slice().reverse(),
      capture_note: ({ text }: any) => {
        state.inbox.push({ id: state.nextId++, content: text, inbox: true, title: "", sortOrder: 0, archived: false, indexed: false, createdAt: "", updatedAt: "" });
        return null;
      },
      keep_inbox_note: ({ id }: any) => {
        const i = state.inbox.findIndex((x: any) => x.id === id);
        if (i >= 0) {
          const p = state.inbox.splice(i, 1)[0];
          p.inbox = false;
          state.pages.push(p);
        }
        return null;
      },
      // ---- AI ----
      hermes_recall: () => ({ mode: "keyword", notes: [] }),
      hermes_add_note: () => null,
      vault_ask: () => ({ answer: "(mock answer)", citations: [] }),
      extract_memories: () => [],
      plan_tasks: () => ({ createdTaskIds: [], createdEventIds: [], projectNames: [], createdEventTitles: [], updatedEventTitles: [], removedEventTitles: [], createdHabitNames: [], clarifications: [] }),
      daily_briefing: () => ({
        date: "2026-06-28",
        weekday: "Sunday",
        events: [
          { id: 1, title: "Morning standup", start: "2026-06-28T09:00:00", end: "2026-06-28T09:15:00", kind: "fixed", source: "manual", createdAt: "" },
          { id: 2, title: "Lunch with Sam", start: "2026-06-28T12:30:00", end: "2026-06-28T13:30:00", kind: "fixed", source: "manual", createdAt: "" },
          { id: 3, title: "Design review", start: "2026-06-28T15:00:00", end: "2026-06-28T16:00:00", kind: "fixed", source: "manual", createdAt: "" },
        ],
        staleTasks: [],
        dueTasks: [
          { id: 10, title: "Finish the Q3 deck" },
          { id: 11, title: "Email the vendor" },
        ],
        focusMinutes: 90,
      }),
    };

    (window as any).__TAURI_INTERNALS__ = {
      // Window/webview identity so @tauri-apps/api `getCurrentWindow()`/`getCurrentWebview()` (used by
      // the frameless TitleBar) resolve instead of throwing during the initial render.
      metadata: {
        currentWindow: { label: "main" },
        currentWebview: { label: "main", windowLabel: "main" },
      },
      transformCallback: (cb: any) => cb,
      invoke: (cmd: string, args: any) => {
        if (handlers[cmd]) return Promise.resolve(handlers[cmd](args || {}));
        // Tauri plugin calls (window controls, events): safe defaults so nothing throws on boot.
        if (cmd.startsWith("plugin:")) {
          if (cmd.includes("|version")) return Promise.resolve("0.0.0-e2e"); // getVersion() → matches seeded lastSeenVersion
          if (cmd.includes("is_") || cmd.includes("fullscreen") || cmd.includes("maximize")) return Promise.resolve(false);
          return Promise.resolve(0);
        }
        return Promise.resolve(null);
      },
    };
  });
}
