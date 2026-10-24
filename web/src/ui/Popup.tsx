import type { Popup } from "../engine/types";
import type { TabProps } from "../app";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";

export function PopupView({ popup, act }: { popup: Popup; act: TabProps["act"] }) {
  return (
    <div class="modal-back">
      <div class="modal">
        <h3>
          <ColorText text={popup.title ?? ""} />
        </h3>
        {popup.lines.map((l, i) => (
          <p key={i}>
            <ColorText text={l} />
          </p>
        ))}
        {popup.controls.map((c) => (
          <ControlView key={c.path} path={c.path!} c={c} set={(path, change) => act("control", { path, ...change })} />
        ))}
        <div class="buttons">
          {popup.buttons.map((b) => (
            <button key={b.name} disabled={!b.enabled} onClick={() => act("popupButton", { name: b.name })}>
              <ColorText text={b.label} />
            </button>
          ))}
          {popup.buttons.length === 0 ? <button onClick={() => act("closePopup")}>Close</button> : null}
        </div>
      </div>
    </div>
  );
}
