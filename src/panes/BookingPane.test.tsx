import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../lib/ipc", () => ({
  api: {
    bookingServerStatus: vi.fn().mockResolvedValue({ running: false, localUrl: null, host: "127.0.0.1", port: null }),
    startBookingServer: vi.fn().mockResolvedValue({ running: true, localUrl: "http://127.0.0.1:47610", host: "127.0.0.1", port: 47610 }),
    stopBookingServer: vi.fn().mockResolvedValue({ running: false, localUrl: null, host: "127.0.0.1", port: null }),
    bookingSlots: vi.fn().mockResolvedValue([{ start: "2026-06-15T09:00:00", end: "2026-06-15T09:30:00" }]),
    updateEventType: vi.fn().mockResolvedValue({}),
    createEventType: vi.fn().mockResolvedValue(2),
    deleteEventType: vi.fn().mockResolvedValue(undefined),
    regenerateEventTypeToken: vi.fn().mockResolvedValue({}),
  },
}));

import BookingPane from "./BookingPane";
import { api } from "../lib/ipc";
import { useStore } from "../state/store";

const settings = {
  onboarded: true,
  googleConnected: false,
  timezone: "UTC",
  workStart: "09:00",
  workEnd: "17:00",
  sleepStart: "23:00",
  sleepEnd: "07:00",
  sleepEnabled: false,
  commitments: [],
  horizonDays: 14,
  bufferMinutes: 0,
  minBlockMinutes: 30,
  maxBlockMinutes: 120,
  modelId: "lite",
  embedModel: "",
  llmBaseUrl: "",
};

const eventType = {
  id: 1,
  name: "Intro call",
  durationMinutes: 30,
  bufferMinutes: 10,
  color: "#0ea5e9",
  slug: "intro-call-1",
  shareToken: "abc123",
  enabled: true,
};

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({
    settings: settings as never,
    eventTypes: [eventType],
    bookings: [],
    load: vi.fn(),
    createBooking: vi.fn(),
    cancelBooking: vi.fn(),
  });
});

describe("BookingPane", () => {
  it("renders event type controls and starts the local server", async () => {
    render(<BookingPane />);
    expect(await screen.findByDisplayValue("Intro call")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /start/i }));
    await waitFor(() => expect(api.startBookingServer).toHaveBeenCalled());
    expect(screen.getAllByText(/http:\/\/127\.0\.0\.1:47610\/b\/abc123\/intro-call-1/).length).toBeGreaterThan(0);
  });

  it("saves event type edits through IPC", async () => {
    render(<BookingPane />);
    const name = await screen.findByDisplayValue("Intro call");
    await userEvent.clear(name);
    await userEvent.type(name, "Consult");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() =>
      expect(api.updateEventType).toHaveBeenCalledWith(1, "Consult", 30, 10, "#0ea5e9", true),
    );
  });
});
