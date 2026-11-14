// Party tab: upstream's own controls for party members' auras, curses,
// warcries, links and enemy conditions, and importing them from a build code
import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { Control } from "../engine/types";
import { ControlView } from "./Control";
import { isNotPorted } from "../engine/client";
import { NotPorted } from "./NotPorted";

export function PartyTab({ rev, setControl }: TabProps) {
  const [controls, setControls] = useState<Control[] | null>(null);
  const [unported, setUnported] = useState(false);
  useEffect(() => {
    engine
      .call<{ controls: Control[] }>("party")
      .then((p) => setControls(p.controls))
      .catch((e) => setUnported(isNotPorted(e)));
  }, [rev]);
  if (unported) return <NotPorted what="the party tab" />;
  if (!controls) return <p>Loading...</p>;
  return (
    <div class="party-tab">
      {controls.map((c) => (
        <ControlView key={c.path} path={c.path!} c={c} set={setControl} />
      ))}
    </div>
  );
}
