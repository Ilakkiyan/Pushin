import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// A small stateful in-memory backend so the *whole app* boots and drives flows in jsdom (no Tauri,
// no browser). Mirrors the Playwright bridge but at the typed `api` layer (positional args).
vi.mock("./lib/ipc", () => {
  const state = { pages: [] as any[], inbox: [] as any[], nextId: 1 };
  const settings = {
    onboarded: true,
    googleConnected: false,
    timezone: "UTC",
    workStart: "09:00",
    workEnd: "17:00",
    sleepStart: "23:00",
    sleepEnd: "07:00",
    commitments: [],
    horizonDays: 14,
    bufferMinutes: 0,
    minBlockMinutes: 30,
    maxBlockMinutes: 120,
    modelId: "lite",
    embedModel: "",
    llmBaseUrl: "",
  };
  const lite = (p: any) => ({ ...p, content: "", contentJson: undefined });
  return {
    api: {
      loadAll: vi.fn().mockResolvedValue({ settings, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
      llmStatus: vi.fn().mockResolvedValue({ reachable: true, baseUrl: "", modelPresent: true, modelId: "lite", models: [] }),
      ensureInference: vi.fn().mockResolvedValue("ready"),
      ensureEmbeddings: vi.fn().mockResolvedValue("ready"),
      reschedule: vi.fn().mockResolvedValue({ conflicts: [] }),
      listLabels: vi.fn().mockResolvedValue([]),
      labelsFor: vi.fn().mockResolvedValue([]),
      listPages: vi.fn(async () => state.pages.filter((p) => !p.inbox).map(lite)),
      listInbox: vi.fn(async () => state.inbox.slice().reverse()),
      captureNote: vi.fn(async (text: string) => {
        state.inbox.push({ id: state.nextId++, content: text, inbox: true, title: "", archived: false, indexed: false, sortOrder: 0, createdAt: "", updatedAt: "" });
      }),
      createPage: vi.fn(async (title: string) => {
        const p = { id: state.nextId++, title, content: "", archived: false, inbox: false, indexed: false, sortOrder: 0, createdAt: "", updatedAt: "" };
        state.pages.push(p);
        return p;
      }),
      getPage: vi.fn(async (id: number) => state.pages.find((p) => p.id === id)),
      hermesRecall: vi.fn().mockResolvedValue({ mode: "keyword", notes: [] }),
      dailyBriefing: vi.fn().mockResolvedValue({ date: "2026-06-15", weekday: "Monday", events: [], dueTasks: [], focusMinutes: 0 }),
      suggestLabels: vi.fn().mockResolvedValue([]),
      activeFocus: vi.fn().mockResolvedValue(null),
      searchPages: vi.fn().mockResolvedValue([]),
    },
  };
});

import App from "./App";
import { api } from "./lib/ipc";
import { useStore } from "./state/store";

beforeEach(() => {
  vi.clearAllMocks();
  // Reset transient store bits between renders of the singleton store.
  useStore.setState({ view: "calendar", captureOpen: false, currentPageId: null, chatMessages: [] });
});

describe("App (mocked-IPC integration)", () => {
  it("boots past the loading screen to the calendar shell", async () => {
    render(<App />);
    // Sidebar + calendar toolbar appear once load_all resolves.
    expect(await screen.findByText("Today")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Plan" })).toBeInTheDocument(); // AI chat pane (Plan/Chat toggle)
    expect(api.loadAll).toHaveBeenCalled();
  });

  it("quick-capture flows into the Inbox", async () => {
    render(<App />);
    await screen.findByText("Today");

    fireEvent.keyDown(window, { key: "n", ctrlKey: true, shiftKey: true });
    // Target the QuickCapture modal's box specifically (ChatPane also has a textbox).
    const box = await screen.findByPlaceholderText(/Capture a thought/);
    await userEvent.type(box, "buy oat milk");
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(api.captureNote).toHaveBeenCalledWith("buy oat milk"));

    // Open the Inbox view and see the capture.
    await userEvent.click(screen.getByText("Inbox", { exact: true }));
    expect(await screen.findByText("buy oat milk")).toBeInTheDocument();
  });
});
