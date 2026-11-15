// Writes the game data the engine reads to public/data (never committed):
// core.bin with the typed engine's data in its binary form (game data, mod
// parser, the current tree) and every tree version under files/ for builds
// on older trees
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
const args = ["run", "--release", "-p", "cli", "--", "bundle", join(web, "public/data")];
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });
