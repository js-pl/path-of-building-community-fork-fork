// Builds the engine for the browser and copies it to public/engine.wasm
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
execFileSync("cargo", ["build", "--profile", "web", "-p", "wasm", "--target", "wasm32-unknown-unknown"], { cwd: root, stdio: "inherit" });
mkdirSync(join(web, "public"), { recursive: true });
copyFileSync(join(root, "target/wasm32-unknown-unknown/web/wasm.wasm"), join(web, "public/engine.wasm"));
console.log("public/engine.wasm written");
