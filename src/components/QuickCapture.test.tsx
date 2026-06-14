import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    captureNote: vi.fn().mockResolvedValue(undefined),
    listInbox: vi.fn().mockResolvedValue([]),
    listPages: vi.fn().mockResolvedValue([]),
  },
}));

import QuickCapture from "./QuickCapture";
import { useStore } from "../state/store";
import { api } from "../lib/ipc";

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ captureOpen: false });
});

describe("QuickCapture", () => {
  it("is hidden until the Ctrl+Shift+N hotkey opens it", () => {
    render(<QuickCapture />);
    expect(screen.queryByText("Quick capture")).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: "n", ctrlKey: true, shiftKey: true });
    expect(screen.getByText("Quick capture")).toBeInTheDocument();
  });

  it("captures text on Ctrl+Enter and closes", async () => {
    render(<QuickCapture />);
    fireEvent.keyDown(window, { key: "n", ctrlKey: true, shiftKey: true });
    const box = screen.getByRole("textbox");
    await userEvent.type(box, "a fleeting idea");
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(api.captureNote).toHaveBeenCalledWith("a fleeting idea"));
    await waitFor(() => expect(screen.queryByText("Quick capture")).not.toBeInTheDocument());
  });

  it("Esc closes without capturing", async () => {
    render(<QuickCapture />);
    fireEvent.keyDown(window, { key: "n", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByText("Quick capture")).not.toBeInTheDocument());
    expect(api.captureNote).not.toHaveBeenCalled();
  });
});
