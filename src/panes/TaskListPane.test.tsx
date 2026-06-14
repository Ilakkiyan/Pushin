import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const sched = { conflicts: [] };
const appData = { settings: { googleConnected: false }, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] };

vi.mock("../lib/ipc", () => ({
  api: {
    setTaskStatus: vi.fn().mockResolvedValue({ conflicts: [] }),
    deleteTask: vi.fn().mockResolvedValue({ conflicts: [] }),
    createTask: vi.fn().mockResolvedValue({ conflicts: [] }),
    loadAll: vi.fn().mockResolvedValue({ settings: { googleConnected: false }, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] }),
    entityPages: vi.fn().mockResolvedValue([]),
    createPage: vi.fn().mockResolvedValue({ id: 7, title: "Write slides" }),
    linkPageEntity: vi.fn().mockResolvedValue(undefined),
    listPages: vi.fn().mockResolvedValue([]),
  },
}));

import TaskListPane from "./TaskListPane";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

const task = { id: 1, title: "Write slides", status: "todo", estimatedMinutes: 60, priority: 2, projectId: null, deadline: null, dependsOn: [] };

beforeEach(() => {
  vi.clearAllMocks();
  void sched;
  void appData;
  useStore.setState({ tasks: [task] as never, projects: [], settings: { googleConnected: false } as never });
});

describe("TaskListPane", () => {
  it("renders tasks and toggles status", async () => {
    render(<TaskListPane />);
    expect(screen.getByText("Write slides")).toBeInTheDocument();
    await userEvent.click(screen.getByLabelText("Mark done"));
    await waitFor(() => expect(api.setTaskStatus).toHaveBeenCalledWith(1, "done"));
  });

  it("Notes action opens/links a page for the task", async () => {
    render(<TaskListPane />);
    await userEvent.click(screen.getByTitle("Open notes for this task"));
    await waitFor(() => expect(api.entityPages).toHaveBeenCalledWith("task", 1));
    await waitFor(() => expect(api.linkPageEntity).toHaveBeenCalledWith(7, "task", 1));
  });

  it("deletes a task", async () => {
    render(<TaskListPane />);
    await userEvent.click(screen.getByLabelText("Delete task"));
    await waitFor(() => expect(api.deleteTask).toHaveBeenCalledWith(1));
  });
});
