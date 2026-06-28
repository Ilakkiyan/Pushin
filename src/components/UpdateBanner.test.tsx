import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Drive the banner without the real Tauri updater plugin.
const checkForUpdate = vi.fn();
const installUpdate = vi.fn();
vi.mock("../lib/updates", () => ({
  checkForUpdate: (...a: unknown[]) => checkForUpdate(...a),
  installUpdate: (...a: unknown[]) => installUpdate(...a),
}));

import UpdateBanner from "./UpdateBanner";

beforeEach(() => vi.clearAllMocks());

describe("UpdateBanner", () => {
  it("renders nothing when up to date", async () => {
    checkForUpdate.mockResolvedValue(null);
    const { container } = render(<UpdateBanner />);
    await waitFor(() => expect(checkForUpdate).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("surfaces an available version and installs on click", async () => {
    const update = { version: "9.9.9", body: "Shiny new things" };
    checkForUpdate.mockResolvedValue(update);
    installUpdate.mockResolvedValue(undefined);

    render(<UpdateBanner />);
    expect(await screen.findByText("9.9.9")).toBeInTheDocument();
    expect(screen.getByText(/tasks, notes, and settings are kept/i)).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Update & restart/i }));
    await waitFor(() => expect(installUpdate).toHaveBeenCalledWith(update, expect.any(Function)));
  });

  it("can be dismissed with Later", async () => {
    checkForUpdate.mockResolvedValue({ version: "9.9.9" });
    render(<UpdateBanner />);
    await screen.findByText("9.9.9");
    await userEvent.click(screen.getByTitle("Later"));
    expect(screen.queryByText("9.9.9")).not.toBeInTheDocument();
  });
});
