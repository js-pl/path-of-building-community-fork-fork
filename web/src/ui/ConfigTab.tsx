// Config tab: upstream's options, shown when upstream shows them
import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { Config } from "../engine/types";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";

export function ConfigTab({ rev, act, setControl }: TabProps) {
  const [config, setConfig] = useState<Config | null>(null);
  const [search, setSearch] = useState("");
  useEffect(() => {
    engine.call<Config>("config").then(setConfig);
  }, [rev]);
  if (!config) return <p>Loading...</p>;
  const q = search.trim().toLowerCase();
  return (
    <div class="config-tab">
      <div class="toolbar">
        {config.configSets.length > 1 ? (
          <select value={String(config.activeConfigSet)} onChange={(e) => act("selectConfigSet", { id: Number((e.target as HTMLSelectElement).value) })}>
            {config.configSets.map((s) => (
              <option key={s.id} value={String(s.id)}>
                {s.title}
              </option>
            ))}
          </select>
        ) : null}
        <input type="search" placeholder="Search options" value={search} onInput={(e) => setSearch((e.target as HTMLInputElement).value)} />
      </div>
      <div class="config-grid">
        {config.sections.map((sec) => {
          const vars = q ? sec.vars.filter((v) => (v.label ?? "").toLowerCase().includes(q)) : sec.vars;
          if (!vars.length) return null;
          return (
            <section key={sec.label} class="config-section">
              <h3>
                <ColorText text={sec.label} />
              </h3>
              {vars.map((v) => (
                <ControlView key={v.var} path={`config.${v.var}`} c={v} set={setControl} />
              ))}
            </section>
          );
        })}
      </div>
    </div>
  );
}
