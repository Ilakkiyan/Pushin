import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import PaneErrorBoundary from "./PaneErrorBoundary";

// React logs caught render errors to console.error; silence it so a passing run stays readable.
let spy: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  spy = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => spy.mockRestore());

function Boom({ throws }: { throws: boolean }): React.ReactElement {
  if (throws) throw new Error("pane exploded");
  return <div>pane content</div>;
}

describe("PaneErrorBoundary", () => {
  it("renders children untouched when nothing throws", () => {
    render(
      <PaneErrorBoundary resetKey="today">
        <Boom throws={false} />
      </PaneErrorBoundary>,
    );
    expect(screen.getByText("pane content")).toBeInTheDocument();
  });

  it("contains a crash instead of unmounting, and surfaces the message", () => {
    render(
      <PaneErrorBoundary resetKey="today">
        <Boom throws={true} />
      </PaneErrorBoundary>,
    );
    expect(screen.getByText(/This view hit a problem/i)).toBeInTheDocument();
    expect(screen.getByText(/pane exploded/)).toBeInTheDocument();
    // The reassurance matters as much as the error: the app is still running.
    expect(screen.getByText(/rest of Pushin is still running/i)).toBeInTheDocument();
  });

  it("recovers when the user navigates to another view", async () => {
    // resetKey is the active view, so switching panes must clear a stuck error.
    function Harness() {
      const [view, setView] = useState("today");
      return (
        <>
          <button onClick={() => setView("calendar")}>go</button>
          <PaneErrorBoundary resetKey={view}>
            <Boom throws={view === "today"} />
          </PaneErrorBoundary>
        </>
      );
    }
    render(<Harness />);
    expect(screen.getByText(/This view hit a problem/i)).toBeInTheDocument();

    await userEvent.click(screen.getByText("go"));
    expect(screen.getByText("pane content")).toBeInTheDocument();
    expect(screen.queryByText(/This view hit a problem/i)).toBeNull();
  });

  it("lets the user retry the same view without restarting the app", async () => {
    let shouldThrow = true;
    function Flaky() {
      if (shouldThrow) throw new Error("transient");
      return <div>recovered</div>;
    }
    render(
      <PaneErrorBoundary resetKey="today">
        <Flaky />
      </PaneErrorBoundary>,
    );
    expect(screen.getByText(/This view hit a problem/i)).toBeInTheDocument();

    shouldThrow = false;
    await userEvent.click(screen.getByRole("button", { name: /Try again/i }));
    expect(screen.getByText("recovered")).toBeInTheDocument();
  });
});
