// Upstream's own controls (dropdowns, edit boxes, check boxes), rendered as
// native inputs; changing one calls its upstream handler through the engine.

import type { Control } from "../engine/types";
import { ColorText, stripColors } from "./ColorText";

export type SetControl = (path: string, change: { sel?: number; text?: string; state?: boolean; click?: boolean; val?: number }) => void;

export function ControlView({ path, c, set, label }: { path: string; c?: Control; set: SetControl; label?: string }) {
  if (!c || !c.shown) return null;
  const title = c.tooltip ? stripColors(c.tooltip) : undefined;
  const disabled = !c.enabled;
  let input;
  if (c.class === "DropDownControl") {
    input = (
      <select disabled={disabled} title={title} value={String(c.sel ?? 1)} onChange={(e) => set(path, { sel: Number((e.target as HTMLSelectElement).value) })}>
        {(c.list ?? []).map((l, i) => (
          <option key={i} value={String(i + 1)}>
            {stripColors(l).replace(/\s{2,}/g, " - ")}
          </option>
        ))}
      </select>
    );
  } else if ((c.class === "EditControl" || c.class === "ResizableEditControl") && c.multiline) {
    input = (
      <textarea
        rows={6}
        disabled={disabled}
        title={title}
        value={c.text ?? ""}
        placeholder={c.placeholder ?? ""}
        onChange={(e) => set(path, { text: (e.target as HTMLTextAreaElement).value })}
      />
    );
  } else if (c.class === "EditControl" || c.class === "ResizableEditControl") {
    input = (
      <input
        type="text"
        disabled={disabled}
        title={title}
        value={c.text ?? ""}
        placeholder={c.placeholder ?? ""}
        onChange={(e) => set(path, { text: (e.target as HTMLInputElement).value })}
      />
    );
  } else if (c.class === "CheckBoxControl") {
    input = <input type="checkbox" disabled={disabled} title={title} checked={!!c.state} onChange={(e) => set(path, { state: (e.target as HTMLInputElement).checked })} />;
  } else if (c.class === "SliderControl") {
    input = (
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        disabled={disabled}
        title={title}
        value={String(c.val ?? 0)}
        onChange={(e) => set(path, { val: Number((e.target as HTMLInputElement).value) })}
      />
    );
  } else if (c.class === "LabelControl") {
    input = <ColorText text={c.label} />;
    return <div class="control">{input}</div>;
  } else if (c.class === "ButtonControl") {
    input = (
      <button disabled={disabled} title={title} onClick={() => set(path, { click: true })}>
        {stripColors(c.label ?? "")}
      </button>
    );
  } else {
    return null;
  }
  const text = label ?? (c.class !== "ButtonControl" ? c.label : undefined);
  return (
    <label class="control" title={title}>
      {text ? <span class="control-label">{stripColors(text)}</span> : null}
      {input}
    </label>
  );
}
