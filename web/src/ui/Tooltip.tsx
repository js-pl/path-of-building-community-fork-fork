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
            <ColorText text={l.text || " "} />
          </div>
        ),
      )}
    </div>
  );
}

// A tooltip that follows the pointer
export function FloatingTooltip({ lines, x, y }: { lines: TooltipLine[]; x: number; y: number }) {
  const left = Math.min(x + 16, window.innerWidth - 420);
  return (
    <div class="floating" style={{ left: `${Math.max(4, left)}px`, top: `${Math.min(y + 16, window.innerHeight - 100)}px` }}>
      <TooltipView lines={lines} />
    </div>
  );
}
