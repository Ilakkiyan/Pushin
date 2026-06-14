import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import TitleBar, { usesNativeTitleBar } from "./TitleBar";

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

function mockPlatform(platform: string) {
  return vi.spyOn(window.navigator, "platform", "get").mockReturnValue(platform);
}

describe("TitleBar (frameless window controls)", () => {
  it("renders the brand + controls and wires the window buttons", async () => {
    const platform = mockPlatform("Win32");
    render(<TitleBar />);
    await waitFor(() => expect(win.isMaximized).toHaveBeenCalled());
    expect(screen.getByText("Pushin")).toBeInTheDocument();

    await userEvent.click(screen.getByTitle("Minimize"));
    expect(win.minimize).toHaveBeenCalledOnce();

    await userEvent.click(screen.getByTitle("Maximize"));
    expect(win.toggleMaximize).toHaveBeenCalledOnce();

    await userEvent.click(screen.getByTitle("Close"));
    expect(win.close).toHaveBeenCalledOnce();
    platform.mockRestore();
  });

  it("queries fullscreen + maximized state on mount (drives auto-hide)", async () => {
    const platform = mockPlatform("Win32");
    render(<TitleBar />);
    await waitFor(() => {
      expect(win.isMaximized).toHaveBeenCalled();
      expect(win.isFullscreen).toHaveBeenCalled();
    });
    platform.mockRestore();
  });

  it("uses native macOS chrome instead of rendering custom window controls", () => {
    const platform = mockPlatform("MacIntel");
    render(<TitleBar />);
    expect(usesNativeTitleBar()).toBe(true);
    expect(screen.queryByText("Pushin")).not.toBeInTheDocument();
    expect(win.isMaximized).not.toHaveBeenCalled();
    platform.mockRestore();
  });
});
