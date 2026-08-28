import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { Sparkles } from "lucide-react";
import WhatsNew, { featuresSince, hasNewFeatures, type Feature } from "./WhatsNew";

// The post-update intro is the changelog users actually read. Showing a card for something they have
// had for three releases makes the whole overlay feel like noise; hiding the one thing they just got
// defeats the point. Both directions are pinned here.

const card = (since: string, title: string): Feature => ({ icon: Sparkles, since, title, body: `body ${title}` });

const LIST: Feature[] = [
  card("0.9.0", "unreleased"),
  card("0.8.3", "newest"),
  card("0.8.2", "middle"),
  card("0.8.0", "oldest"),
];

describe("featuresSince", () => {
  it("shows only what shipped after the version the user was on", () => {
    expect(featuresSince("0.8.2", "0.8.3", LIST).map((f) => f.title)).toEqual(["newest"]);
    expect(featuresSince("0.8.0", "0.8.3", LIST).map((f) => f.title)).toEqual(["newest", "middle"]);
  });

  it("catches a user up across several skipped releases", () => {
    // Someone who last opened the app on 0.7.1 should see every release since, not just the latest.
    expect(featuresSince("0.7.1", "0.8.3", LIST).map((f) => f.title)).toEqual(["newest", "middle", "oldest"]);
  });

  it("excludes the version the user is coming FROM", () => {
    // They already saw 0.8.2's cards when they updated to 0.8.2.
    expect(featuresSince("0.8.3", "0.8.3", LIST)).toEqual([]);
  });

  it("does not leak a card tagged for an unreleased version", () => {
    // A card can be written ahead of the release that carries it; it must stay hidden until then.
    const titles = featuresSince("0.8.0", "0.8.3", LIST).map((f) => f.title);
    expect(titles).not.toContain("unreleased");
    expect(featuresSince("0.8.3", "0.9.0", LIST).map((f) => f.title)).toEqual(["unreleased"]);
  });

  it("shows everything when the starting version is unknown", () => {
    // Cleared storage or a first run after the key was introduced. Better to over-tell than to show
    // an overlay with nothing in it.
    expect(featuresSince(null, "0.8.3", LIST)).toHaveLength(LIST.length);
    expect(featuresSince(undefined, "0.8.3", LIST)).toHaveLength(LIST.length);
    expect(featuresSince("", "0.8.3", LIST)).toHaveLength(LIST.length);
  });

  it("shows everything when there is no upper bound", () => {
    expect(featuresSince("0.8.2", null, LIST).map((f) => f.title)).toEqual(["unreleased", "newest"]);
  });

  it("keeps the list in its declared order", () => {
    // Newest first, matching the file. The staggered animation reads top-down.
    expect(featuresSince("0.7.0", "0.9.0", LIST).map((f) => f.since)).toEqual(["0.9.0", "0.8.3", "0.8.2", "0.8.0"]);
  });

  it("compares numerically, not as text", () => {
    const list = [card("0.10.0", "ten"), card("0.9.0", "nine")];
    expect(featuresSince("0.9.0", "0.10.0", list).map((f) => f.title)).toEqual(["ten"]);
  });

  it("survives a garbage stored version by showing everything since 0", () => {
    expect(featuresSince("not-a-version", "0.8.3", LIST).map((f) => f.title)).toEqual(["newest", "middle", "oldest"]);
  });
});

describe("hasNewFeatures", () => {
  it("is false when an update has nothing to announce", () => {
    // A pure bug-fix release adds no cards; App uses this to skip the overlay rather than render a
    // heading over empty space.
    expect(hasNewFeatures("0.8.3", "0.8.3")).toBe(false);
  });

  it("is true when there is something to show", () => {
    expect(hasNewFeatures("0.8.2", "0.8.3")).toBe(true);
    expect(hasNewFeatures("0.7.0", "0.8.3")).toBe(true);
  });

  it("is true when the starting version is unknown", () => {
    expect(hasNewFeatures(null, "0.8.3")).toBe(true);
  });
});

describe("WhatsNew — rendering", () => {
  it("renders only the cards for the range it was given", () => {
    render(<WhatsNew version="0.8.3" from="0.8.2" onDone={() => {}} />);
    expect(screen.getByText(/One task, however it's split/)).toBeInTheDocument();
    // 0.8.0's cards belong to a version this user already ran.
    expect(screen.queryByText(/Opens on your day/)).not.toBeInTheDocument();
  });

  it("catches up a user who skipped releases", () => {
    render(<WhatsNew version="0.8.3" from="0.8.0" onDone={() => {}} />);
    expect(screen.getByText(/One task, however it's split/)).toBeInTheDocument();
    expect(screen.getByText(/Nothing gets quietly lost/)).toBeInTheDocument();
    expect(screen.getByText(/Connect Google once/)).toBeInTheDocument();
    expect(screen.queryByText(/Opens on your day/)).not.toBeInTheDocument();
  });

  it("shows the whole list when the previous version is unknown", () => {
    render(<WhatsNew version="0.8.3" from={null} onDone={() => {}} />);
    expect(screen.getByText(/One task, however it's split/)).toBeInTheDocument();
    expect(screen.getByText(/Opens on your day/)).toBeInTheDocument();
  });

  it("shows the version it updated to", () => {
    render(<WhatsNew version="0.8.3" from="0.8.2" onDone={() => {}} />);
    expect(screen.getByText("Version 0.8.3")).toBeInTheDocument();
  });

  it("announces this release's headline changes", () => {
    // The one assertion tied to the REAL card list: a release that forgets to add its cards ships an
    // update whose "what's new" screen is silently empty, and nobody notices until it's out.
    render(<WhatsNew version="0.8.4" from="0.8.3" onDone={() => {}} />);
    expect(screen.getByText(/Your vault is a place now/)).toBeInTheDocument();
    expect(screen.getByText(/The pages you're working across/)).toBeInTheDocument();
    expect(screen.getByText(/Your files travel with your notes/)).toBeInTheDocument();
    // ...and not the previous release's, which this user already saw.
    expect(screen.queryByText(/One task, however it's split/)).not.toBeInTheDocument();
  });

  it("still renders its heading and CTA when the range is empty", () => {
    // App skips mounting it in this case, but the component must not break if it is mounted anyway.
    render(<WhatsNew version="0.8.3" from="0.8.3" onDone={() => {}} />);
    expect(screen.getByRole("button", { name: /Explore/ })).toBeInTheDocument();
    expect(screen.queryByText(/One task, however it's split/)).not.toBeInTheDocument();
  });
});
