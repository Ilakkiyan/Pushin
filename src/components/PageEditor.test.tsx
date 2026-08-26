import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { Page } from "../lib/ipc";

// Mock the whole BlockNote stack — jsdom can't drive ProseMirror. We test our autosave/link-extraction
// wiring around a fake editor; real editing is covered by the Playwright E2E.
vi.mock("@blocknote/react", () => ({
  useCreateBlockNote: () => ({
    document: [],
    insertInlineContent: vi.fn(),
    insertBlocks: vi.fn(),
    getTextCursorPosition: () => ({ block: {} }),
  }),
  SuggestionMenuController: () => null,
  getDefaultReactSlashMenuItems: () => [],
}));
vi.mock("@blocknote/mantine", () => ({ BlockNoteView: ({ children }: { children?: unknown }) => <div data-testid="bn">{children as never}</div> }));
vi.mock("@blocknote/core", () => ({ filterSuggestionItems: (items: unknown) => items }));
vi.mock("../lib/editorSchema", () => ({ schema: {} }));

vi.mock("../lib/ipc", () => ({
  api: {
    updatePage: vi.fn().mockResolvedValue({ id: 1 }),
    listPages: vi.fn().mockResolvedValue([]),
    pageBacklinks: vi.fn().mockResolvedValue([]),
    unlinkedMentions: vi.fn().mockResolvedValue([]),
    pageEntities: vi.fn().mockResolvedValue([]),
    labelsFor: vi.fn().mockResolvedValue([]),
  },
}));

import PageEditor from "./PageEditor";
import { api } from "../lib/ipc";

const page: Page = {
  id: 1,
  title: "Doc",
  content: "",
  sortOrder: 0,
  archived: false,
  inbox: false,
  createdAt: "",
  updatedAt: "",
  indexed: false,
};

beforeEach(() => vi.clearAllMocks());

describe("PageEditor (autosave wiring)", () => {
  it("loads backlinks / mentions / linked entities on mount", async () => {
    render(<PageEditor page={page} />);
    await waitFor(() => {
      expect(api.pageBacklinks).toHaveBeenCalledWith(1);
      expect(api.unlinkedMentions).toHaveBeenCalledWith(1);
      expect(api.pageEntities).toHaveBeenCalledWith(1);
    });
  });

  it("debounce-saves after the title is edited", async () => {
    render(<PageEditor page={page} />);
    const title = screen.getByPlaceholderText("Untitled");
    await userEvent.clear(title);
    await userEvent.type(title, "Roadmap");
    // Debounced: poll until the latest save carries the final title (intermediate saves may fire).
    await waitFor(
      () => {
        const last = (api.updatePage as ReturnType<typeof vi.fn>).mock.calls.at(-1);
        expect(last?.[0]).toBe(1);
        expect(last?.[1]).toBe("Roadmap");
      },
      { timeout: 3000 },
    );
  });

  it("says so when a save fails, instead of looking idle", async () => {
    // A failed save used to render as "idle" — indistinguishable from "nothing to save" — so the
    // user kept typing into a page that was no longer persisting. The vault is the second brain;
    // silent data loss there is the worst failure this app can have.
    (api.updatePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("disk full"));
    render(<PageEditor page={page} />);
    const title = screen.getByPlaceholderText("Untitled");
    await userEvent.clear(title);
    await userEvent.type(title, "Roadmap");

    await waitFor(() => expect(screen.getByText(/Couldn't save/i)).toBeInTheDocument(), { timeout: 4000 });
  });

  it("retries on its own after a failed save and clears the warning", async () => {
    // Without a retry a transient failure waits for the next keystroke — and a user who has stopped
    // typing never produces one.
    (api.updatePage as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("offline"));
    render(<PageEditor page={page} />);
    const title = screen.getByPlaceholderText("Untitled");
    await userEvent.clear(title);
    await userEvent.type(title, "Roadmap");

    await waitFor(() => expect(screen.getByText(/Couldn't save/i)).toBeInTheDocument(), { timeout: 4000 });
    // The next attempt succeeds (the mock only rejects once) and the warning clears itself.
    await waitFor(() => expect(screen.getByText(/Saved/i)).toBeInTheDocument(), { timeout: 15000 });
    expect(screen.queryByText(/Couldn't save/i)).toBeNull();
  }, 20000);
});
