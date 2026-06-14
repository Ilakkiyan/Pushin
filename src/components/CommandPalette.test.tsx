import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    hermesRecall: vi.fn().mockResolvedValue({ mode: "semantic", notes: [{ id: 1 }] }),
    searchPages: vi.fn().mockResolvedValue([]),
    vaultAsk: vi.fn().mockResolvedValue({ answer: "The budget is $42.", citations: [1] }),
  },
}));

import CommandPalette from "./CommandPalette";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";

const pages = [
  { id: 1, title: "Budget" },
  { id: 2, title: "Roadmap" },
] as never;

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ pages, currentPageId: null, view: "calendar" });
});

const openPalette = () => fireEvent.keyDown(window, { key: "k", metaKey: true });

describe("CommandPalette", () => {
  it("opens on Cmd+K and semantic-searches pages", async () => {
    render(<CommandPalette />);
    expect(screen.queryByPlaceholderText(/Search pages/)).not.toBeInTheDocument();
    openPalette();
    const input = screen.getByPlaceholderText(/Search pages/);
    await userEvent.type(input, "bud");
    // Debounced recall → maps note id 1 → the "Budget" page, with a "semantic" mode pill.
    await waitFor(() => expect(api.hermesRecall).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByText("Budget")).toBeInTheDocument());
    expect(screen.getByText("semantic")).toBeInTheDocument();
  });

  it("offers + runs ask-your-vault, showing the answer", async () => {
    render(<CommandPalette />);
    openPalette();
    await userEvent.type(screen.getByPlaceholderText(/Search pages/), "what is the budget");
    const ask = await screen.findByText(/Ask your vault:/);
    await userEvent.click(ask);
    await waitFor(() => expect(api.vaultAsk).toHaveBeenCalledWith("what is the budget"));
    expect(await screen.findByText("The budget is $42.")).toBeInTheDocument();
  });

  it("Enter opens the highlighted page and closes", async () => {
    render(<CommandPalette />);
    openPalette();
    const input = screen.getByPlaceholderText(/Search pages/);
    await userEvent.type(input, "bud");
    await waitFor(() => expect(screen.getByText("Budget")).toBeInTheDocument());
    fireEvent.keyDown(input, { key: "Enter" });
    expect(useStore.getState().currentPageId).toBe(1);
    expect(useStore.getState().view).toBe("vault");
  });
});
