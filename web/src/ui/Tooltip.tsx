import { useLayoutEffect, useRef } from "preact/hooks";
import type { TooltipLine } from "../engine/types";
import { ColorText } from "./ColorText";

export function TooltipView({ lines }: { lines: TooltipLine[] }) {
  return (
    <div class="tooltip">
      {lines.map((l, i) =>
        l.separator ? (
          <hr key={i} />
        ) : (
          <div key={i} class="tooltip-line" style={{ fontSize: `${Math.max(11, (l.size ?? 16) - 2)}px` }}>
            <ColorText text={l.text || " "} />
          </div>
        ),
      )}
    </div>
  );
}

// A tooltip beside a point (x, y in the viewport), `gap` pixels away: to
// its right where it fits, else to its left, and moved up as far as needed
// to stay on screen. It is measured before it shows, so it never jumps.
export function FloatingTooltip({ lines, x, y, gap = 12 }: { lines: TooltipLine[]; x: number; y: number; gap?: number }) {
  const el = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const e = el.current;
    if (!e) return;
    const w = e.offsetWidth;
    const h = e.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left = x + gap + 4;
    if (left + w > vw - 8) left = x - gap - 4 - w;
    if (left < 8) left = Math.max(8, Math.min(vw - w - 8, x - w / 2));
    const top = Math.max(8, Math.min(y - 24, vh - h - 8));
    e.style.left = `${left}px`;
    e.style.top = `${top}px`;
    e.style.visibility = "visible";
  });
  return (
    <div ref={el} class="floating" style={{ visibility: "hidden", left: "0px", top: "0px" }}>
      <TooltipView lines={lines} />
    </div>
  );
}
