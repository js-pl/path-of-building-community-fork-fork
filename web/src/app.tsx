import { useCallback, useEffect, useState } from "preact/hooks";
import { Engine, isNotPorted } from "./engine/client";
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
import { HistoryTab } from "./ui/HistoryTab";
import { AgentsTab } from "./ui/AgentsTab";
import { OptionsDialog } from "./ui/OptionsDialog";
import { BuildPicker } from "./ui/BuildPicker";
import { reportContext } from "./ui/context";
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

const baseTabs = ["Import/Export", "Tree", "Skills", "Items", "Calcs", "Config", "Notes", "Party"] as const;
// On the host, the build's undo list and history too
const hostTabs = ["History", "Agents"] as const;
type Tab = (typeof baseTabs)[number] | (typeof hostTabs)[number];
const tabs: readonly Tab[] = engine.hosted ? [...baseTabs, ...hostTabs] : baseTabs;

export function App() {
  const [state, setState] = useState<State | null>(null);
  const [error, setError] = useState<string | null>(null);
  // An edit the engine does not have yet: a notice, not an error
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 6000);
    return () => clearTimeout(t);
  }, [notice]);
  const [rev, setRev] = useState(0);
  const [tab, setTab] = useState<Tab>("Tree");
  // Reported before the new tab renders, so the tab's own selection
  // follows it
  const pickTab = (t: Tab) => {
    reportContext({ tab: t });
    setTab(t);
  };
  useEffect(() => reportContext({ tab }), []);
  const [options, setOptions] = useState(false);
  const [busy, setBusy] = useState(false);
  // Only a slow edit shows the busy bar, so quick ones never flicker
  const [showBusy, setShowBusy] = useState(false);
  useEffect(() => {
    if (!busy) return setShowBusy(false);
    const t = setTimeout(() => setShowBusy(true), 150);
    return () => clearTimeout(t);
  }, [busy]);

  const refresh = useCallback(async () => {
    const s = await engine.call<State>("state");
    setState(s);
    setRev((r) => r + 1);
  }, []);

  const act = useCallback(
    async <T,>(cmd: string, args: Record<string, unknown> = {}) => {
      setBusy(true);
      setNotice(null);
      try {
        const r = await engine.call<T>(cmd, args);
        await refresh();
        return r;
      } catch (e) {
        if (isNotPorted(e)) {
          setNotice(`Not available in this engine yet: ${cmd}.`);
          // The build stays as it was; show it as it is now
          await refresh().catch(() => undefined);
        } else setError(String(e instanceof Error ? e.message : e));
        return undefined;
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const setControl: SetControl = useCallback((path, change) => void act("control", { path, ...change }), [act]);

  // On the host, an agent's change, or another tab's, shows at once
  useEffect(() => {
    engine.hosted?.onEvent((e) => {
      if (e.event === "change" && e.build !== engine.hosted?.build) return;
      void refresh().catch((err) => setError(String(err instanceof Error ? err.message : err)));
    });
  }, [refresh]);

  useEffect(() => {
    (async () => {
      try {
        // On the host, `?build=<name>` shows that build in this page
        const asked = engine.hosted ? new URLSearchParams(location.search).get("build") : null;
        if (asked && asked !== engine.hosted?.build) {
          await engine.hosted!.host("open", { build: asked }).catch((e) => setError(String(e instanceof Error ? e.message : e)));
        }
        if (await loadFromHash(engine)) {
          // On the host the code became a build of its own; a reload
          // mustn't make another
          if (engine.hosted) history.replaceState(null, "", location.pathname + location.search);
        } else await engine.call("state");
        await refresh();
      } catch (e) {
        setError(String(e instanceof Error ? e.message : e));
      }
    })();
  }, [refresh]);

  if (!state) {
    return (
      <div class="loading">
        {error ? (
          <pre class="error">{error}</pre>
        ) : (
          <div class="loading-card">
            <span class="spinner large" />
            <p>Loading the engine and game data…</p>
          </div>
        )}
      </div>
    );
  }
  const props: TabProps = { act, setControl, rev, state };
  return (
    <div class="app">
      <header class="topbar">
        <span class="title" title={state.name}>
          {engine.hosted ? <BuildPicker rev={rev} onError={setError} /> : state.name}
          {/* On the host every edit is saved at once */}
          {state.unsaved && !engine.hosted ? <span class="unsaved" title="Unsaved changes" /> : null}
        </span>
        <nav class="tabs" role="tablist">
          {tabs.map((t) => (
            <button key={t} role="tab" aria-selected={t === tab} class={t === tab ? "active" : ""} onClick={() => pickTab(t)}>
              {t}
            </button>
          ))}
        </nav>
        {engine.hosted ? (
          <button class="ghost options-button" onClick={() => setOptions(true)}>
            Options
          </button>
        ) : null}
        <div class={"busy-bar" + (showBusy ? " shown" : "")} aria-hidden={!showBusy} />
      </header>
      <aside class="sidebar">
        <Sidebar {...props} />
      </aside>
      <main class="content">
        <div class="toasts">
          {error ? (
            <div class="toast error-toast" role="alert">
              <span>{error}</span>
              <button class="ghost" title="Dismiss" onClick={() => setError(null)}>
                ✕
              </button>
            </div>
          ) : null}
          {notice ? (
            <div class="toast" role="status">
              <span>{notice}</span>
              <button class="ghost" title="Dismiss" onClick={() => setNotice(null)}>
                ✕
              </button>
            </div>
          ) : null}
        </div>
        {tab === "Import/Export" ? <ImportTab {...props} /> : null}
        {tab === "Tree" ? <TreeTab {...props} /> : null}
        {tab === "Skills" ? <SkillsTab {...props} /> : null}
        {tab === "Items" ? <ItemsTab {...props} /> : null}
        {tab === "Calcs" ? <CalcsTab {...props} /> : null}
        {tab === "Config" ? <ConfigTab {...props} /> : null}
        {tab === "Notes" ? <NotesTab {...props} /> : null}
        {tab === "Party" ? <PartyTab {...props} /> : null}
        {tab === "History" ? <HistoryTab {...props} /> : null}
        {tab === "Agents" ? <AgentsTab {...props} /> : null}
      </main>
      {state.popups.length ? <PopupView popup={state.popups[0]} act={act} /> : null}
      {options ? <OptionsDialog onClose={() => setOptions(false)} /> : null}
    </div>
  );
}
