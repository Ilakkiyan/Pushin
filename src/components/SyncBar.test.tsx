import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";

import SyncBar, { describeSync, syncPercent } from "./SyncBar";
import { useStore } from "../state/store";
import type { SyncProgress } from "../lib/ipc";

function progress(over: Partial<SyncProgress> = {}): SyncProgress {
  return { source: "device", phase: "files", label: "", done: 0, total: 0, active: true, ...over };
}

beforeEach(() => {
  useStore.setState({ syncProgress: null });
});

describe("SyncBar", () => {
  it("renders nothing at all when no sync is running", () => {
    // The footer keeps its usual height when idle — the bar is not a permanently reserved slot.
    const { container } = render(<SyncBar collapsed={false} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("disappears again on the closing event rather than sitting at 100%", () => {
    useStore.setState({ syncProgress: progress({ active: false, done: 10, total: 10 }) });
    const { container } = render(<SyncBar collapsed={false} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the phase and a real percentage while files are moving", () => {
    useStore.setState({ syncProgress: progress({ done: 3, total: 4 }) });
    render(<SyncBar collapsed={false} />);
    expect(screen.getByText("Syncing vault files")).toBeInTheDocument();
    expect(screen.getByText("75%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "75");
  });

  it("shows no number while the backend does not know the size of the work", () => {
    // An invented percentage that parks at 90% is worse than none: it teaches you to stop believing
    // the bar. `total: 0` means indeterminate, and it must render motion without a figure.
    useStore.setState({ syncProgress: progress({ source: "google", phase: "pull", done: 0, total: 0 }) });
    render(<SyncBar collapsed={false} />);
    expect(screen.getByText("Pulling from Google")).toBeInTheDocument();
    expect(screen.queryByText(/%/)).not.toBeInTheDocument();
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
  });

  it("collapses to a bare bar with the detail in the tooltip", () => {
    useStore.setState({ syncProgress: progress({ done: 1, total: 2 }) });
    render(<SyncBar collapsed />);
    // No room for words on the rail, so nothing is truncated into nonsense — but the label is still
    // reachable by hover and by a screen reader.
    expect(screen.queryByText("Syncing vault files")).not.toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("title", "Syncing vault files — 50%");
  });
});

describe("describeSync", () => {
  it("names each engine's phase in the user's terms", () => {
    expect(describeSync(progress({ source: "google", phase: "pull" }))).toBe("Pulling from Google");
    expect(describeSync(progress({ source: "google", phase: "push" }))).toBe("Pushing to Google");
    expect(describeSync(progress({ source: "google", phase: "mirror" }))).toBe("Mirroring blocks");
    expect(describeSync(progress({ source: "device", phase: "files" }))).toBe("Syncing vault files");
    expect(describeSync(progress({ source: "device", phase: "rows" }))).toBe("Syncing devices");
    expect(describeSync(progress({ source: "device", phase: "rows", label: "MacBook" }))).toBe(
      "Syncing with MacBook",
    );
  });
});

describe("syncPercent", () => {
  it("is null when indeterminate and clamped otherwise", () => {
    expect(syncPercent(progress({ done: 0, total: 0 }))).toBeNull();
    expect(syncPercent(progress({ done: 1, total: 3 }))).toBe(33);
    // A total that lags behind the count (a want-list that grew mid-phase) must not print 140%.
    expect(syncPercent(progress({ done: 7, total: 5 }))).toBe(100);
    expect(syncPercent(progress({ done: 0, total: 5 }))).toBe(0);
  });
});
