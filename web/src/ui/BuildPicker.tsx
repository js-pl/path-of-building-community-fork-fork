// On the host only: the build this page shows, among the user's builds.
// Picking one shows it here; the link opens the build in a new page, so
// several builds can be open side by side. The host keeps each one's
// session (docs/agent.md, "Several builds").

import { useEffect, useState } from "preact/hooks";
import { engine } from "../app";

type Builds = { builds: string[]; current: string };

export function BuildPicker({ rev, onError }: { rev: number; onError: (e: string) => void }) {
  const host = engine.hosted!;
  const [builds, setBuilds] = useState<Builds | null>(null);
  useEffect(() => {
    host.host<Builds>("builds").then(setBuilds, () => undefined);
  }, [rev]);
  const current = builds?.current ?? host.build ?? "";
  const names = builds?.builds ?? [];
  // A build of an agent directory isn't among the user's builds
  const options = names.includes(current) || !current ? names : [current, ...names];
  const pick = async (build: string) => {
    try {
      await host.host("open", { build });
    } catch (e) {
      onError(String(e instanceof Error ? e.message : e));
    }
  };
  return (
    <span class="build-picker">
      <select aria-label="Build" value={current} title={current} onChange={(e) => void pick((e.target as HTMLSelectElement).value)}>
        {options.map((b) => (
          <option key={b} value={b}>
            {b}
          </option>
        ))}
      </select>
      <a class="ghost" href={`?build=${encodeURIComponent(current)}`} target="_blank" rel="noopener" title="Open this build in a new page">
        ↗
      </a>
    </span>
  );
}
