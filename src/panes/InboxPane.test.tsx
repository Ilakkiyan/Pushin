import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    listInbox: vi.fn().mockResolvedValue([{ id: 5, content: "buy milk", inbox: true }]),
    keepInboxNote: vi.fn().mockResolvedValue(undefined),
    deletePage: vi.fn().mockResolvedValue([]),
    listPages: vi.fn().mockResolvedValue([]),
    planTasks: vi.fn().mockResolvedValue({ createdTaskIds: [], createdHabitNames: [], projectNames: [], createdEventTitles: [], updatedEventTitles: [], removedEventTitles: [], clarifications: [] }),
    loadAll: vi.fn().mockResolvedValue({ settings: {}, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
    reschedule: vi.fn().mockResolvedValue({ conflicts: [] }),
  },
}));

import InboxPane from "./InboxPane";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ inbox: [], settings: { googleConnected: false } as never });
});

describe("InboxPane", () => {
  it("lists captures and supports Keep / Delete / Plan triage", async () => {
    render(<InboxPane />);
    expect(await screen.findByText("buy milk")).toBeInTheDocument();

    await userEvent.click(screen.getByText("Keep as note"));
    await waitFor(() => expect(api.keepInboxNote).toHaveBeenCalledWith(5));
  });

  it("Plan with AI sends the capture through the planner then removes it", async () => {
    render(<InboxPane />);
    await screen.findByText("buy milk");
    await userEvent.click(screen.getByText("Plan with AI"));
    await waitFor(() => expect(api.planTasks).toHaveBeenCalledWith("buy milk", []));
    await waitFor(() => expect(api.deletePage).toHaveBeenCalledWith(5));
  });

  it("Delete removes the capture", async () => {
    render(<InboxPane />);
    await screen.findByText("buy milk");
    await userEvent.click(screen.getByTitle("Delete"));
    await waitFor(() => expect(api.deletePage).toHaveBeenCalledWith(5));
  });
});
