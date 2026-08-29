/** The vault page currently being dragged, shared across the sidebar tree and the file browser.
 *
 *  A browser deliberately withholds `dataTransfer.getData` during `dragover`: the payload only
 *  arrives on `drop`. So a drop target that wants to say "yes, land here" (or refuse, and let the
 *  drag fall through to the folder behind it) cannot read the drag it is being offered. Every real
 *  drag in the vault starts inside this app, so the source is recorded here on `dragstart` and both
 *  sides read it while hovering. The `text/page` payload is still set and is still what a drop
 *  reads, so a drag that somehow arrives without passing through `dragstart` keeps working.
 *
 *  Module state rather than React state on purpose: the drag crosses component trees (sidebar to
 *  browser and back), and a re-render per dragover would be a lot of churn for a value nothing
 *  renders. */
let dragging: number | null = null;

export const setPageDrag = (id: number | null) => {
  dragging = id;
};

export const pageDrag = (): number | null => dragging;

/** The dragged page id: the live drag first, falling back to the payload a drop carries. */
export function dragSource(e: { dataTransfer?: DataTransfer | null }): number | null {
  if (dragging != null) return dragging;
  const id = Number(e.dataTransfer?.getData("text/page"));
  return id ? id : null;
}
