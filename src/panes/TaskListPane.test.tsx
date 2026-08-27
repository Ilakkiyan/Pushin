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
    explainSchedule: vi.fn().mockResolvedValue([]),
    entityPages: vi.fn().mockResolvedValue([]),
    createPage: vi.fn().mockResolvedValue({ id: 7, title: "Write slides" }),
    linkPageEntity: vi.fn().mockResolvedValue(undefined),
    listPages: vi.fn().mockResolvedValue([]),
    labelsFor: vi.fn().mockResolvedValue([]),
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

  it("shows a rollover chip only once a task has actually been missed", async () => {
    // The visible half of the day-rollover sweep: a task whose planned time came and went carries a
    // "pushed forward N times" mark, so a task you keep sliding is obvious rather than silently
    // reappearing at a new time each morning.
    const { unmount } = render(<TaskListPane />);
    expect(screen.queryByText("2×")).not.toBeInTheDocument();
    unmount();

    useStore.setState({ tasks: [{ ...task, missedCount: 2 }] as never });
    render(<TaskListPane />);
    await waitFor(() => expect(screen.getByText("2×")).toBeInTheDocument());
  });

  it("hides the rollover chip on a finished task", async () => {
    // Done is done — the nag must not outlive the work.
    useStore.setState({ tasks: [{ ...task, status: "done", missedCount: 3 }] as never });
    render(<TaskListPane />);
    expect(screen.queryByText("3×")).not.toBeInTheDocument();
  });
});

describe("TaskListPane — the done bin", () => {
  const finished = { ...task, id: 2, title: "Finished thing", status: "done" };

  beforeEach(() => {
    localStorage.clear();
  });

  it("is not shown at all until something is finished", () => {
    useStore.setState({ tasks: [task] as never });
    render(<TaskListPane />);
    expect(screen.queryByRole("button", { name: /^Done/ })).not.toBeInTheDocument();
  });

  it("starts collapsed, so finished work never buries the active list", async () => {
    // The bin only ever grows. Open by default it becomes a wall you scroll past to reach nothing.
    useStore.setState({ tasks: [task, finished] as never });
    render(<TaskListPane />);

    expect(screen.getByText("Write slides")).toBeInTheDocument();
    expect(screen.queryByText("Finished thing")).not.toBeInTheDocument();
  });

  it("says how much is inside without being opened", () => {
    useStore.setState({ tasks: [task, finished, { ...finished, id: 3, title: "Another" }] as never });
    render(<TaskListPane />);
    expect(screen.getByText("· 2")).toBeInTheDocument();
  });

  it("opens and closes on click", async () => {
    useStore.setState({ tasks: [task, finished] as never });
    render(<TaskListPane />);
    const toggle = screen.getByRole("button", { name: /^Done/ });

    await userEvent.click(toggle);
    expect(await screen.findByText("Finished thing")).toBeInTheDocument();

    await userEvent.click(toggle);
    await waitFor(() => expect(screen.queryByText("Finished thing")).not.toBeInTheDocument());
  });

  it("reports its state to assistive tech", async () => {
    useStore.setState({ tasks: [task, finished] as never });
    render(<TaskListPane />);
    const toggle = screen.getByRole("button", { name: /^Done/ });

    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-expanded", "true"));
  });

  it("remembers that it was left open", async () => {
    useStore.setState({ tasks: [task, finished] as never });
    const { unmount } = render(<TaskListPane />);
    await userEvent.click(screen.getByRole("button", { name: /^Done/ }));
    await screen.findByText("Finished thing");
    unmount();

    render(<TaskListPane />);
    expect(await screen.findByText("Finished thing")).toBeInTheDocument();
  });

  it("remembers that it was left closed", async () => {
    useStore.setState({ tasks: [task, finished] as never });
    const { unmount } = render(<TaskListPane />);
    const toggle = screen.getByRole("button", { name: /^Done/ });
    await userEvent.click(toggle); // open
    await userEvent.click(toggle); // and closed again
    unmount();

    render(<TaskListPane />);
    expect(screen.queryByText("Finished thing")).not.toBeInTheDocument();
  });

  it("still works when localStorage is unavailable", async () => {
    // A private window, cleared site data, or a browser set to block storage throws on access. The
    // bin forgetting between sessions is fine; the task list crashing is not.
    const getItem = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });

    useStore.setState({ tasks: [task, finished] as never });
    expect(() => render(<TaskListPane />)).not.toThrow();
    await userEvent.click(screen.getByRole("button", { name: /^Done/ }));
    expect(await screen.findByText("Finished thing")).toBeInTheDocument();

    getItem.mockRestore();
    setItem.mockRestore();
  });

  it("keeps the header count on active tasks only", () => {
    // The pane header counts what is left to do; finishing something must decrease it, not hide it
    // behind the bin's own count.
    useStore.setState({ tasks: [task, finished] as never });
    render(<TaskListPane />);
    // Scoped to the pane header — the bin has a count of its own.
    expect(screen.getByText(/^Tasks/).textContent).toContain("· 1");
  });
});
