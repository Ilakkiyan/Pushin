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
    const state: any = {
      nextId: 1,
      pages: [] as any[],
      inbox: [] as any[],
      settings: {
        // `?new` in the URL → a fresh (un-onboarded) user, for capturing the WelcomeGuide.
        onboarded: !new URLSearchParams(window.location.search).has("new"),
        googleConnected: false,
        timezone: "UTC",
        workStart: "09:00",
        workEnd: "17:00",
        workDays: [1, 2, 3, 4, 5],
        commitments: [],
        sleepEnabled: false,
        sleepStart: "23:00",
        sleepEnd: "07:00",
        modelId: "lite",
        embedModel: "",
      },
    };
    const titleOf = (p: any) => (p.title && p.title.trim()) || (p.content || "").split("\n")[0]?.slice(0, 80) || "Untitled";
    const lite = (p: any) => ({ ...p, content: "", contentJson: undefined, title: titleOf(p) });

    const handlers: Record<string, (a: any) => any> = {
      load_all: () => ({ settings: state.settings, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
      reschedule: () => ({ conflicts: [] }),
      save_settings: () => null,
      llm_status: () => ({ reachable: true, baseUrl: "", modelPresent: true, modelId: "lite", models: [] }),
      list_models: () => [],
      ensure_inference: () => "ready",
      ensure_embeddings: () => "ready",
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
          if (cmd.includes("is_") || cmd.includes("fullscreen") || cmd.includes("maximize")) return Promise.resolve(false);
          return Promise.resolve(0);
        }
        return Promise.resolve(null);
      },
    };
  });
}
