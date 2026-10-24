// Calcs tab: upstream's calc sections with cells formatted as upstream
// shows them; clicking a cell shows its breakdown.

import { useEffect, useState } from "preact/hooks";
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

  useEffect(() => {
    (async () => {
      const c = await engine.call<Calcs>("calcs");
      const paths = c.sections.flatMap((s) => s.subs.flatMap((u) => u.rows.flatMap((r) => r.cells.filter((x) => x.control).map((x) => x.control!))));
      setControls(await engine.call<Record<string, Control>>("controls", { paths }));
      setCalcs(c);
      setCell(null);
    })();
  }, [rev]);

  if (!calcs) return <p>Loading...</p>;
  const open = async (key: string, label: string, b: number[]) => {
    if (cell?.key === key) return setCell(null);
    setCell({ key, label, data: await engine.call<Breakdown>("breakdown", { cell: b }) });
  };
  return (
    <div class="calcs-tab">
      <div class="calcs-grid">
        {calcs.sections.map((sec, si) => (
          <section key={sec.id + si} class="calc-section" style={{ borderColor: sec.color?.startsWith("^x") ? "#" + sec.color.slice(2) : undefined }}>
            {sec.subs.map((sub, ui) => {
              const k = `${si}.${ui}`;
              const isCollapsed = collapsed[k] ?? !!sub.collapsed;
              return (
                <div key={ui} class="calc-sub">
                  <h4 onClick={() => setCollapsed({ ...collapsed, [k]: !isCollapsed })}>
                    {isCollapsed ? "+ " : "- "}
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
      {cell ? (
        <aside class="breakdown">
          <h3>
            <ColorText text={cell.label} /> <button onClick={() => setCell(null)}>close</button>
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
