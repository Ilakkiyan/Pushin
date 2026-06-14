import { useEffect, useMemo, useRef, useState } from "react";
import { useCreateBlockNote, SuggestionMenuController, getDefaultReactSlashMenuItems, type DefaultReactSuggestionItem } from "@blocknote/react";
import { filterSuggestionItems } from "@blocknote/core";
import { BlockNoteView } from "@blocknote/mantine";
import { Check, FileText, Loader2, Link2, CheckSquare, CalendarDays } from "lucide-react";
import { useStore } from "../state/store";
import { api, type Page, type EntityRef } from "../lib/ipc";
import { blocksToPlainText, extractLinkTitles, pageToInitialContent } from "../lib/blocks";
import { schema, type PartialPageBlock } from "../lib/editorSchema";
import LabelPicker from "./LabelPicker";

const SAVE_DEBOUNCE_MS = 600;

/** The Notion-style document editor for a single vault page. Mounted with `key={page.id}` so it
 *  resets cleanly when switching pages. Autosaves (debounced) title + block JSON + derived plaintext
 *  + outgoing wikilinks. Type `[[` to link another page. */
export default function PageEditor({ page }: { page: Page }) {
  const savePage = useStore((s) => s.savePage);
  const pages = useStore((s) => s.pages);
  const loadPages = useStore((s) => s.loadPages);
  const openPage = useStore((s) => s.openPage);
  const tasks = useStore((s) => s.tasks);
  const events = useStore((s) => s.events);
  const setView = useStore((s) => s.setView);
  const [title, setTitle] = useState(page.title === "Untitled" ? "" : page.title);
  const [status, setStatus] = useState<"idle" | "saving" | "saved">("idle");
  const [backlinks, setBacklinks] = useState<Page[]>([]);
  const [mentions, setMentions] = useState<Page[]>([]);
  const [entities, setEntities] = useState<EntityRef[]>([]);

  const initialContent = useMemo(() => pageToInitialContent(page) as PartialPageBlock[] | undefined, [page]);
  const editor = useCreateBlockNote({ schema, initialContent });

  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const dirty = useRef(false);
  // Mirror the latest title into a ref so the debounced save (whose closure was captured a keystroke
  // earlier) reads the current value rather than dropping the last character typed before a pause.
  const titleRef = useRef(title);
  titleRef.current = title;

  const refreshBacklinks = () => {
    api.pageBacklinks(page.id).then(setBacklinks).catch(() => {});
    api.unlinkedMentions(page.id).then(setMentions).catch(() => {});
  };
  useEffect(() => {
    refreshBacklinks();
    api.pageEntities(page.id).then(setEntities).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page.id]);

  // Resolve linked task/event refs to current titles from the store.
  const linkedEntities = entities
    .map((e) => {
      const title = e.kind === "task" ? tasks.find((t) => t.id === e.id)?.title : events.find((v) => v.id === e.id)?.title;
      return title ? { ...e, title } : null;
    })
    .filter((x): x is EntityRef & { title: string } => x !== null);

  const persist = async () => {
    const blocks = editor.document;
    const text = blocksToPlainText(blocks);
    const finalTitle = titleRef.current.trim() || text.split("\n")[0]?.slice(0, 80) || "Untitled";
    await savePage(page.id, finalTitle, page.icon ?? null, text, JSON.stringify(blocks), extractLinkTitles(blocks));
    dirty.current = false;
  };

  // Debounced autosave; reads the latest title + document at fire time.
  const scheduleSave = () => {
    dirty.current = true;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(async () => {
      setStatus("saving");
      try {
        await persist();
        setStatus("saved");
        refreshBacklinks();
      } catch {
        setStatus("idle");
      }
    }, SAVE_DEBOUNCE_MS);
  };

  // Flush a pending save on unmount (e.g. navigating away) so nothing is lost.
  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current);
      if (dirty.current) persist().catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // `[[` link picker. Triggered on "[" (a second "[" lands in the query, which we strip), listing
  // matching pages plus a "create new page" option.
  const getLinkItems = async (raw: string): Promise<DefaultReactSuggestionItem[]> => {
    const query = raw.replace(/^\[+/, "").trim();
    const insertLink = (p: Page) =>
      editor.insertInlineContent([
        { type: "pageLink", props: { pageId: String(p.id), title: p.title } },
        " ",
      ]);

    const matches = pages
      .filter((p) => p.id !== page.id && p.title.toLowerCase().includes(query.toLowerCase()))
      .slice(0, 8)
      .map<DefaultReactSuggestionItem>((p) => ({
        title: p.title,
        onItemClick: () => insertLink(p),
      }));

    const items = [...matches];
    if (query.length > 0 && !pages.some((p) => p.title.toLowerCase() === query.toLowerCase())) {
      items.push({
        title: `Create "${query}"`,
        onItemClick: async () => {
          const created = await api.createPage(query, null);
          await loadPages();
          insertLink(created);
        },
      });
    }
    return items;
  };

  // Starter-content templates, added to the "/" slash menu under a Templates group.
  const templateItems: DefaultReactSuggestionItem[] = [
    {
      title: "Meeting note",
      subtext: "Attendees, agenda, notes, action items",
      aliases: ["meeting", "template"],
      group: "Templates",
      onItemClick: () =>
        editor.insertBlocks(
          [
            { type: "heading", props: { level: 2 }, content: "Meeting" },
            { type: "paragraph", content: "Attendees: " },
            { type: "heading", props: { level: 3 }, content: "Agenda" },
            { type: "bulletListItem", content: "" },
            { type: "heading", props: { level: 3 }, content: "Notes" },
            { type: "paragraph", content: "" },
            { type: "heading", props: { level: 3 }, content: "Action items" },
            { type: "checkListItem", content: "" },
          ],
          editor.getTextCursorPosition().block,
          "after",
        ),
    },
    {
      title: "Daily plan",
      subtext: "Focus, tasks, notes",
      aliases: ["daily", "journal", "template"],
      group: "Templates",
      onItemClick: () =>
        editor.insertBlocks(
          [
            { type: "heading", props: { level: 3 }, content: "Today's focus" },
            { type: "paragraph", content: "" },
            { type: "heading", props: { level: 3 }, content: "Tasks" },
            { type: "checkListItem", content: "" },
            { type: "heading", props: { level: 3 }, content: "Notes" },
            { type: "paragraph", content: "" },
          ],
          editor.getTextCursorPosition().block,
          "after",
        ),
    },
  ];

  return (
    <div className="h-full w-full overflow-y-auto">
      <div className="max-w-3xl mx-auto px-12 py-10">
        <div className="flex items-center gap-2 mb-2 h-5">
          {status === "saving" && (
            <span className="text-[11px] text-gray-500 flex items-center gap-1">
              <Loader2 className="size-3 animate-spin" /> Saving…
            </span>
          )}
          {status === "saved" && (
            <span className="text-[11px] text-gray-600 flex items-center gap-1">
              <Check className="size-3" /> Saved
            </span>
          )}
        </div>
        <input
          value={title}
          onChange={(e) => {
            setTitle(e.target.value);
            scheduleSave();
          }}
          placeholder="Untitled"
          className="w-full bg-transparent outline-none text-3xl font-bold tracking-tight placeholder:text-gray-700 mb-2"
        />
        <div className="mb-4">
          <LabelPicker kind="page" entityId={page.id} />
        </div>
        <BlockNoteView editor={editor} theme="dark" onChange={scheduleSave} className="pushin-editor" slashMenu={false}>
          <SuggestionMenuController triggerCharacter="[" getItems={getLinkItems} />
          <SuggestionMenuController
            triggerCharacter="/"
            getItems={async (q) => filterSuggestionItems([...getDefaultReactSlashMenuItems(editor), ...templateItems], q)}
          />
        </BlockNoteView>

        {linkedEntities.length > 0 && (
          <section className="mt-10 pt-6 border-t border-white/10">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-500 flex items-center gap-1.5 mb-3">
              <Link2 className="size-3.5" /> Linked tasks & events ({linkedEntities.length})
            </h2>
            <div className="space-y-1">
              {linkedEntities.map((e) => (
                <button
                  key={`${e.kind}-${e.id}`}
                  onClick={() => setView("calendar")}
                  className="w-full flex items-center gap-2 text-left text-sm px-3 py-2 rounded-lg text-gray-300 hover:bg-white/5 hover:text-white"
                >
                  {e.kind === "task" ? <CheckSquare className="size-3.5 shrink-0 text-emerald-400/70" /> : <CalendarDays className="size-3.5 shrink-0 text-rose-400/70" />}
                  <span className="truncate">{e.title}</span>
                  <span className="ml-auto text-[10px] text-gray-600">{e.kind}</span>
                </button>
              ))}
            </div>
          </section>
        )}

        {backlinks.length > 0 && (
          <section className="mt-10 pt-6 border-t border-white/10">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-500 flex items-center gap-1.5 mb-3">
              <Link2 className="size-3.5" /> Linked references ({backlinks.length})
            </h2>
            <div className="space-y-1">
              {backlinks.map((b) => (
                <button
                  key={b.id}
                  onClick={() => openPage(b.id)}
                  className="w-full flex items-center gap-2 text-left text-sm px-3 py-2 rounded-lg text-gray-300 hover:bg-white/5 hover:text-white"
                >
                  <span className="shrink-0">{b.icon ?? <FileText className="size-3.5 inline text-gray-500" />}</span>
                  <span className="truncate">{b.title}</span>
                </button>
              ))}
            </div>
          </section>
        )}

        {mentions.length > 0 && (
          <section className="mt-8">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-gray-600 flex items-center gap-1.5 mb-2">
              Unlinked mentions ({mentions.length})
            </h2>
            <p className="text-[11px] text-gray-600 mb-2">These pages mention "{page.title}" but don't link it. Open one and type [[ to link.</p>
            <div className="space-y-1">
              {mentions.map((m) => (
                <button
                  key={m.id}
                  onClick={() => openPage(m.id)}
                  className="w-full flex items-center gap-2 text-left text-sm px-3 py-2 rounded-lg text-gray-400 hover:bg-white/5 hover:text-white"
                >
                  <span className="shrink-0">{m.icon ?? <FileText className="size-3.5 inline text-gray-500" />}</span>
                  <span className="truncate">{m.title}</span>
                </button>
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
