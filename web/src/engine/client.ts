// Engine client: promise-based requests to engine workers. The pool has a
// main worker holding the build being edited and a background worker for
// slow calculations (node power), which works on a copy of the build so
// edits never wait for it.

import type { WorkerRequest, WorkerResponse } from "./worker";

export class EngineError extends Error {}

class EngineWorker {
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

export class Engine {
  readonly main = new EngineWorker();
  private background = new EngineWorker();
  private backgroundXml: string | null = null;

  call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    return this.main.call<T>(cmd, args);
  }

  // Runs a command on the background worker, on the build as it is now
  async background_<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
    const { xml } = await this.main.call<{ xml: string }>("saveXml");
    if (xml !== this.backgroundXml) {
      await this.background.call("loadXml", { xml });
      this.backgroundXml = xml;
    }
    return this.background.call<T>(cmd, args);
  }
}
