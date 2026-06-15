import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({ api: { ensureInference: vi.fn().mockResolvedValue(undefined) } }));
// VaultTree pulls in the BlockNote-backed importer; stub it so the Sidebar test stays light.
vi.mock("../lib/import", () => ({ importMarkdownFolder: vi.fn() }));

import Sidebar from "./Sidebar";
import { useStore } from "../state/store";

beforeEach(() => {
  useStore.setState({
    view: "calendar",
    sidebarCollapsed: false,
    pages: [],
    inbox: [{ id: 1 }, { id: 2 }] as never,
    llm: { reachable: true } as never,
    busy: false,
  });
});

describe("Sidebar", () => {
  it("renders all nav destinations + the AI status", () => {
    render(<Sidebar />);
    for (const label of ["Calendar", "Projects", "Habits", "Booking", "Notes", "Inbox", "Graph", "Settings", "Today's note"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
    expect(screen.getByText(/AI ready/)).toBeInTheDocument();
  });

  it("shows the Inbox count badge", () => {
    render(<Sidebar />);
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("clicking a nav item switches the view", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Projects"));
    expect(useStore.getState().view).toBe("projects");
    await userEvent.click(screen.getByText("Graph"));
    expect(useStore.getState().view).toBe("graph");
  });

  it("collapse toggle flips sidebarCollapsed", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByTitle("Collapse sidebar"));
    expect(useStore.getState().sidebarCollapsed).toBe(true);
  });
});
