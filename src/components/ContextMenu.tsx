import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import clsx from "clsx";
import type { LucideIcon } from "lucide-react";

/** One row of a context menu, or a hairline between groups. */
export type MenuItem =
  | { separator: true }
  | {
      separator?: false;
      label: string;
      icon?: LucideIcon;
      onSelect: () => void;
      /** Destructive rows read red and sit last, the way every desktop menu puts them. */
      danger?: boolean;
      disabled?: boolean;
    };

export type MenuAnchor = { x: number; y: number; items: MenuItem[] };

/** Rows you can actually land on with the keyboard. */
const selectable = (items: MenuItem[]) =>
  items.map((it, i) => ({ it, i })).filter(({ it }) => !it.separator && !it.disabled);

/** A cursor-anchored menu, portalled to the body so no scroll container clips it.
 *
 *  It closes on anything that would make its position a lie: a click elsewhere, Escape, a scroll, a
 *  resize, the window losing focus. Arrow keys walk it and Enter picks, because a menu you can only
 *  reach with the mouse is a menu half the people using the app cannot reach at all. */
export default function ContextMenu({ anchor, onClose }: { anchor: MenuAnchor; onClose: () => void }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ x: anchor.x, y: anchor.y });
  const [active, setActive] = useState<number | null>(null);
  const rows = selectable(anchor.items);

  // Measure once mounted and pull the menu back inside the viewport. Flipping (rather than clamping)
  // near the bottom edge keeps the cursor outside the menu, so the click that opened it can't also
  // land on a row.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const pad = 8;
    let x = anchor.x;
    let y = anchor.y;
    if (x + width > window.innerWidth - pad) x = Math.max(pad, anchor.x - width);
    if (y + height > window.innerHeight - pad) y = Math.max(pad, anchor.y - height);
    setPos({ x, y });
    el.focus();
  }, [anchor]);

  useEffect(() => {
    const away = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    // Capture the scroll: a menu pinned to a cursor position is wrong the moment the page moves.
    window.addEventListener("mousedown", away);
    window.addEventListener("scroll", onClose, true);
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    return () => {
      window.removeEventListener("mousedown", away);
      window.removeEventListener("scroll", onClose, true);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
    };
  }, [onClose]);

  const step = (dir: 1 | -1) => {
    if (rows.length === 0) return;
    const at = rows.findIndex((r) => r.i === active);
    const next = at === -1 ? (dir === 1 ? 0 : rows.length - 1) : (at + dir + rows.length) % rows.length;
    setActive(rows[next].i);
  };

  const pick = (item: MenuItem) => {
    if (item.separator || item.disabled) return;
    onClose();
    item.onSelect();
  };

  return createPortal(
    <div
      ref={ref}
      role="menu"
      tabIndex={-1}
      aria-label="Context menu"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") return onClose();
        if (e.key === "ArrowDown") return (e.preventDefault(), step(1));
        if (e.key === "ArrowUp") return (e.preventDefault(), step(-1));
        if (e.key === "Enter" && active != null) {
          e.preventDefault();
          pick(anchor.items[active]);
        }
      }}
      className="pop-in fixed z-[100] min-w-[190px] border border-white/15 bg-[var(--raised)] py-1 shadow-2xl shadow-black/60 outline-none"
    >
      {anchor.items.map((item, i) =>
        item.separator ? (
          <div key={i} className="my-1 h-px bg-white/10" />
        ) : (
          <button
            key={i}
            role="menuitem"
            disabled={item.disabled}
            onMouseEnter={() => setActive(i)}
            onClick={() => pick(item)}
            className={clsx(
              "flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-[13px]",
              item.disabled
                ? "cursor-default text-[var(--ink-faint)] opacity-50"
                : item.danger
                  ? "text-rose-300 hover:bg-white/10"
                  : "text-[var(--ink)] hover:bg-white/10",
              active === i && !item.disabled && "bg-white/10",
            )}
          >
            {item.icon ? <item.icon className="size-3.5 shrink-0 text-[var(--ink-faint)]" /> : <span className="size-3.5 shrink-0" />}
            <span className="truncate">{item.label}</span>
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
