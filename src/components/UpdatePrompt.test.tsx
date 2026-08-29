import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Drive the prompt without the real Tauri updater plugin.
const checkForUpdate = vi.fn();
const downloadUpdate = vi.fn();
const installDownloaded = vi.fn();
const installUpdate = vi.fn();
vi.mock("../lib/updates", () => ({
  checkForUpdate: (...a: unknown[]) => checkForUpdate(...a),
  downloadUpdate: (...a: unknown[]) => downloadUpdate(...a),
  installDownloaded: (...a: unknown[]) => installDownloaded(...a),
  installUpdate: (...a: unknown[]) => installUpdate(...a),
}));

import UpdatePrompt, { CHECK_EVERY_MS, setAutoUpdateEnabled } from "./UpdatePrompt";

const UPDATE = { version: "9.9.9", body: "Shiny new things" };

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  downloadUpdate.mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

describe("UpdatePrompt", () => {
  it("renders nothing when up to date", async () => {
    checkForUpdate.mockResolvedValue(null);
    const { container } = render(<UpdatePrompt />);
    await waitFor(() => expect(checkForUpdate).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
    expect(downloadUpdate).not.toHaveBeenCalled();
  });

  it("downloads in the background and only then asks", async () => {
    // The whole point of the change: nobody presses "check for updates", and nobody waits for a
    // transfer they agreed to. The question arrives with the bytes already down.
    let release: () => void = () => {};
    downloadUpdate.mockReturnValue(new Promise<void>((r) => (release = r)));
    checkForUpdate.mockResolvedValue(UPDATE);

    render(<UpdatePrompt />);
    await waitFor(() => expect(downloadUpdate).toHaveBeenCalledWith(UPDATE, expect.any(Function)));
    // Silent while it downloads.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    release();
    expect(await screen.findByText(/9\.9\.9 is ready to install/)).toBeInTheDocument();
  });

  it("installs the already-downloaded package rather than fetching it again", async () => {
    checkForUpdate.mockResolvedValue(UPDATE);
    render(<UpdatePrompt />);
    await screen.findByText(/ready to install/);

    await userEvent.click(screen.getByRole("button", { name: /Install now/i }));
    await waitFor(() => expect(installDownloaded).toHaveBeenCalledWith(UPDATE));
    expect(installUpdate).not.toHaveBeenCalled();
  });

  it("shows the release notes on request", async () => {
    checkForUpdate.mockResolvedValue(UPDATE);
    render(<UpdatePrompt />);
    await screen.findByText(/ready to install/);
    await userEvent.click(screen.getByRole("button", { name: /What's new/i }));
    expect(screen.getByText("Shiny new things")).toBeInTheDocument();
  });

  it("postpones with Later, and remembers it per version", async () => {
    checkForUpdate.mockResolvedValue(UPDATE);
    const { unmount } = render(<UpdatePrompt />);
    await screen.findByText(/ready to install/);

    await userEvent.click(screen.getByRole("button", { name: /Later/i }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    // A restart inside the postponement window stays quiet rather than asking again immediately.
    unmount();
    render(<UpdatePrompt />);
    await waitFor(() => expect(checkForUpdate).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(downloadUpdate).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("postpones one version, not updating in general", async () => {
    // Postponing 9.9.9 must not silence the release that comes after it.
    checkForUpdate.mockResolvedValue(UPDATE);
    const { unmount } = render(<UpdatePrompt />);
    await screen.findByText(/ready to install/);
    await userEvent.click(screen.getByRole("button", { name: /Later/i }));
    unmount();

    checkForUpdate.mockResolvedValue({ version: "9.9.10" });
    render(<UpdatePrompt />);
    expect(await screen.findByText(/9\.9\.10 is ready to install/)).toBeInTheDocument();
  });

  it("does not interrupt the opening sequence, but downloads underneath it", async () => {
    checkForUpdate.mockResolvedValue(UPDATE);
    const { rerender } = render(<UpdatePrompt hold />);
    await waitFor(() => expect(downloadUpdate).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    rerender(<UpdatePrompt hold={false} />);
    expect(await screen.findByText(/ready to install/)).toBeInTheDocument();
  });

  it("asks before downloading when automatic updates are off", async () => {
    setAutoUpdateEnabled(false);
    checkForUpdate.mockResolvedValue(UPDATE);
    installUpdate.mockResolvedValue(undefined);

    render(<UpdatePrompt />);
    expect(await screen.findByText(/9\.9\.9 is available/)).toBeInTheDocument();
    expect(downloadUpdate).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: /Update now/i }));
    await waitFor(() => expect(installUpdate).toHaveBeenCalledWith(UPDATE, expect.any(Function)));
  });

  it("stays quiet about a failed background download until it keeps failing", async () => {
    // An unattended transfer nobody asked for must not throw an error dialog at the user on a flaky
    // connection. It retries; only a persistent failure turns into a question they can act on.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    checkForUpdate.mockResolvedValue(UPDATE);
    downloadUpdate.mockRejectedValue(new Error("connection reset"));

    render(<UpdatePrompt />);
    await vi.waitFor(() => expect(downloadUpdate).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await vi.advanceTimersByTimeAsync(CHECK_EVERY_MS + 10);
    await vi.waitFor(() => expect(downloadUpdate).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(screen.getByText(/9\.9\.9 is available/)).toBeInTheDocument());
  });

  it("surfaces an install failure instead of dying silently", async () => {
    checkForUpdate.mockResolvedValue(UPDATE);
    installDownloaded.mockRejectedValue(new Error("signature mismatch"));
    render(<UpdatePrompt />);
    await screen.findByText(/ready to install/);

    await userEvent.click(screen.getByRole("button", { name: /Install now/i }));
    expect(await screen.findByText(/signature mismatch/)).toBeInTheDocument();
  });
});
