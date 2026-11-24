// The engine client's transports, under Node with stand-ins for the
// browser: a page with `<meta name="pob-host">` talks to the host over
// its WebSocket, any other page to its wasm workers. On the host, answers
// match their requests, and the client hears of changes it didn't make.
import { test } from "node:test";
import assert from "node:assert/strict";

class FakeWorker {
  static made = 0;
  constructor() {
    FakeWorker.made++;
  }
  postMessage() {}
}

class FakeSocket {
  static last = null;
  constructor(url) {
    this.url = url;
    this.sent = [];
    FakeSocket.last = this;
    queueMicrotask(() => this.onopen?.());
  }
  send(text) {
    this.sent.push(JSON.parse(text));
  }
  // A message from the host
  push(m) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
}

function page(hosted) {
  globalThis.document = {
    baseURI: "http://127.0.0.1:8730/",
    querySelector: (sel) => (hosted && sel === 'meta[name="pob-host"]' ? {} : null),
  };
  globalThis.location = { protocol: "http:", host: "127.0.0.1:8730" };
  globalThis.Worker = FakeWorker;
  globalThis.WebSocket = FakeSocket;
}

const { Engine, EngineError } = await import("../src/engine/client.ts");

const tick = () => new Promise((r) => setTimeout(r, 0));

test("a page of its own uses the workers", () => {
  page(false);
  FakeWorker.made = 0;
  FakeSocket.last = null;
  const e = new Engine();
  assert.equal(e.hosted, null);
  assert.equal(FakeWorker.made, 2);
  assert.equal(FakeSocket.last, null);
});

test("a page the host served uses its WebSocket", async () => {
  page(true);
  FakeWorker.made = 0;
  const e = new Engine();
  assert.ok(e.hosted);
  assert.equal(FakeWorker.made, 0);
  const ws = FakeSocket.last;
  assert.equal(ws.url, "ws://127.0.0.1:8730/ws");
  ws.push({ event: "hello", build: "RF", version: 0, history: true });
  assert.equal(e.hosted.build, "RF");
  assert.equal(e.hosted.history, true);

  const events = [];
  e.hosted.onEvent((ev) => events.push(ev));

  // A command's own `id` stays the command's
  const click = e.call("clickNode", { id: 36634 });
  await tick();
  assert.deepEqual(ws.sent[0], { id: 1, req: { cmd: "clickNode", id: 36634 } });
  ws.push({ id: 1, ok: { done: true }, version: 1 });
  assert.deepEqual(await click, { done: true });
  // Its change event is the client's own: not passed on
  ws.push({ event: "change", build: "RF", version: 1, label: "allocate", source: "user" });
  assert.equal(events.length, 0);
  // An agent's is
  ws.push({ event: "change", build: "RF", version: 2, label: "level 90", source: "agent claude" });
  assert.equal(events.length, 1);
  assert.equal(events[0].source, "agent claude");
  assert.equal(e.hosted.version, 2);

  // Errors are the engine's
  const bad = e.call("nope");
  await tick();
  ws.push({ id: 2, error: "not ported: nope" });
  await assert.rejects(bad, (err) => err instanceof EngineError && err.message === "not ported: nope");
  assert.equal(e.ported("nope"), false);

  // The host's own requests, and the background goes to the host too
  const b = e.hosted.host("builds");
  const p = e.background_("nodePower", { index: 1 });
  await tick();
  assert.deepEqual(ws.sent[2], { id: 3, host: "builds", args: {} });
  assert.deepEqual(ws.sent[3], { id: 4, req: { cmd: "nodePower", index: 1 } });
  ws.push({ id: 4, ok: [1] });
  ws.push({ id: 3, ok: { builds: ["RF"] } });
  assert.deepEqual(await b, { builds: ["RF"] });
  assert.deepEqual(await p, [1]);

  // A closed connection fails what waits, and what comes after
  const waiting = e.call("state");
  await tick();
  ws.onclose();
  await assert.rejects(waiting, EngineError);
  await assert.rejects(e.call("state"), EngineError);
});
