// The browser engine (public/engine.wasm with public/data, see
// scripts/build-wasm.mjs and scripts/build-data.mjs) run under Node: it
// starts, loads a corpus build, and gives the same sidebar as the native
// engine (`pob serve`). Built without the translation (--typed), commands
// the typed engine does not have are "not ported" errors.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
const pub = join(web, "public");

let translation;

async function engine() {
  const { instance } = await WebAssembly.instantiate(readFileSync(join(pub, "engine.wasm")), {});
  const ex = instance.exports;
  translation = ex.has_translation() === 1;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const put = (bytes) => {
    const p = ex.alloc(bytes.length) >>> 0;
    new Uint8Array(ex.memory.buffer, p, bytes.length).set(bytes);
    return [p, bytes.length];
  };
  const result = () => JSON.parse(dec.decode(new Uint8Array(ex.memory.buffer, ex.result_ptr() >>> 0, ex.result_len() >>> 0)));
  const addMissing = (missing) => {
    let added = false;
    for (const path of missing ?? []) {
      const file = join(pub, "data/files", path);
      if (!existsSync(file)) continue;
      const [p, pl] = put(enc.encode(path));
      const [d, dl] = put(readFileSync(file));
      ex.add_file(p, pl, d, dl);
      added = true;
    }
    return added;
  };
  ex.add_bundle(...put(readFileSync(join(pub, "data/core.bin"))));
  assert.ok(result().ok > 0);
  for (;;) {
    ex.start();
    const r = result();
    if (!r.error) break;
    if (!addMissing(r.missing)) throw new Error(r.error);
  }
  return (req) => {
    for (;;) {
      ex.request(...put(enc.encode(JSON.stringify(req))));
      const r = result();
      if (!r.error || !addMissing(r.missing)) return r;
    }
  };
}

function native(reqs) {
  const out = execFileSync(join(root, "target/release/pob"), ["serve"], { input: reqs.map((r) => JSON.stringify(r)).join("\n") + "\n", maxBuffer: 1 << 28 });
  return out.toString().trim().split("\n").map((l) => JSON.parse(l));
}

for (const rel of ["corpus/generated/gen-0001.xml", "upstream/PathOfBuilding/spec/TestBuilds/3.13/OccVortex.xml"]) {
  test(`wasm engine matches the native engine on ${rel}`, async () => {
    const call = await engine();
    const xml = readFileSync(join(root, rel), "utf8");
    const reqs = [{ cmd: "loadXml", xml }, { cmd: "state" }, { cmd: "saveCode" }];
    const got = reqs.map(call);
    // A command the typed engine does not have goes to the translation,
    // or is "not ported" without it
    const other = call({ cmd: "noSuchCommand" });
    if (translation) assert.equal(other.error, "unknown command noSuchCommand");
    else assert.equal(other.error, "not ported: noSuchCommand");
    for (const r of got) assert.equal(r.error, undefined, r.error);
    const want = native(reqs);
    assert.deepEqual(got[1].ok.stats, want[1].ok.stats);
    assert.equal(got[2].ok.code, want[2].ok.code);
  });
}
