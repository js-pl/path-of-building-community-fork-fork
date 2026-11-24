// The Options dialog, laid out like upstream's Options popup
// (main:OpenOptionsPopup): sections under a header, each option a label
// right-aligned before its control, Save and Cancel below. Its one section
// so far is the build history's git settings (crates/settings), shown
// only when the app is served by the host, which keeps the history.

import { useEffect, useState } from "preact/hooks";
import { engine } from "../app";

type HistorySettings = {
  file: string;
  enabled: boolean;
  git: string;
  gitConfig: string;
  // What git the settings give: its path and version, "off", or why none
  found: { ok?: string; error?: string };
  // Whether the running host keeps a history
  active: boolean;
};

export function OptionsDialog({ onClose }: { onClose: () => void }) {
  const host = engine.hosted;
  const [saved, setSaved] = useState<HistorySettings | null>(null);
  const [enabled, setEnabled] = useState(true);
  const [git, setGit] = useState("");
  const [gitConfig, setGitConfig] = useState("");
  const [error, setError] = useState<string | null>(null);

  const show = (s: HistorySettings) => {
    setSaved(s);
    setEnabled(s.enabled);
    setGit(s.git);
    setGitConfig(s.gitConfig);
  };
  useEffect(() => {
    host?.host<HistorySettings>("settings").then(show, (e) => setError(String(e.message ?? e)));
  }, []);
  useEffect(() => {
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    addEventListener("keydown", key);
    return () => removeEventListener("keydown", key);
  }, [onClose]);

  const save = async () => {
    try {
      show(await host!.host<HistorySettings>("setSettings", { enabled, git, gitConfig }));
      setError(null);
      onClose();
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  const changed = saved && (saved.enabled !== enabled || saved.git !== git || saved.gitConfig !== gitConfig);
  return (
    <div class="modal-back" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div class="modal options" role="dialog" aria-label="Options">
        <h3>Options</h3>
        {host ? (
          <section>
            <div class="options-section">Build history</div>
            <label class="option-row">
              <span>Keep a history:</span>
              <input type="checkbox" checked={enabled} onChange={(e) => setEnabled((e.target as HTMLInputElement).checked)} />
            </label>
            <label class="option-row">
              <span>Git program:</span>
              <input type="text" value={git} placeholder="found on the PATH" onInput={(e) => setGit((e.target as HTMLInputElement).value)} />
            </label>
            <label class="option-row">
              <span>Git config file:</span>
              <input
                type="text"
                value={gitConfig}
                placeholder="your global gitconfig"
                onInput={(e) => setGitConfig((e.target as HTMLInputElement).value)}
              />
            </label>
            {saved ? (
              <p class="muted option-note">
                {saved.found.error ? <span class="error">Git can't be used: {saved.found.error}. </span> : <>Git: {saved.found.ok}. </>}
                {saved.active ? "The host keeps a history now." : "The host keeps no history now."} Changes take effect when the host
                starts again. Saved in <code>{saved.file}</code>.
              </p>
            ) : null}
          </section>
        ) : (
          <p class="muted">No options here yet.</p>
        )}
        {error ? <p class="error">{error}</p> : null}
        <div class="buttons options-buttons">
          {host ? (
            <button onClick={save} disabled={!changed}>
              Save
            </button>
          ) : null}
          <button onClick={onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
