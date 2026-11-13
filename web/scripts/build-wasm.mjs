// Builds the engine for the browser and copies it to public/engine.wasm.
// With --typed, the engine without upstream's translated program: commands
// the typed engine does not have yet are "not ported" errors (see
// docs/progress.md)
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
const typed = process.argv.includes("--typed");
const args = ["build", "--profile", "web", "-p", "wasm", "--target", "wasm32-unknown-unknown"];
if (typed) args.push("--no-default-features");
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
mkdirSync(join(web, "public"), { recursive: true });
const out = join(web, "public/engine.wasm");
copyFileSync(join(root, "target/wasm32-unknown-unknown/web/wasm.wasm"), out);
console.log(`public/engine.wasm written (${typed ? "typed engine only" : "with the translation"}, ${(statSync(out).size / 1e6).toFixed(1)} MB)`);
