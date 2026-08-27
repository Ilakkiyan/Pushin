import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";

// A pane must survive whatever the backend hands it.
//
// Every pane here has no test of its own, and the failure they share is not a subtle one: a pane
// that throws while rendering takes its whole subtree with it. `PaneErrorBoundary` contains that
// today, but a contained crash is still a blank screen where the user's day should be — and this is
// a real, observed failure mode in this app (PeoplePane once unmounted on a null payload).
//
// Rather than hand-mocking each pane's IPC surface, the whole `lib/ipc` module is replaced by a
// proxy that answers ANY method with a plausible empty value. That is deliberately harsher than
// production: it exercises the empty-state path of every pane at once, which is what a new user
// actually sees on first run, and it cannot drift as panes gain new calls.

/** A benign default for a command, chosen from its name. Collections read as empty, everything
 *  else as `null` — the two shapes a fresh install genuinely produces. */
function defaultFor(name: string): unknown {
  if (/^(list|search|suggest|backlinks|recent|all)/.test(name)) return [];
  if (name === "loadAll") {
    return { settings: {}, projects: [], tasks: [], events: [], blocks: [], eventTypes: [], bookings: [] };
  }
  if (name === "reschedule") return { conflicts: [] };
  if (name === "pageGraph") return { nodes: [], edges: [] };
  if (name === "briefing") return null;
  if (/^(get|create|update|upsert)/.test(name)) return null;
  return null;
}

vi.mock("../lib/ipc", () => ({
  api: new Proxy(
    {},
    {
      get: (_target, prop) => {
        const name = String(prop);
        if (name === "then") return undefined; // never let the proxy look thenable
        return vi.fn(async () => defaultFor(name));
      },
    },
  ),
}));

// The graph pane pulls in a canvas-backed force renderer that jsdom cannot run.
vi.mock("react-force-graph-2d", () => ({ default: () => null }));

import { useStore } from "../state/store";
import TodayPane from "./TodayPane";
import MonthPane from "./MonthPane";
import HabitsPane from "./HabitsPane";
import ProjectsPane from "./ProjectsPane";
import LabelPane from "./LabelPane";
import PeoplePane from "./PeoplePane";
import VaultPane from "./VaultPane";
import GraphPane from "./GraphPane";
import SettingsPane from "./SettingsPane";

const settings = {
  timezone: "UTC",
  workStart: "09:00",
  workEnd: "17:00",
  workDays: [1, 2, 3, 4, 5],
  horizonDays: 14,
  bufferMinutes: 5,
  defaultMinChunk: 30,
  defaultMaxChunk: 120,
  modelId: "m",
  llmBaseUrl: "http://127.0.0.1:8080",
  googleConnected: false,
  googleClientId: "",
  googleClientSecret: "",
  onboarded: true,
  sleepEnabled: false,
  sleepStart: "23:00",
  sleepEnd: "07:00",
  commitments: [],
  embedModel: "bge",
  vaultDir: null,
  archetypes: [],
  aboutMe: "",
  idleUnloadMinutes: 10,
};

/** The store as a brand-new install sees it: everything empty, nothing loaded yet. */
function emptyStore() {
  useStore.setState({
    settings: settings as never,
    projects: [],
    tasks: [],
    events: [],
    blocks: [],
    conflicts: [],
    blockReasons: {},
    pages: [],
    inbox: [],
    labels: [],
    people: [],
    habits: [],
    eventTypes: [],
    bookings: [],
    calMode: "week",
    calColorByLabel: false,
    calLabelFilterIds: [],
    focusDateIso: null,
    view: "today",
    space: "planner",
  } as never);
}

const PANES: Array<[string, () => React.ReactElement]> = [
  ["TodayPane", () => <TodayPane />],
  ["MonthPane", () => <MonthPane />],
  ["HabitsPane", () => <HabitsPane />],
  ["ProjectsPane", () => <ProjectsPane />],
  ["LabelPane", () => <LabelPane />],
  ["PeoplePane", () => <PeoplePane />],
  ["VaultPane", () => <VaultPane />],
  ["GraphPane", () => <GraphPane />],
  ["SettingsPane", () => <SettingsPane />],
];

beforeEach(() => {
  vi.clearAllMocks();
  emptyStore();
});

describe("every pane renders on a brand-new, empty install", () => {
  it.each(PANES)("%s mounts, settles, and puts something on screen", async (name, mount) => {
    const errors: unknown[] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...args) => errors.push(args[0]));

    let container: HTMLElement;
    await act(async () => {
      ({ container } = render(mount()));
    });

    // It rendered something — not an empty shell where the pane should be.
    await waitFor(() => expect(container!.textContent?.trim().length ?? 0).toBeGreaterThan(0), { timeout: 3000 });

    // ...and nothing threw during render or in an effect. React reports both through console.error,
    // which is the only signal a swallowed async render failure leaves behind.
    const fatal = errors.filter((e) => String(e).match(/not a function|undefined is not|Cannot read|of null|of undefined/i));
    expect(fatal, `${name} threw: ${fatal.join(" | ")}`).toEqual([]);
    spy.mockRestore();
  });
});

describe("every pane survives a backend command that fails", () => {
  // The reachable harsh case. Store collections are never null — serde cannot produce null for a
  // `Vec`, and App gates every pane behind `loaded` — but an *IPC call rejecting* happens for real:
  // a poisoned lock, a mid-write DB, a command that returns Err. A pane that chains `.then(setState)`
  // with no `.catch` leaves an unhandled rejection and a permanently blank panel.
  it.each(PANES)("%s handles a rejected command without an unhandled rejection", async (name, mount) => {
    const rejections: unknown[] = [];
    const onUnhandled = (e: PromiseRejectionEvent) => {
      e.preventDefault();
      rejections.push(e.reason);
    };
    window.addEventListener("unhandledrejection", onUnhandled);
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    const failing = new Proxy(
      {},
      { get: () => vi.fn(async () => Promise.reject(new Error("backend unavailable"))) },
    );
    const ipc = await import("../lib/ipc");
    const original = ipc.api;
    Object.defineProperty(ipc, "api", { value: failing, configurable: true, writable: true });

    let container: HTMLElement;
    await act(async () => {
      ({ container } = render(mount()));
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });

    expect(container!).toBeTruthy();
    expect(rejections, `${name} left an unhandled rejection: ${rejections.join(" | ")}`).toEqual([]);

    Object.defineProperty(ipc, "api", { value: original, configurable: true, writable: true });
    window.removeEventListener("unhandledrejection", onUnhandled);
    spy.mockRestore();
  });
});

describe("panes clean up after themselves", () => {
  it.each(PANES)("%s unmounts without throwing", async (_name, mount) => {
    let unmount: () => void;
    await act(async () => {
      ({ unmount } = render(mount()));
    });
    expect(() => unmount!()).not.toThrow();
  });
});

describe("the empty state says something useful", () => {
  it("Today does not render a blank page when there is nothing scheduled", async () => {
    // A first-run user must be told what they are looking at, not shown an empty rectangle. This is
    // the app's landing view, so a blank one is the first thing a new install shows.
    let container: HTMLElement;
    await act(async () => {
      ({ container } = render(<TodayPane />));
    });
    await waitFor(() => expect(container!.textContent!.replace(/\s+/g, "").length).toBeGreaterThan(20));
  });
});
