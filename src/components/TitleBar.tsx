import { useEffect, useState } from "react";
import { Minus, Square, Copy, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

const appWindow = getCurrentWindow();

export function usesNativeTitleBar(): boolean {
  const nav = navigator as Navigator & { userAgentData?: { platform?: string } };
  const platform = nav.userAgentData?.platform || navigator.platform || navigator.userAgent;
  return /Mac/i.test(platform);
}

/** A slim, frameless custom title bar (the window is `decorations: false`) with our own
 *  minimize/maximize/close + a drag region.
 *
 *  Auto-hide: when the window is **maximized or fullscreen** the bar tucks away so content gets the
 *  full height, and slides back in when you move the cursor to the very top edge — so the controls
 *  are always one hover away (never trapped). When the window is a normal floating size the bar is
 *  shown inline (you need it there to drag the window). F11 toggles fullscreen; Esc exits it. */
export default function TitleBar() {
  const nativeTitleBar = usesNativeTitleBar();
  const [maximized, setMaximized] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);

  useEffect(() => {
    if (nativeTitleBar) return;
    let unlisten: (() => void) | undefined;
    const sync = async () => {
      setMaximized(await appWindow.isMaximized());
      setFullscreen(await appWindow.isFullscreen());
    };
    sync();
    // Resize fires on maximize/restore and on fullscreen enter/exit — re-read both then.
    appWindow.onResized(sync).then((u) => (unlisten = u));

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "F11") {
        e.preventDefault();
        appWindow.isFullscreen().then((f) => appWindow.setFullscreen(!f));
      } else if (e.key === "Escape") {
        appWindow.isFullscreen().then((f) => {
          if (f) appWindow.setFullscreen(false);
        });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      unlisten?.();
      window.removeEventListener("keydown", onKey);
    };
  }, [nativeTitleBar]);

  if (nativeTitleBar) return null;

  const btn = "h-full px-3.5 grid place-items-center text-gray-400 hover:bg-white/10 hover:text-white transition";

  const bar = (
    <div data-tauri-drag-region className="h-8 flex items-stretch justify-between bg-[var(--surface)] border-b border-white/10 select-none">
      {/* Empty draggable region — the brand lives in the sidebar, so the title bar stays minimal. */}
      <div data-tauri-drag-region className="flex-1" />
      <div className="flex items-stretch">
        <button onClick={() => appWindow.minimize()} title="Minimize" className={btn}>
          <Minus className="size-3.5" />
        </button>
        <button onClick={() => appWindow.toggleMaximize()} title={maximized ? "Restore" : "Maximize"} className={btn}>
          {maximized ? <Copy className="size-3" /> : <Square className="size-3" />}
        </button>
        <button onClick={() => appWindow.close()} title="Close" className="h-full px-3.5 grid place-items-center text-gray-400 hover:bg-red-500 hover:text-white transition">
          <X className="size-4" />
        </button>
      </div>
    </div>
  );

  // Floating window → inline bar (also the drag handle to move the window).
  if (!maximized && !fullscreen) {
    return <div className="shrink-0">{bar}</div>;
  }

  // Maximized / fullscreen → take no layout height; reveal the bar on hover at the top edge.
  return (
    <div className="group relative h-0 z-50">
      {/* invisible 6px hot-zone at the very top to catch the hover */}
      <div className="absolute top-0 left-0 right-0 h-1.5" />
      <div className="absolute top-0 left-0 right-0 -translate-y-full group-hover:translate-y-0 transition-transform duration-150 shadow-lg">
        {bar}
      </div>
    </div>
  );
}
