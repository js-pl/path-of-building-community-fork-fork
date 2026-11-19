// Calcs tab: upstream's calc sections with cells formatted as upstream
// shows them; clicking a cell shows its breakdown.

import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { Breakdown, Calcs, Control } from "../engine/types";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";

export function CalcsTab({ rev, setControl }: TabProps) {
  const [calcs, setCalcs] = useState<Calcs | null>(null);
  const [controls, setControls] = useState<Record<string, Control>>({});
  const [cell, setCell] = useState<{ key: string; label: string; data: Breakdown } | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [width, setWidth] = useState(0);
  const grid = useRef<HTMLDivElement>(null);
  // The open breakdown's cell, refreshed after every change
  const openCell = useRef<{ key: string; label: string; b: number[] } | null>(null);

  useEffect(() => {
    let stale = false;
    (async () => {
      const c = await engine.call<Calcs>("calcs");
      const paths = c.sections.flatMap((s) => s.subs.flatMap((u) => u.rows.flatMap((r) => r.cells.filter((x) => x.control).map((x) => x.control!))));
      const ctl = await engine.call<Record<string, Control>>("controls", { paths });
      const o = openCell.current;
      const data = o ? await engine.call<Breakdown>("breakdown", { cell: o.b }).catch(() => null) : null;
      if (stale) return;
      setControls(ctl);
      setCalcs(c);
      setCell(o && data ? { key: o.key, label: o.label, data } : null);
    })();
    return () => {
      stale = true;
    };
  }, [rev]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        openCell.current = null;
        setCell(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const g = grid.current;
    if (!g) return;
    const ro = new ResizeObserver(() => setWidth(g.clientWidth));
    ro.observe(g);
    return () => ro.disconnect();
  }, [!!calcs]);

  // Sections go to columns by their size with every subsection expanded
  // as upstream first shows them, so collapsing one never moves the others
  const columns = useMemo(() => {
    if (!calcs) return [];
    const n = Math.max(1, Math.floor((width + 8) / 368));
    const cols: number[][] = Array.from({ length: n }, () => []);
    const heights = new Array(n).fill(0);
    calcs.sections.forEach((sec, si) => {
      const h = sec.subs.reduce((t, u) => t + 1 + (u.collapsed ? 0 : u.rows.length), 1);
      const c = heights.indexOf(Math.min(...heights));
      cols[c].push(si);
      heights[c] += h;
    });
    return cols;
  }, [calcs, width]);

  if (!calcs)
    return (
      <div class="tab-loading">
        <span class="spinner" /> Loading calculations…
      </div>
    );
  const open = async (key: string, label: string, b: number[]) => {
    if (cell?.key === key) {
      openCell.current = null;
      return setCell(null);
    }
    openCell.current = { key, label, b };
    const data = await engine.call<Breakdown>("breakdown", { cell: b });
    if (openCell.current?.key === key) setCell({ key, label, data });
  };
  const close = () => {
    openCell.current = null;
    setCell(null);
  };
  return (
    <div class="calcs-tab">
      <div class="calcs-grid" ref={grid}>
        {columns.map((col, ci) => (
          <div key={ci} class="calcs-col">
            {col
              .map((si) => [si, calcs.sections[si]] as const)
              .map(([si, sec]) => (
                <section key={sec.id + si} class="calc-section" style={{ borderColor: sec.color?.startsWith("^x") ? "#" + sec.color.slice(2) : undefined }}>
                  {sec.subs.map((sub, ui) => {
                    const k = `${si}.${ui}`;
                    const isCollapsed = collapsed[k] ?? !!sub.collapsed;
                    return (
                      <div key={ui} class="calc-sub">
                        <h4 class={isCollapsed ? "collapsed" : ""} onClick={() => setCollapsed({ ...collapsed, [k]: !isCollapsed })}>
                          <span class="chevron" />
                          <ColorText text={sub.label} />
                          {sub.extra ? (
                            <span class="extra">
                              {" "}
                              <ColorText text={sub.extra} />
                            </span>
                          ) : null}
                        </h4>
                        {isCollapsed ? null : (
                          <table>
                            <tbody>
                              {sub.rows.map((row, ri) => (
                                <tr key={ri}>
                                  <th>
                                    <ColorText text={(row.color ?? "") + (row.label ?? "")} />
                                  </th>
                                  {row.cells.map((c, ci) => {
                                    const key = `${si}.${ui}.${ri}.${ci}`;
                                    if (c.control) {
                                      return (
                                        <td key={ci}>
                                          <ControlView path={c.control} c={controls[c.control]} set={setControl} />
                                        </td>
                                      );
                                    }
                                    return (
                                      <td
                                        key={ci}
                                        class={(c.breakdown ? "has-breakdown" : "") + (cell?.key === key ? " selected" : "")}
                                        onClick={c.breakdown ? () => open(key, row.label ?? "", c.breakdown!) : undefined}
                                      >
                                        <ColorText text={c.text} />
                                      </td>
                                    );
                                  })}
                                </tr>
                              ))}
                            </tbody>
                          </table>
                        )}
                      </div>
                    );
                  })}
                </section>
              ))}
          </div>
        ))}
      </div>
      {cell ? (
        <aside class="breakdown">
          <h3>
            <ColorText text={cell.label} />
            <button class="ghost" title="Close" onClick={close}>
              ✕
            </button>
          </h3>
          {cell.data.sections.length === 0 ? <p>No breakdown.</p> : null}
          {cell.data.sections.map((s, i) =>
            s.type === "text" ? (
              <div key={i} class="bd-text">
                {s.lines.map((l, j) => (
                  <div key={j}>
                    <ColorText text={l} />
                  </div>
                ))}
              </div>
            ) : (
              <div key={i} class="bd-table">
                {s.label ? (
                  <h4>
                    <ColorText text={s.label} />
                  </h4>
                ) : null}
                <table>
                  <thead>
                    <tr>
                      {s.cols.map((c) => (
                        <th key={c.key}>{c.label}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {s.rows.map((r, j) => (
                      <tr key={j}>
                        {s.cols.map((c) => (
                          <td key={c.key}>
                            <ColorText text={r[c.key]} />
                          </td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
                {s.footer ? (
                  <p class="footer">
                    <ColorText text={s.footer} />
                  </p>
                ) : null}
              </div>
            ),
          )}
        </aside>
      ) : null}
    </div>
  );
}
