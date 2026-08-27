import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The same contract the pane smoke suite enforces, for the components none of the pane tests reach.
// These are the pieces of chrome that render on top of everything — a banner, the first-run guide,
// the sync panel — so one of them throwing takes the surrounding view with it.

function defaultFor(name: string): unknown {
  if (/^(list|search|suggest|backlinks|recent|all)/.test(name)) return [];
  if (name === "briefing") {
    return { greeting: "Good morning", date: "2026-08-27", lines: [], events: [], tasks: [], habits: [] };
  }
  if (name === "syncStatus") {
    return { nodeId: "abc", deviceName: "This device", running: false, peers: [], useRelay: true };
  }
  if (name === "llmStatus") {
    return { reachable: false, baseUrl: "http://127.0.0.1:8080", modelPresent: false, modelId: "m", models: [] };
  }
  return null;
}

vi.mock("../lib/ipc", () => ({
  api: new Proxy(
    {},
    {
      get: (_t, prop) => {
        const name = String(prop);
        if (name === "then") return undefined;
        return vi.fn(async () => defaultFor(name));
      },
    },
  ),
}));

import { useStore } from "../state/store";
import AiMemory from "./AiMemory";
import BriefingCard from "./BriefingCard";
import CalendarLabelControls from "./CalendarLabelControls";
import CalendarLegend from "./CalendarLegend";
import ConflictBanner from "./ConflictBanner";
import DevicesSync from "./DevicesSync";
import InferenceSetup from "./InferenceSetup";
import LabelPicker from "./LabelPicker";
import MobileShell from "./MobileShell";
import OpeningAnimation from "./OpeningAnimation";
import StaleTasks from "./StaleTasks";
import ViewToggle from "./ViewToggle";
import WelcomeBack from "./WelcomeBack";
import WelcomeGuide from "./WelcomeGuide";
import WhatsNew from "./WhatsNew";

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

const task = (id: number, title: string) => ({
  id,
  projectId: null,
  title,
  notes: "",
  estimatedMinutes: 60,
  deadline: null,
  earliestStart: null,
  priority: 2,
  minChunkMinutes: 30,
  maxChunkMinutes: 120,
  status: "todo",
  createdAt: "",
  missedCount: 4,
  lastMissedOn: "2026-08-20",
  dependsOn: [],
});

beforeEach(() => {
  vi.clearAllMocks();
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
    calMode: "week",
    calColorByLabel: false,
    calLabelFilterIds: [],
    view: "today",
    space: "planner",
    busy: false,
    llm: { reachable: false, baseUrl: "", modelPresent: false, modelId: "m", models: [] },
  } as never);
});

const COMPONENTS: Array<[string, () => React.ReactElement]> = [
  ["AiMemory", () => <AiMemory />],
  ["BriefingCard", () => <BriefingCard />],
  ["CalendarLabelControls", () => <CalendarLabelControls />],
  ["CalendarLegend", () => <CalendarLegend />],
  ["ConflictBanner", () => <ConflictBanner />],
  ["DevicesSync", () => <DevicesSync />],
  ["InferenceSetup", () => <InferenceSetup />],
  ["LabelPicker", () => <LabelPicker kind="task" entityId={1} />],
  ["MobileShell", () => <MobileShell />],
  ["OpeningAnimation", () => <OpeningAnimation ready onDone={() => {}} />],
  ["StaleTasks", () => <StaleTasks tasks={[task(1, "Old thing")]} />],
  ["ViewToggle", () => <ViewToggle />],
  ["WelcomeBack", () => <WelcomeBack onEnter={() => {}} />],
  ["WelcomeGuide", () => <WelcomeGuide onDone={() => {}} />],
  ["WhatsNew", () => <WhatsNew version="0.8.3" from="0.8.2" onDone={() => {}} />],
];

describe("every component mounts and unmounts cleanly", () => {
  it.each(COMPONENTS)("%s renders without throwing", async (name, mount) => {
    const errors: unknown[] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...a) => errors.push(a[0]));

    let unmount: () => void;
    await act(async () => {
      ({ unmount } = render(mount()));
    });

    const fatal = errors.filter((e) => String(e).match(/not a function|undefined is not|Cannot read|of null|of undefined/i));
    expect(fatal, `${name} threw: ${fatal.join(" | ")}`).toEqual([]);
    expect(() => unmount!()).not.toThrow();
    spy.mockRestore();
  });

  it.each(COMPONENTS)("%s survives a backend that rejects every call", async (name, mount) => {
    const rejections: unknown[] = [];
    const onUnhandled = (e: PromiseRejectionEvent) => {
      e.preventDefault();
      rejections.push(e.reason);
    };
    window.addEventListener("unhandledrejection", onUnhandled);
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    const ipc = await import("../lib/ipc");
    const original = ipc.api;
    Object.defineProperty(ipc, "api", {
      value: new Proxy({}, { get: () => vi.fn(async () => Promise.reject(new Error("backend unavailable"))) }),
      configurable: true,
      writable: true,
    });

    await act(async () => {
      render(mount());
    });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });

    expect(rejections, `${name} left an unhandled rejection: ${rejections.join(" | ")}`).toEqual([]);

    Object.defineProperty(ipc, "api", { value: original, configurable: true, writable: true });
    window.removeEventListener("unhandledrejection", onUnhandled);
    spy.mockRestore();
  });
});

describe("ConflictBanner", () => {
  it("renders nothing when the schedule is clean", () => {
    const { container } = render(<ConflictBanner />);
    expect(container).toBeEmptyDOMElement();
  });

  it("names the task behind an unschedulable conflict", () => {
    useStore.setState({
      conflicts: [{ kind: "unschedulable", taskId: 3, title: "Write the thing", remainingMinutes: 90 }],
    } as never);
    render(<ConflictBanner />);
    expect(screen.getByText(/Write the thing/)).toBeInTheDocument();
  });

  it("surfaces a missed deadline", () => {
    useStore.setState({
      conflicts: [
        {
          kind: "deadlineMiss",
          taskId: 4,
          title: "Grant application",
          scheduledEnd: "2026-08-28T17:00:00",
          deadline: "2026-08-27T23:59:00",
        },
      ],
    } as never);
    render(<ConflictBanner />);
    expect(screen.getByText(/Grant application/)).toBeInTheDocument();
  });

  it("reports a dependency cycle rather than silently dropping the tasks", () => {
    useStore.setState({ conflicts: [{ kind: "dependencyCycle", taskIds: [1, 2] }] } as never);
    const { container } = render(<ConflictBanner />);
    expect(container.textContent!.length).toBeGreaterThan(0);
  });
});

describe("ViewToggle", () => {
  it("switches the calendar between week and month", async () => {
    render(<ViewToggle />);
    const month = screen.getByRole("button", { name: /month/i });
    await userEvent.click(month);
    await waitFor(() => expect(useStore.getState().calMode).toBe("month"));

    await userEvent.click(screen.getByRole("button", { name: /week/i }));
    await waitFor(() => expect(useStore.getState().calMode).toBe("week"));
  });
});

describe("StaleTasks", () => {
  it("says nothing when nothing has gone stale", () => {
    const { container } = render(<StaleTasks tasks={[]} />);
    expect(container.textContent).not.toMatch(/\w{4,}/);
  });

  it("surfaces a task that keeps being pushed forward", () => {
    const { container } = render(<StaleTasks tasks={[task(1, "Perpetually deferred")]} />);
    expect(container.textContent!.length).toBeGreaterThan(0);
  });
});
