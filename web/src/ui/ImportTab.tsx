// Import and export: build codes (as upstream's import tab reads and writes
// them), PoB XML files, links with the code in the URL, and builds saved in
// this browser.

import { useEffect, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { Engine } from "../engine/client";

const STORE = "pob-builds";

type Saved = { name: string; code: string; saved: number };

function readSaved(): Saved[] {
  try {
    return JSON.parse(localStorage.getItem(STORE) ?? "[]");
  } catch {
    return [];
  }
}

function writeSaved(list: Saved[]) {
  try {
    localStorage.setItem(STORE, JSON.stringify(list));
  } catch {
    // storage unavailable
  }
}

// `#build=<code>` in the URL loads that build
export async function loadFromHash(e: Engine): Promise<boolean> {
  const m = location.hash.match(/^#build=(.+)$/);
  if (!m) return false;
  await e.call("loadCode", { code: decodeURIComponent(m[1]) });
  return true;
}

function looksLikeXml(text: string) {
  return text.trimStart().startsWith("<");
}

export function ImportTab({ act, state }: TabProps) {
  const [input, setInput] = useState("");
  const [code, setCode] = useState("");
  const [saved, setSaved] = useState<Saved[]>(readSaved);
  const [name, setName] = useState(state.name);

  useEffect(() => setName(state.name), [state.name]);

  const load = async (text: string, buildName?: string) => {
    const t = text.trim();
    if (!t) return;
    if (looksLikeXml(t)) await act("loadXml", { xml: t, name: buildName });
    else await act("loadCode", { code: t, name: buildName });
  };

  const exportCode = async () => {
    const r = await engine.call<{ code: string }>("saveCode");
    setCode(r.code);
    return r.code;
  };

  const downloadXml = async () => {
    const { xml } = await engine.call<{ xml: string }>("saveXml");
    const a = document.createElement("a");
    a.href = URL.createObjectURL(new Blob([xml], { type: "application/xml" }));
    a.download = `${name || "build"}.xml`;
    a.click();
    URL.revokeObjectURL(a.href);
  };

  const saveHere = async () => {
    const c = await exportCode();
    const list = [{ name: name || "Unnamed build", code: c, saved: Date.now() }, ...saved.filter((s) => s.name !== name)];
    setSaved(list);
    writeSaved(list);
  };

  return (
    <div class="import-tab">
      <section>
        <h3>Import</h3>
        <p>Paste a build code or the XML of a build.</p>
        <textarea rows={6} value={input} onInput={(e) => setInput((e.target as HTMLTextAreaElement).value)} />
        <div class="buttons">
          <button onClick={() => load(input)}>Import</button>
          <label class="file">
            Open .xml file
            <input
              type="file"
              accept=".xml,text/xml"
              onChange={async (e) => {
                const f = (e.target as HTMLInputElement).files?.[0];
                if (f) await load(await f.text(), f.name.replace(/\.xml$/i, ""));
              }}
            />
          </label>
          <button onClick={() => act("newBuild")}>New build</button>
        </div>
      </section>
      <section>
        <h3>Export</h3>
        <div class="buttons">
          <button onClick={exportCode}>Generate build code</button>
          <button
            onClick={async () => {
              const c = code || (await exportCode());
              await navigator.clipboard?.writeText(c);
            }}
          >
            Copy code
          </button>
          <button
            onClick={async () => {
              const c = code || (await exportCode());
              const url = `${location.origin}${location.pathname}#build=${encodeURIComponent(c)}`;
              history.replaceState(null, "", url);
              await navigator.clipboard?.writeText(url);
            }}
          >
            Copy link
          </button>
          <button onClick={downloadXml}>Download .xml</button>
        </div>
        {code ? <textarea rows={4} readOnly value={code} /> : null}
      </section>
      <section>
        <h3>Builds saved in this browser</h3>
        <div class="buttons">
          <input type="text" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
          <button onClick={saveHere}>Save current build</button>
        </div>
        <ul class="saved">
          {saved.map((s) => (
            <li key={s.name}>
              <button onClick={() => load(s.code, s.name)}>{s.name}</button>
              <span class="date">{new Date(s.saved).toLocaleString()}</span>
              <button
                class="small"
                onClick={() => {
                  const list = saved.filter((x) => x !== s);
                  setSaved(list);
                  writeSaved(list);
                }}
              >
                delete
              </button>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
