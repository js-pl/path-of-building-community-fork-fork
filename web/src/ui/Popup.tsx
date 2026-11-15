import type { Popup } from "../engine/types";
import type { TabProps } from "../app";
import { ColorText } from "./ColorText";
import { ControlView } from "./Control";

export function PopupView({
  popup,
  act,
}: {
  popup: Popup;
  act: TabProps["act"];
}) {
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
          <ControlView
            key={c.path}
            path={c.path!}
            c={c}
            set={(path, change) => act("control", { path, ...change })}
          />
        ))}
        <div class="buttons">
          {popup.buttons.map((b) => (
            <button
              key={b.name}
              disabled={!b.enabled}
              onClick={() => act("popupButton", { name: b.name })}
            >
              <ColorText text={b.label} />
            </button>
          ))}
          {/* Upstream's unnamed buttons (Cancel) and Escape close the popup;
              while it is open the engine takes no other tab's input */}
          {popup.buttons.some((b) => b.name === "close") ? null : (
            <button onClick={() => act("closePopup")}>
              {popup.buttons.length === 0 ? "Close" : "Cancel"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
