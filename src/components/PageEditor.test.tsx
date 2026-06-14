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
});
