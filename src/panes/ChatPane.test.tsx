import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    planTasks: vi.fn().mockResolvedValue({ createdTaskIds: [1], projectNames: [], createdEventTitles: [], updatedEventTitles: [], removedEventTitles: [], createdHabitNames: [], clarifications: [], recalledNotes: [] }),
    loadAll: vi.fn().mockResolvedValue({ settings: { googleConnected: false }, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
    reschedule: vi.fn().mockResolvedValue({ conflicts: [] }),
    extractMemories: vi.fn().mockResolvedValue(["Sarah prefers afternoon meetings"]),
    hermesAddNote: vi.fn().mockResolvedValue([]),
    listPages: vi.fn().mockResolvedValue([]),
  },
}));

import ChatPane from "./ChatPane";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ llm: { reachable: true } as never, busy: false, chatMessages: [], settings: { googleConnected: false } as never });
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
});
