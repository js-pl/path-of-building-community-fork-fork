// One engine instance per worker (docs/design.md, "Workers"). Loads
// engine.wasm and the data bundle, then answers requests; data files the
// engine finds missing (older tree versions) are fetched and the request is
// retried.

type Exports = {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  result_ptr(): number;
  result_len(): number;
  add_file(path: number, pathLen: number, data: number, dataLen: number): void;
  add_bundle(ptr: number, len: number): void;
  start(): void;
  request(ptr: number, len: number): void;
};

export type WorkerRequest = { id: number; req: Record<string, unknown> } | { init: string };
export type WorkerResponse = { id: number; res: { ok?: unknown; error?: string } };

// The site root, which the page sends first
let base = "";
const encoder = new TextEncoder();
const decoder = new TextDecoder();
let ex: Exports;

// Pointers are unsigned; wasm returns them as signed 32-bit integers
function put(bytes: Uint8Array): [number, number] {
  const ptr = ex.alloc(bytes.length) >>> 0;
  new Uint8Array(ex.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}

function result(): { ok?: unknown; error?: string; missing?: string[] } {
  const bytes = new Uint8Array(ex.memory.buffer, ex.result_ptr() >>> 0, ex.result_len() >>> 0);
  return JSON.parse(decoder.decode(bytes));
}

async function fetchBytes(path: string): Promise<Uint8Array> {
  const r = await fetch(base + path);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

// Fetches the missing files; false when none of them exist
async function addMissing(missing: string[] | undefined): Promise<boolean> {
  let added = false;
  for (const path of missing ?? []) {
    try {
      const data = await fetchBytes("data/files/" + path);
      const [p, pl] = put(encoder.encode(path));
      const [d, dl] = put(data);
      ex.add_file(p, pl, d, dl);
      ex.dealloc(p, pl);
      ex.dealloc(d, dl);
      added = true;
    } catch {
      // not a file the site serves
    }
  }
  return added;
}

async function init() {
  const [wasm, bundle] = await Promise.all([
    fetch(base + "engine.wasm").then((r) => r.arrayBuffer()),
    fetchBytes("data/core.bin"),
  ]);
  const { instance } = await WebAssembly.instantiate(wasm, {});
  ex = instance.exports as unknown as Exports;
  const [p, l] = put(bundle);
  ex.add_bundle(p, l);
  ex.dealloc(p, l);
  const b = result();
  if (b.error) throw new Error(b.error);
  for (let i = 0; i < 10; i++) {
    ex.start();
    const r = result();
    if (!r.error) return;
    if (!(await addMissing(r.missing))) throw new Error(r.error);
  }
  throw new Error("engine did not start");
}

async function run(req: Record<string, unknown>) {
  const bytes = encoder.encode(JSON.stringify(req));
  for (let i = 0; i < 5; i++) {
    const [p, l] = put(bytes);
    ex.request(p, l);
    ex.dealloc(p, l);
    const r = result();
    if (!r.error || !(await addMissing(r.missing))) {
      delete r.missing;
      return r;
    }
  }
  return { error: "data files are missing" };
}

let ready: Promise<void> | null = null;
let queue = Promise.resolve();

self.onmessage = (e: MessageEvent<WorkerRequest>) => {
  if ("init" in e.data) {
    base = e.data.init;
    ready = init();
    return;
  }
  const { id, req } = e.data;
  queue = queue.then(async () => {
    let res;
    try {
      await ready;
      if (!ready) throw new Error("worker not initialised");
      res = await run(req);
    } catch (err) {
      res = { error: String(err instanceof Error ? err.message : err) };
    }
    (self as unknown as Worker).postMessage({ id, res } satisfies WorkerResponse);
  });
};
