// On the host only: the build's undo list, each step with who made it
// (you, or which agent), and its history, the build's git log, with a
// restore per version (a restore is a new version, so it can be undone
// too). See docs/agent.md, "Live sync" and "Build history".

import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";

type Steps = {
  version: number;
  // What `undo` takes back, newest first
  steps: { label: string; source: string }[];
  // Every change since the host opened the build, newest first
  changes: { version: number; label: string; source: string; commit?: string | null }[];
};
type Version = { version: string; time: number; label: string; source: string; trailers: [string, string][] };
type History = { enabled: boolean; pending: boolean; versions: Version[] };

// `user` is you; `agent claude-code` is that agent's connection
export function sourceLabel(source: string): string {
  if (source === "user") return "you";
  if (source.startsWith("agent ")) return source.slice(6);
  return source;
}

function SourceBadge({ source }: { source: string }) {
  const kind = source === "user" ? "user" : source.startsWith("agent ") ? "agent" : "other";
  return <span class={`source source-${kind}`}>{sourceLabel(source)}</span>;
}

function when(t: number): string {
  return new Date(t * 1000).toLocaleString();
}

export function HistoryTab({ act, rev }: TabProps) {
  const host = engine.hosted!;
  const [steps, setSteps] = useState<Steps | null>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    host.host<Steps>("steps").then(setSteps, (e) => setError(String(e.message ?? e)));
    host.host<History>("history", { n: 100 }).then(setHistory, (e) => setError(String(e.message ?? e)));
  }, [rev]);

  const restore = async (v: Version) => {
    if (!confirm(`Restore the build as it was at "${v.label}"? This adds a new version; nothing is lost.`)) return;
    try {
      const r = await host.host<{ unchanged?: boolean }>("restore", { version: v.version });
      setError(r.unchanged ? "That version is the build as it is now: nothing changed." : null);
      await act("state");
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  return (
    <div class="history-tab">
      {error ? <p class="error">{error}</p> : null}
      <section>
        <h3>Changes</h3>
        <p class="muted">Every change since the host opened the build, newest first, and who made it.</p>
        {steps && steps.changes.length ? (
          <ol class="steps">
            {steps.changes.map((c) => (
              <li key={c.version}>
                <SourceBadge source={c.source} />
                <span class="label">{c.label}</span>
                <span class="muted">v{c.version}</span>
              </li>
            ))}
          </ol>
        ) : (
          <p class="muted">No changes yet.</p>
        )}
        <div class="buttons">
          {steps && steps.steps.length ? (
            <button onClick={() => act("undo")}>
              Undo “{steps.steps[0].label}” <SourceBadge source={steps.steps[0].source} />
            </button>
          ) : (
            <span class="muted">Nothing to undo since the build was opened; restore a version below.</span>
          )}
        </div>
      </section>
      <section>
        <h3>History</h3>
        {history && !history.enabled ? (
          <p class="muted">The history is off: versions aren't kept. Turn it on in Options.</p>
        ) : (
          <>
            <p class="muted">
              Every version of the build, kept in its git repository. Your edits are committed together after 10 seconds without
              another, and before an agent's step.{history?.pending ? " Your latest edits wait to be committed." : ""}
            </p>
            <table class="versions">
              <tbody>
                {(history?.versions ?? []).map((v, i) => (
                  <tr key={v.version}>
                    <td class="muted">{when(v.time)}</td>
                    <td>
                      <SourceBadge source={v.source} />
                    </td>
                    <td class="label">
                      {v.label}
                      {v.trailers.length ? <span class="muted"> · {v.trailers.map(([k, x]) => `${k} ${x}`).join(", ")}</span> : null}
                    </td>
                    <td>
                      <code>{v.version.slice(0, 7)}</code>
                    </td>
                    <td>{i === 0 && !history?.pending ? <span class="muted">current</span> : <button onClick={() => restore(v)}>Restore</button>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </section>
    </div>
  );
}
