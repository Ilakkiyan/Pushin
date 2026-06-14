import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import TitleBar from "./TitleBar";

// The mocked Tauri window stub installed in vitest.setup.ts.
const win = (globalThis as Record<string, unknown>).__tauriWindow as {
  minimize: ReturnType<typeof vi.fn>;
  toggleMaximize: ReturnType<typeof vi.fn>;
  close: ReturnType<typeof vi.fn>;
  isMaximized: ReturnType<typeof vi.fn>;
  isFullscreen: ReturnType<typeof vi.fn>;
};

beforeEach(() => {
  vi.clearAllMocks();
  win.isMaximized.mockResolvedValue(false);
  win.isFullscreen.mockResolvedValue(false);
});

describe("TitleBar (frameless window controls)", () => {
  it("renders the brand + controls and wires the window buttons", async () => {
    render(<TitleBar />);
    await waitFor(() => expect(win.isMaximized).toHaveBeenCalled());
    expect(screen.getByText("Pushin")).toBeInTheDocument();

    await userEvent.click(screen.getByTitle("Minimize"));
    expect(win.minimize).toHaveBeenCalledOnce();

    await userEvent.click(screen.getByTitle("Maximize"));
    expect(win.toggleMaximize).toHaveBeenCalledOnce();

    await userEvent.click(screen.getByTitle("Close"));
    expect(win.close).toHaveBeenCalledOnce();
  });

  it("queries fullscreen + maximized state on mount (drives auto-hide)", async () => {
    render(<TitleBar />);
    await waitFor(() => {
      expect(win.isMaximized).toHaveBeenCalled();
      expect(win.isFullscreen).toHaveBeenCalled();
    });
  });
});
