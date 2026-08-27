import { describe, it, expect, vi, beforeEach } from "vitest";

const check = vi.fn();
const relaunch = vi.fn();
vi.mock("@tauri-apps/plugin-updater", () => ({ check: (...a: unknown[]) => check(...a) }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: (...a: unknown[]) => relaunch(...a) }));

import { checkForUpdate, installUpdate } from "./updates";

// The updater runs unattended on every launch, on machines that are offline, on dev builds with no
// updater configured, and on mobile where the plugin isn't compiled in at all. A throw on any of
// those paths is a crash at startup, which is the worst place to have one — hence the contract that
// `checkForUpdate` NEVER rejects.

beforeEach(() => {
  vi.clearAllMocks();
});

describe("checkForUpdate", () => {
  it("passes an available update straight through", async () => {
    const update = { version: "0.9.0", downloadAndInstall: vi.fn() };
    check.mockResolvedValue(update);
    await expect(checkForUpdate()).resolves.toBe(update);
  });

  it("reports null when already up to date", async () => {
    check.mockResolvedValue(null);
    await expect(checkForUpdate()).resolves.toBeNull();
  });

  it("never rejects, whatever the updater throws", async () => {
    // Dev build with no endpoint, offline, a 503 from GitHub, a malformed signature — all the same
    // answer: nothing to update. A rejection here surfaces as an unhandled error during boot.
    for (const boom of [
      new Error("network error"),
      new Error("plugin updater not found"),
      "a bare string rejection",
      undefined,
      null,
    ]) {
      check.mockRejectedValueOnce(boom);
      await expect(checkForUpdate(), `threw on ${String(boom)}`).resolves.toBeNull();
    }
  });

  it("does not relaunch merely from checking", async () => {
    check.mockResolvedValue(null);
    await checkForUpdate();
    expect(relaunch).not.toHaveBeenCalled();
  });
});

describe("installUpdate", () => {
  /** Build a fake Update that replays a scripted sequence of progress events. */
  function fakeUpdate(events: Array<Record<string, unknown>>) {
    return {
      downloadAndInstall: vi.fn(async (cb: (e: unknown) => void) => {
        for (const e of events) cb(e);
      }),
    } as never;
  }

  it("reports progress as a running percentage and relaunches when done", async () => {
    const seen: Array<{ downloaded: number; total: number; pct: number | null }> = [];
    await installUpdate(
      fakeUpdate([
        { event: "Started", data: { contentLength: 1000 } },
        { event: "Progress", data: { chunkLength: 250 } },
        { event: "Progress", data: { chunkLength: 250 } },
        { event: "Progress", data: { chunkLength: 500 } },
        { event: "Finished", data: {} },
      ]),
      (p) => seen.push(p),
    );

    expect(seen.map((p) => p.pct)).toEqual([25, 50, 100, 100]);
    expect(seen.at(-1)).toMatchObject({ downloaded: 1000, total: 1000 });
    expect(relaunch).toHaveBeenCalledTimes(1);
  });

  it("accumulates chunks rather than reporting each one in isolation", async () => {
    const seen: number[] = [];
    await installUpdate(
      fakeUpdate([
        { event: "Started", data: { contentLength: 400 } },
        { event: "Progress", data: { chunkLength: 100 } },
        { event: "Progress", data: { chunkLength: 100 } },
      ]),
      (p) => seen.push(p.downloaded),
    );
    expect(seen).toEqual([100, 200]);
  });

  it("reports a null percentage when the server sent no content length", async () => {
    // A progress bar must show indeterminate rather than dividing by zero and rendering NaN%.
    const seen: Array<number | null> = [];
    await installUpdate(
      fakeUpdate([
        { event: "Started", data: {} },
        { event: "Progress", data: { chunkLength: 100 } },
        { event: "Finished", data: {} },
      ]),
      (p) => seen.push(p.pct),
    );
    expect(seen).toEqual([null, null]);
  });

  it("works with no progress callback at all", async () => {
    await expect(
      installUpdate(
        fakeUpdate([
          { event: "Started", data: { contentLength: 10 } },
          { event: "Progress", data: { chunkLength: 10 } },
          { event: "Finished", data: {} },
        ]),
      ),
    ).resolves.toBeUndefined();
    expect(relaunch).toHaveBeenCalled();
  });

  it("does not relaunch when the install fails", async () => {
    // A half-applied update that relaunches anyway is how you get an app that will not start.
    const update = { downloadAndInstall: vi.fn(async () => Promise.reject(new Error("signature mismatch"))) } as never;
    await expect(installUpdate(update)).rejects.toThrow(/signature mismatch/);
    expect(relaunch).not.toHaveBeenCalled();
  });

  it("ignores event kinds it does not know", async () => {
    const seen: unknown[] = [];
    await expect(
      installUpdate(fakeUpdate([{ event: "SomethingNew", data: {} }, { event: "Finished", data: {} }]), (p) =>
        seen.push(p),
      ),
    ).resolves.toBeUndefined();
    expect(seen).toHaveLength(1); // just the Finished report
  });
});
