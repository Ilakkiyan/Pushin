import { useEffect, useState } from "react";
import { Notebook, Loader2, Plus, FolderOpen } from "lucide-react";
import { useStore } from "../state/store";
import { api, type Page } from "../lib/ipc";
import PageEditor from "../components/PageEditor";

/** The vault workspace: hosts the block editor for the currently open page, or an empty state. */
export default function VaultPane() {
  const currentPageId = useStore((s) => s.currentPageId);
  const createPage = useStore((s) => s.createPage);
  const openFolder = useStore((s) => s.openFolder);
  const pages = useStore((s) => s.pages);
  const [page, setPage] = useState<Page | null>(null);
  const [loading, setLoading] = useState(false);

  // Fetch the full page (with body) whenever the open page changes. The lightweight tree rows in
  // the store don't carry content_json, so the editor needs a full getPage.
  useEffect(() => {
    let cancelled = false;
    if (currentPageId == null) {
      setPage(null);
      return;
    }
    setLoading(true);
    api
      .getPage(currentPageId)
      .then((p) => {
        if (!cancelled) setPage(p);
      })
      .catch(() => {
        if (!cancelled) setPage(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [currentPageId]);

  if (currentPageId == null || (!loading && !page)) {
    return (
      <div className="h-full w-full grid place-items-center text-gray-500">
        <div className="flex flex-col items-center gap-3 text-center">
          <Notebook className="size-8 text-gray-600" />
          <p className="text-sm">
            {pages.length === 0 ? "Your vault is empty." : "No page open. Browse your files, or start a new one."}
          </p>
          {/* Two ways out of an empty editor: go find something, or make something. Before the file
              browser existed this said "select a page from the sidebar", which was a dead end when
              the sidebar was collapsed. */}
          <div className="flex items-center gap-2">
            <button
              onClick={() => createPage(null)}
              className="flex items-center gap-2 text-sm px-4 py-2 bg-white/90 hover:bg-white text-gray-900 font-medium"
            >
              <Plus className="size-4" /> New page
            </button>
            <button
              onClick={() => openFolder(null)}
              className="flex items-center gap-2 text-sm px-4 py-2 hoverable border border-white/10"
            >
              <FolderOpen className="size-4" /> Browse files
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (loading || !page) {
    return (
      <div className="h-full w-full grid place-items-center text-gray-500">
        <Loader2 className="size-5 animate-spin" />
      </div>
    );
  }

  // Remount the editor per page so its internal block state resets cleanly.
  return <PageEditor key={page.id} page={page} />;
}
