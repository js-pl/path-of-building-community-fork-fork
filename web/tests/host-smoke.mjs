// End to end on `pob host`: Playwright drives the web app the host serves
// while `pob ctl` edits the same build as an agent would; the agent's
// change shows in the sidebar without a reload, and the UI's own edit is
// committed to the build's history as the user's.
//
//   npm run build && cargo build --release -p cli
//   node tests/host-smoke.mjs            (POB=<pob binary> picks another)
//
// Needs Playwright with Chromium and git; not part of `npm test`.
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, copyFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
const pob = process.env.POB ?? join(root, "target/release/pob");
const dir = mkdtempSync(join(tmpdir(), "pob-smoke-"));
const builds = join(dir, "builds");
const socket = join(dir, "run/control.sock");
mkdirSync(join(builds, "RF"), { recursive: true });
copyFileSync(join(root, "corpus/generated/gen-0003.xml"), join(builds, "RF/build.xml"));

const host = spawn(pob, ["host", "--builds", builds, "--socket", socket, "--listen", "none", "--web", "127.0.0.1:0", "--app", join(web, "dist"), "--build", "RF", "--no-open"], {
  stdio: ["ignore", "ignore", "pipe"],
});
let log = "";
const url = await new Promise((resolve, reject) => {
  host.stderr.on("data", (d) => {
    log += d;
    const m = log.match(/the web app on (\S+)/);
    if (m) resolve(m[1]);
  });
  host.on("exit", (c) => reject(new Error(`pob host exited (${c}): ${log}`)));
});

const ctl = (...args) => JSON.parse(execFileSync(pob, ["ctl", "--socket", socket, "--build", "RF", "--agent", "smoke", "--json", ...args], { encoding: "utf8" })).ok;

const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  page.on("pageerror", (e) => console.error("page:", e.message));
  await page.goto(url);
  const sidebar = page.locator("aside.sidebar");
  await sidebar.waitFor({ timeout: 120_000 });
  await page.waitForFunction(() => document.querySelector("aside.sidebar")?.textContent?.length, null, { timeout: 120_000 });
  const before = await sidebar.innerText();

  // An agent's edit shows at once
  const r = ctl("allocate", "nodes=[36634]");
  assert.ok(r.version >= 1, JSON.stringify(r));
  await page.waitForFunction((b) => document.querySelector("aside.sidebar")?.innerText !== b, before, { timeout: 30_000 });
  console.log(`agent's step shown: ${r.label}`);

  // The UI's undo undoes it, as the user
  await page.getByRole("tab", { name: "Tree" }).click();
  await page.getByRole("button", { name: "Undo" }).click();
  await page.waitForFunction((b) => document.querySelector("aside.sidebar")?.innerText === b, before, { timeout: 30_000 });
  const changes = ctl("changes_since", "version=0").changes;
  assert.deepEqual(
    changes.map((c) => c.source),
    ["agent ctl", "user"],
    JSON.stringify(changes),
  );

  // The history tab: the changes with who made them, and a restore
  await page.getByRole("tab", { name: "History" }).click();
  const steps = page.locator(".history-tab .steps li");
  await steps.first().waitFor();
  assert.deepEqual(await page.locator(".history-tab .steps .source").allInnerTexts(), ["you", "ctl"]);
  page.once("dialog", (d) => d.accept());
  const restoreButtons = page.getByRole("button", { name: "Restore" });
  await restoreButtons.first().waitFor();
  // The agent's version, which the undo took back
  await restoreButtons.first().click();
  await page.waitForFunction(() => document.querySelectorAll(".history-tab .steps li").length === 3, null, { timeout: 30_000 });
  const last = ctl("changes_since", "version=2").changes[0];
  assert.ok(last.label.startsWith("restore "), JSON.stringify(last));

  // The agents tab: an agent's copy, kept among the user's builds
  ctl("copy", "name=try");
  await page.getByRole("tab", { name: "Agents" }).click();
  await page.getByRole("button", { name: "Keep" }).click();
  await page.getByText("Kept as try.").waitFor({ timeout: 30_000 });
  console.log("ok: the UI and an agent on one build on the host");
} finally {
  await browser.close();
  host.kill();
  rmSync(dir, { recursive: true, force: true });
}
