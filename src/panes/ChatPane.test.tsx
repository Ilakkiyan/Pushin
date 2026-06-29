import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    planTasks: vi.fn().mockResolvedValue({ createdTaskIds: [1], createdEventIds: [], projectNames: [], createdEventTitles: [], updatedEventTitles: [], removedEventTitles: [], createdHabitNames: [], clarifications: [], recalledNotes: [] }),
    loadAll: vi.fn().mockResolvedValue({ settings: { googleConnected: false }, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
    reschedule: vi.fn().mockResolvedValue({ conflicts: [] }),
    extractMemories: vi.fn().mockResolvedValue(["Sarah prefers afternoon meetings"]),
    hermesAddNote: vi.fn().mockResolvedValue([]),
    listPages: vi.fn().mockResolvedValue([]),
    quickLabel: vi.fn().mockResolvedValue([{ id: 7, name: "Health", color: "#10b981" }]),
    labelsFor: vi.fn().mockResolvedValue([]),
    setEntityLabels: vi.fn().mockResolvedValue(undefined),
    listLabels: vi.fn().mockResolvedValue([]),
    routeIntent: vi.fn().mockResolvedValue("plan"),
  },
}));

import ChatPane from "./ChatPane";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ llm: { reachable: true } as never, busy: false, chatMessages: [], chatMode: "plan", settings: { googleConnected: false } as never });
});

describe("ChatPane", () => {
  it("plans a message and summarizes the result", async () => {
    render(<ChatPane />);
    await userEvent.type(screen.getByPlaceholderText(/Describe your projects/), "schedule a call{Enter}");
    await waitFor(() => expect(api.planTasks).toHaveBeenCalled());
    expect(await screen.findByText(/Added 1 task/)).toBeInTheDocument();
  });

  it("offers durable facts and saves them on confirm", async () => {
    render(<ChatPane />);
    await userEvent.type(screen.getByPlaceholderText(/Describe your projects/), "Sarah likes afternoons{Enter}");
    const save = await screen.findByRole("button", { name: "Save" });
    expect(screen.getByText("Sarah prefers afternoon meetings")).toBeInTheDocument();
    await userEvent.click(save);
    await waitFor(() => expect(api.hermesAddNote).toHaveBeenCalledWith("Sarah prefers afternoon meetings"));
  });

  it("offers deterministic auto-labels and applies them on confirm", async () => {
    vi.mocked(api.extractMemories).mockResolvedValueOnce([]);
    vi.mocked(api.planTasks).mockResolvedValueOnce({
      createdTaskIds: [1],
      createdEventIds: [],
      projectNames: [],
      createdEventTitles: [],
      updatedEventTitles: [],
      removedEventTitles: [],
      createdHabitNames: [],
      clarifications: [],
      recalledNotes: [],
    });
    vi.mocked(api.loadAll).mockResolvedValue({
      settings: { googleConnected: false } as never,
      projects: [],
      tasks: [{ id: 1, title: "Gym workout", notes: "" }],
      events: [],
      blocks: [],
      eventTypes: [],
      bookings: [],
    } as never);

    render(<ChatPane />);
    await userEvent.type(screen.getByPlaceholderText(/Describe your projects/), "go to the gym{Enter}");
    expect(await screen.findByText("Apply labels?")).toBeInTheDocument();
    expect(screen.getByText("Gym workout")).toBeInTheDocument();
    expect(screen.getByText("Health")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(api.quickLabel).toHaveBeenCalledWith("Health", "#10b981"));
    expect(api.setEntityLabels).toHaveBeenCalledWith("task", 1, [7]);
  });
});
