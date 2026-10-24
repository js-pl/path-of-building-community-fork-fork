import { useCallback, useEffect, useState } from "preact/hooks";
import { Engine } from "./engine/client";
import type { State } from "./engine/types";
import { Sidebar } from "./ui/Sidebar";
import { TreeTab } from "./ui/TreeTab";
import { CalcsTab } from "./ui/CalcsTab";
import { SkillsTab } from "./ui/SkillsTab";
import { ItemsTab } from "./ui/ItemsTab";
import { ConfigTab } from "./ui/ConfigTab";
import { NotesTab } from "./ui/NotesTab";
import { PartyTab } from "./ui/PartyTab";
import { ImportTab, loadFromHash } from "./ui/ImportTab";
import { PopupView } from "./ui/Popup";
import type { SetControl } from "./ui/Control";

export const engine = new Engine();

// What every tab gets: the engine, an edit function that refreshes the
// build state afterwards, and a revision that changes after every edit
export type TabProps = {
  act: <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T | undefined>;
  setControl: SetControl;
  rev: number;
  state: State;
};

const tabs = ["Import/Export", "Tree", "Skills", "Items", "Calcs", "Config", "Notes", "Party"] as const;
type Tab = (typeof tabs)[number];

export function App() {
  const [state, setState] = useState<State | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rev, setRev] = useState(0);
  const [tab, setTab] = useState<Tab>("Tree");
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    const s = await engine.call<State>("state");
    setState(s);
    setRev((r) => r + 1);
  }, []);

  const act = useCallback(
    async <T,>(cmd: string, args: Record<string, unknown> = {}) => {
      setBusy(true);
      try {
        const r = await engine.call<T>(cmd, args);
        await refresh();
        return r;
      } catch (e) {
        setError(String(e instanceof Error ? e.message : e));
        return undefined;
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const setControl: SetControl = useCallback((path, change) => void act("control", { path, ...change }), [act]);

  useEffect(() => {
    (async () => {
      try {
        if (!(await loadFromHash(engine))) await engine.call("state");
        await refresh();
      } catch (e) {
        setError(String(e instanceof Error ? e.message : e));
      }
    })();
  }, [refresh]);

  if (!state) {
    return (
      <div class="loading">
        {error ? <pre class="error">{error}</pre> : <p>Loading the engine and game data...</p>}
      </div>
    );
  }
  const props: TabProps = { act, setControl, rev, state };
  return (
    <div class="app">
      <header class="topbar">
        <span class="title">{state.name}</span>
        <nav>
          {tabs.map((t) => (
            <button key={t} class={t === tab ? "active" : ""} onClick={() => setTab(t)}>
              {t}
            </button>
          ))}
        </nav>
        {busy ? <span class="busy">calculating...</span> : null}
      </header>
      <aside class="sidebar">
        <Sidebar {...props} />
      </aside>
      <main class="content">
        {error ? (
          <div class="error-bar" onClick={() => setError(null)}>
            {error} <span class="dismiss">(dismiss)</span>
          </div>
        ) : null}
        {tab === "Import/Export" ? <ImportTab {...props} /> : null}
        {tab === "Tree" ? <TreeTab {...props} /> : null}
        {tab === "Skills" ? <SkillsTab {...props} /> : null}
        {tab === "Items" ? <ItemsTab {...props} /> : null}
        {tab === "Calcs" ? <CalcsTab {...props} /> : null}
        {tab === "Config" ? <ConfigTab {...props} /> : null}
        {tab === "Notes" ? <NotesTab {...props} /> : null}
        {tab === "Party" ? <PartyTab {...props} /> : null}
      </main>
      {state.popups.length ? <PopupView popup={state.popups[0]} act={act} /> : null}
    </div>
  );
}
