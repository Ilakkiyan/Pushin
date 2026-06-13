import { BlockNoteSchema, defaultInlineContentSpecs } from "@blocknote/core";
import { createReactInlineContentSpec } from "@blocknote/react";
import { useStore } from "../state/store";

/** A wikilink to another vault page, rendered as a clickable `[[Title]]` chip. `pageId` is the
 *  target's id as a string (BlockNote inline props are strings); empty = an unresolved/ghost link. */
export const PageLinkChip = createReactInlineContentSpec(
  {
    type: "pageLink",
    propSchema: {
      pageId: { default: "" },
      title: { default: "" },
    },
    content: "none",
  },
  {
    render: (props) => <PageLink pageId={props.inlineContent.props.pageId} title={props.inlineContent.props.title} />,
  },
);

function PageLink({ pageId, title }: { pageId: string; title: string }) {
  const openPage = useStore((s) => s.openPage);
  const id = Number(pageId);
  return (
    <span
      contentEditable={false}
      onClick={() => {
        if (Number.isFinite(id) && id > 0) openPage(id);
      }}
      className="text-indigo-300 hover:text-indigo-200 hover:underline cursor-pointer rounded px-0.5"
      title={`Open "${title}"`}
    >
      [[{title}]]
    </span>
  );
}

/** The editor schema: all default blocks/inline content plus our `pageLink` wikilink. */
export const schema = BlockNoteSchema.create({
  inlineContentSpecs: {
    ...defaultInlineContentSpecs,
    pageLink: PageLinkChip,
  },
});

/** The schema's partial-block type, for typing initial editor content. */
export type PartialPageBlock = typeof schema.PartialBlock;
