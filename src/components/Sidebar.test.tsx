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
    view: "today",
    space: "planner",
    prevPlannerView: "today",
    sidebarCollapsed: false,
    pages: [],
    inbox: [{ id: 1 }, { id: 2 }] as never,
    llm: { reachable: true } as never,
    busy: false,
  });
});

describe("Sidebar", () => {
  it("renders the planner destinations + Vault entry + AI status", () => {
    render(<Sidebar />);
    for (const label of ["Today", "Calendar", "Projects", "Habits", "Booking", "People", "Vault", "Settings"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
    expect(screen.getByText(/AI ready/)).toBeInTheDocument();
  });

  it("entering the Vault space reveals the second-brain destinations", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Vault"));
    for (const label of ["Back to app", "Files", "Notes", "Today's note", "Inbox", "Graph"]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  it("shows the Inbox count badge on the Vault entry", () => {
    render(<Sidebar />);
    expect(screen.getByText("2")).toBeInTheDocument();
  });

  it("clicking a nav item switches the view (across spaces)", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByText("Projects"));
    expect(useStore.getState().view).toBe("projects");
    // The Vault button opens the vault at its FILE BROWSER, rooted — not at whichever document
    // happened to be open last. That's the whole point of it being a "place" you step into.
    await userEvent.click(screen.getByText("Vault"));
    expect(useStore.getState().view).toBe("files");
    expect(useStore.getState().space).toBe("vault");
    expect(useStore.getState().vaultFolderId).toBe(null);
    await userEvent.click(screen.getByText("Graph"));
    expect(useStore.getState().view).toBe("graph");
    // Back returns to the planner, at the view we left.
    await userEvent.click(screen.getByText("Back to app"));
    expect(useStore.getState().space).toBe("planner");
    expect(useStore.getState().view).toBe("projects");
  });

  it("collapse toggle flips sidebarCollapsed", async () => {
    render(<Sidebar />);
    await userEvent.click(screen.getByTitle("Collapse sidebar"));
    expect(useStore.getState().sidebarCollapsed).toBe(true);
  });
});
