// Skills tab: socket groups and their gems, edited the way upstream's skills
// tab edits them

import { useEffect, useMemo, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { Gem, Skills, TooltipLine } from "../engine/types";
import { ColorText, stripColors } from "./ColorText";
import { TooltipView } from "./Tooltip";

type GemInfo = { name: string; support: boolean; color?: number; tags?: string };

let gemListCache: Promise<GemInfo[]> | null = null;

export function SkillsTab({ act, rev }: TabProps) {
  const [skills, setSkills] = useState<Skills | null>(null);
  const [sel, setSel] = useState(1);
  const [gems, setGems] = useState<GemInfo[]>([]);
  const [tip, setTip] = useState<TooltipLine[] | null>(null);
  const [paste, setPaste] = useState("");

  useEffect(() => {
    engine.call<Skills>("skills").then(setSkills);
  }, [rev]);
  useEffect(() => {
    gemListCache ??= engine.call<GemInfo[]>("gemList");
    gemListCache.then(setGems);
  }, []);
  useEffect(() => {
    if (!skills?.groups[sel - 1]) return setTip(null);
    engine.call<{ lines: TooltipLine[] }>("socketGroupTooltip", { index: sel }).then((t) => setTip(t.lines));
  }, [skills, sel]);

  const gemNames = useMemo(() => gems.map((g) => g.name), [gems]);

  if (!skills) return <p>Loading...</p>;
  const group = skills.groups[sel - 1];
  const setGem = (gem: number, change: Partial<Gem>) => act("setGem", { group: sel, gem, ...change });

  return (
    <div class="skills-tab">
      <div class="group-list">
        <div class="toolbar">
          {skills.skillSets.length > 1 ? (
            <select value={String(skills.activeSkillSet)} onChange={(e) => act("selectSkillSet", { id: Number((e.target as HTMLSelectElement).value) })}>
              {skills.skillSets.map((s) => (
                <option key={s.id} value={String(s.id)}>
                  {s.title}
                </option>
              ))}
            </select>
          ) : null}
          <button
            onClick={async () => {
              const r = await act<{ index: number }>("addSocketGroup");
              if (r) setSel(r.index);
            }}
          >
            New group
          </button>
        </div>
        <ul>
          {skills.groups.map((g, i) => (
            <li key={i} class={(i + 1 === sel ? "selected " : "") + (g.enabled ? "" : "disabled")} onClick={() => setSel(i + 1)}>
              <input
                type="radio"
                name="main"
                title="Main skill"
                checked={skills.mainSocketGroup === i + 1}
                onClick={(e) => {
                  e.stopPropagation();
                  act("setMainSocketGroup", { index: i + 1 });
                }}
              />
              <ColorText text={g.displayLabel || g.label || "<No active skills>"} />
              {g.slot ? <span class="slot"> ({g.slot})</span> : null}
              {g.includeInFullDPS ? <span class="fulldps" title="Included in Full DPS"> FD</span> : null}
            </li>
          ))}
        </ul>
        <h4>Paste a socket group</h4>
        <textarea rows={4} value={paste} onInput={(e) => setPaste((e.target as HTMLTextAreaElement).value)} placeholder="Label: ...&#10;Fireball 20/0 Default  1" />
        <button
          onClick={async () => {
            await act("pasteSocketGroup", { text: paste });
            setPaste("");
          }}
        >
          Paste
        </button>
      </div>
      {group ? (
        <div class="group-editor">
          <div class="row">
            <label>
              Label{" "}
              <input type="text" value={group.label ?? ""} onChange={(e) => act("setSocketGroup", { index: sel, label: (e.target as HTMLInputElement).value })} />
            </label>
            <label>
              Socketed in{" "}
              <select value={group.slot ?? ""} onChange={(e) => act("setSocketGroup", { index: sel, slot: (e.target as HTMLSelectElement).value })}>
                <option value="">None</option>
                {skills.slots.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <input type="checkbox" checked={group.enabled} onChange={(e) => act("setSocketGroup", { index: sel, enabled: (e.target as HTMLInputElement).checked })} /> Enabled
            </label>
            <label>
              <input
                type="checkbox"
                checked={group.includeInFullDPS}
                onChange={(e) => act("setSocketGroup", { index: sel, includeInFullDPS: (e.target as HTMLInputElement).checked })}
              />{" "}
              Include in Full DPS
            </label>
            <button
              onClick={async () => {
                const r = await engine.call<{ text: string }>("copySocketGroup", { index: sel });
                await navigator.clipboard?.writeText(r.text);
              }}
            >
              Copy
            </button>
            <button
              onClick={async () => {
                await act("removeSocketGroup", { index: sel });
                setSel(Math.max(1, sel - 1));
              }}
            >
              Delete group
            </button>
          </div>
          {group.source ? (
            <p class="source">
              Granted by <ColorText text={group.source} />
            </p>
          ) : null}
          <datalist id="gem-names">
            {gemNames.map((n) => (
              <option key={n} value={n} />
            ))}
          </datalist>
          <table class="gems">
            <thead>
              <tr>
                <th>Gem</th>
                <th>Level</th>
                <th>Quality</th>
                <th>Enabled</th>
                <th>Count</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {group.gems.map((g, i) => (
                <tr key={`${sel}.${i}.${g.name}`}>
                  <td>
                    <input
                      type="text"
                      list="gem-names"
                      value={g.name}
                      style={g.color ? { color: parseColor(g.color) } : undefined}
                      onChange={(e) => setGem(i + 1, { name: (e.target as HTMLInputElement).value })}
                    />
                    {g.error ? <div class="error">{stripColors(g.error)}</div> : null}
                  </td>
                  <td>
                    <input type="number" min={1} max={40} value={g.level} onChange={(e) => setGem(i + 1, { level: Number((e.target as HTMLInputElement).value) })} />
                  </td>
                  <td>
                    <input type="number" min={0} max={23} value={g.quality} onChange={(e) => setGem(i + 1, { quality: Number((e.target as HTMLInputElement).value) })} />
                  </td>
                  <td>
                    <input type="checkbox" checked={g.enabled} onChange={(e) => setGem(i + 1, { enabled: (e.target as HTMLInputElement).checked })} />
                  </td>
                  <td>
                    <input type="number" min={1} value={g.count ?? 1} onChange={(e) => setGem(i + 1, { count: Number((e.target as HTMLInputElement).value) })} />
                  </td>
                  <td>
                    <button class="small" onClick={() => act("removeGem", { group: sel, gem: i + 1 })}>
                      remove
                    </button>
                  </td>
                </tr>
              ))}
              <tr>
                <td colSpan={6}>
                  <input
                    type="text"
                    list="gem-names"
                    placeholder="Add a gem"
                    onChange={(e) => {
                      const v = (e.target as HTMLInputElement).value;
                      (e.target as HTMLInputElement).value = "";
                      if (v.trim()) setGem(group.gems.length + 1, { name: v });
                    }}
                  />
                </td>
              </tr>
            </tbody>
          </table>
          {tip?.length ? <TooltipView lines={tip} /> : null}
        </div>
      ) : (
        <p>No socket groups. Add one, or import a build.</p>
      )}
    </div>
  );
}

function parseColor(code: string): string | undefined {
  if (code.startsWith("^x")) return "#" + code.slice(2);
  return undefined;
}
