// Engine client: promise-based requests to the engine, over one of two
// transports behind the same `call(cmd, args)`:
//
// - browser-only: a pool of wasm workers, a main worker holding the build
//   being edited and a background worker for slow calculations (node
//   power), which works on a copy of the build so edits never wait for it;
// - served by `pob host` (the page has `<meta name="pob-host">`): a
//   WebSocket to the host, whose native session of the build is the only
//   copy. The host tells every client of each change to the build (the
//   UI's or an agent's) with a change event (docs/agent.md, "Live sync").

import type { WorkerRequest, WorkerResponse } from "./worker";

export class EngineError extends Error {}

// A command the typed engine does not have yet, in an engine built
// without the translation: the answer is "not ported: <cmd>" or names
// what is not ported
export function isNotPorted(e: unknown): boolean {
  return e instanceof EngineError && /\bnot ported\b/.test(e.message);
}

type Res = { ok?: unknown; error?: string };

export interface Transport {
  call<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}

class EngineWorker implements Transport {
  private worker: Worker;
  private next = 1;
  private pending = new Map<number, (r: WorkerResponse["res"]) => void>();

  constructor() {
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.worker.onmessage = (e: MessageEvent<WorkerResponse>) => {
      const cb = this.pending.get(e.data.id);
      this.pending.delete(e.data.id);
      cb?.(e.data.res);
    };
    this.worker.postMessage({ init: new URL(".", document.baseURI).href } satisfies WorkerRequest);
  }

  call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    const id = this.next++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, (res) => {
        if (res.error !== undefined) reject(new EngineError(res.error));
        else resolve(res.ok as T);
      });
      this.worker.postMessage({ id, req: { cmd, ...args } } satisfies WorkerRequest);
    });
  }
}

// A change to the build, from the host: its new version, the undo step's
// label and who made it (`user`, `agent <name>`, a job)
export type Change = { build: string; version: number; label: string; source: string; commit?: string | null };

// What the host says on its own: the build a client shows, a change
export type HostEvent =
  | ({ event: "change" } & Change)
  | { event: "hello"; build?: string; version?: number; history?: boolean; error?: string }
  | { event: "open"; build: string; version: number };

// The WebSocket to the host. Answers carry the request's id; events come
// in the order the host made the changes, after the answer to the
// request that made one
export class HostTransport implements Transport {
  private ws: WebSocket;
  private next = 1;
  private pending = new Map<number, (r: Res & { version?: number }) => void>();
  private opened: Promise<void>;
  private listeners: ((e: HostEvent) => void)[] = [];
  // The build shown and its version as this client knows it
  build: string | null = null;
  version = 0;
  history = false;
  // Set when the connection is gone
  closed: string | null = null;

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.opened = new Promise((resolve, reject) => {
      this.ws.onopen = () => resolve();
      this.ws.onerror = () => reject(new EngineError("the host can't be reached"));
    });
    this.ws.onclose = () => {
      this.closed = "the connection to the host closed";
      for (const cb of this.pending.values()) cb({ error: this.closed });
      this.pending.clear();
    };
    this.ws.onmessage = (e: MessageEvent<string>) => this.receive(JSON.parse(e.data));
  }

  private receive(m: Record<string, unknown>) {
    if (typeof m.id === "number") {
      const cb = this.pending.get(m.id);
      this.pending.delete(m.id);
      if (typeof m.version === "number") this.version = Math.max(this.version, m.version);
      cb?.(m as Res);
      return;
    }
    const e = m as HostEvent;
    if (e.event === "hello" || e.event === "open") {
      this.build = e.build ?? null;
      this.version = e.version ?? 0;
      if (e.event === "hello") this.history = !!e.history;
    } else if (e.event === "change" && e.build === this.build) {
      // A change this client made itself was answered first, with its
      // version
      if (e.version <= this.version) return;
      this.version = e.version;
    }
    for (const l of this.listeners) l(e);
  }

  // Calls `cb` on the host's events: a change to the build shown that the
  // client didn't make itself, another build opened, changes elsewhere
  onEvent(cb: (e: HostEvent) => void) {
    this.listeners.push(cb);
  }

  private send<T>(msg: Record<string, unknown>): Promise<T> {
    if (this.closed) return Promise.reject(new EngineError(this.closed));
    const id = this.next++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, (res) => {
        if (res.error !== undefined) reject(new EngineError(res.error));
        else resolve(res.ok as T);
      });
      this.opened.then(
        () => this.ws.send(JSON.stringify({ id, ...msg })),
        (e) => {
          this.pending.delete(id);
          reject(e);
        },
      );
    });
  }

  call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.send<T>({ req: { cmd, ...args } });
  }

  // One of the host's own requests (builds, history, ...)
  host<T>(what: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.send<T>({ host: what, args });
  }
}

// Whether the page was served by `pob host`
export function servedByHost(doc: Pick<Document, "querySelector"> = document): boolean {
  return doc.querySelector('meta[name="pob-host"]') !== null;
}

// The host's WebSocket for a page it served
export function hostUrl(loc: Pick<Location, "protocol" | "host"> = location): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}/ws`;
}

export class Engine {
  readonly main: Transport;
  // The host's connection, when the page is served by the host
  readonly hosted: HostTransport | null;
  private background: Transport;
  private backgroundXml: string | null = null;
  // Commands the engine answered with "not ported: <cmd>"
  private unported = new Set<string>();

  constructor(transport: "auto" | "worker" | HostTransport = "auto") {
    if (transport === "auto") transport = servedByHost() ? new HostTransport(hostUrl()) : "worker";
    if (transport instanceof HostTransport) {
      this.hosted = transport;
      this.main = transport;
      this.background = transport;
    } else {
      this.hosted = null;
      this.main = new EngineWorker();
      this.background = new EngineWorker();
    }
  }

  call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.main.call<T>(cmd, args).catch((e) => {
      if (e instanceof EngineError && e.message === `not ported: ${cmd}`) this.unported.add(cmd);
      throw e;
    });
  }

  // False once the engine said it does not have the command
  ported(cmd: string): boolean {
    return !this.unported.has(cmd);
  }

  // Runs a command on the background worker, on the build as it is now;
  // on the host, on the host's session (native, it needs no copy)
  async background_<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    if (this.hosted) return this.hosted.call<T>(cmd, args);
    const { xml } = await this.main.call<{ xml: string }>("saveXml");
    if (xml !== this.backgroundXml) {
      await this.background.call("loadXml", { xml });
      this.backgroundXml = xml;
    }
    return this.background.call<T>(cmd, args);
  }
}
