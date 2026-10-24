// Writes the game data the engine reads to public/data (never committed):
// core.bin with what starting up and loading a build on the current tree
// reads, and every tree version under files/ for builds on older trees
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(web, "..");
const builds = [join(root, "corpus/generated/gen-0001.xml")];
execFileSync("cargo", ["run", "--release", "-p", "cli", "--", "bundle", join(web, "public/data"), ...builds], { cwd: root, stdio: "inherit" });
