// On the host only: the agents connected now, each closed or its token
// revoked at once, and the agent directories with their builds: open one
// to look at it, keep one (it moves, with its history, among your
// builds), or clean a directory up (it moves to the builds' trash/, which
// only you empty). See docs/agent.md, "Agent directories".

import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";

type Connection = {
  id: number;
  kind: string;
  connection: string;
  client?: string | null;
  agent_directory: string;
  builds: string[];
  token?: string | null;
  since: number;
  calls: number;
};
type Dir = { dir: string; size: number; builds: { name: string; build: string; size: number }[] };

function size(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1 << 20) return `${(n / 1024).toFixed(0)} kB`;
  return `${(n / (1 << 20)).toFixed(1)} MB`;
}

export function AgentsTab({ act, rev }: TabProps) {
  const host = engine.hosted!;
  const [connections, setConnections] = useState<Connection[]>([]);
  const [dirs, setDirs] = useState<Dir[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [tick, setTick] = useState(0);

  // Connections come and go without changing the build: looked at again
  // every few seconds
  useEffect(() => {
    const t = setInterval(() => setTick((x) => x + 1), 5000);
    return () => clearInterval(t);
  }, []);
  useEffect(() => {
    host.host<{ connections: Connection[] }>("connections").then((r) => setConnections(r.connections), () => undefined);
    host.host<{ dirs: Dir[] }>("agents").then((r) => setDirs(r.dirs), () => undefined);
  }, [rev, tick]);

  const run = async (what: string, args: Record<string, unknown>, done: (r: any) => string) => {
    try {
      const r = await host.host(what, args);
      setNote(done(r));
      await act("state");
      setTick((x) => x + 1);
    } catch (e) {
      setNote(String(e instanceof Error ? e.message : e));
    }
  };

  return (
    <div class="history-tab agents-tab">
      {note ? <p class="muted">{note}</p> : null}
      <section>
        <h3>Connected agents</h3>
        {connections.length ? (
          <table class="versions">
            <tbody>
              {connections.map((c) => (
                <tr key={c.id}>
                  <td>
                    <span class="source source-agent">{c.connection}</span>
                  </td>
                  <td class="label">
                    {c.client ?? c.kind}
                    <span class="muted">
                      {" "}
                      · {c.kind === "http" ? "HTTP, token " + c.token : "this machine"} · builds: {c.builds.length ? c.builds.join(", ") : "none"} · own
                      directory {c.agent_directory} · {c.calls} calls since {new Date(c.since * 1000).toLocaleTimeString()}
                    </span>
                  </td>
                  <td>
                    <button onClick={() => run("disconnect", { id: c.id }, () => `${c.connection} disconnected.`)}>Disconnect</button>
                  </td>
                  <td>
                    {c.token ? (
                      <button
                        class="danger"
                        onClick={() => run("revoke", { token: c.token }, (r) => `Token ${c.token} revoked; ${r.closed} connection(s) closed.`)}
                      >
                        Revoke token
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p class="muted">
            No agent is connected. <code>pob agents install</code> adds PoB to Claude Code or Codex on this machine; <code>pob agents token</code> makes a
            token for MCP over HTTP.
          </p>
        )}
      </section>
      <section>
        <h3>Agent directories</h3>
        <p class="muted">Builds the agents made for themselves. Keep one to make it yours; clean up what you don't need.</p>
        {dirs.length ? (
          <>
            {dirs.map((d) => (
              <div key={d.dir} class="agent-dir">
                <div class="toolbar">
                  <strong>{d.dir}</strong>
                  <span class="muted">{size(d.size)}</span>
                  <span class="spacer" />
                  <button onClick={() => confirm(`Move ${d.dir} to the trash?`) && run("clean", { dir: d.dir }, () => `${d.dir} moved to the trash.`)}>
                    Clean up
                  </button>
                </div>
                <table class="versions">
                  <tbody>
                    {d.builds.map((b) => (
                      <tr key={b.build}>
                        <td class="label">{b.name}</td>
                        <td class="muted">{size(b.size)}</td>
                        <td>
                          <button onClick={() => run("open", { build: b.build }, () => `Showing ${b.build}.`)}>Open</button>
                        </td>
                        <td>
                          <button onClick={() => run("keep", { build: b.build }, (r) => `Kept as ${r.build}.`)}>Keep</button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ))}
            <div class="buttons">
              <button class="danger" onClick={() => confirm("Move every agent directory to the trash?") && run("clean", {}, () => "Every agent directory moved to the trash.")}>
                Clean up all
              </button>
            </div>
          </>
        ) : (
          <p class="muted">No agent directories.</p>
        )}
      </section>
    </div>
  );
}
