// Writes the passive tree's art to public/art/<version> (never committed):
// upstream's sprite sheets copied from the submodule, and art.json with the
// sprite coordinates, the node groups (for their backgrounds) and the
// masteries' icons. Trees without sprites at zoom 0.3835 (before 3.19) get
// no art; the tree view then draws plain shapes.
//
//   node scripts/build-art.mjs [--if-present] [version...]   (default: every tree version)
//
// `npm run build` runs it with --if-present, so a built web app has the art
// whenever the submodule is checked out.
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { parseLua } from "./lua-literal.mjs";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const treeData = join(web, "../upstream/PathOfBuilding/src/TreeData");
const out = join(web, "public/art");

// The sprite types the tree view draws
const types = [
  "background",
  "normalActive",
  "notableActive",
  "keystoneActive",
  "normalInactive",
  "notableInactive",
  "keystoneInactive",
  "mastery",
  "masteryConnected",
  "masteryActiveSelected",
  "masteryInactive",
  "masteryActiveEffect",
  "tattooActiveEffect",
  "ascendancy",
  "startNode",
  "groupBackground",
  "frame",
  "jewel",
];

function build(version) {
  const dir = join(treeData, version);
  const tree = parseLua(readFileSync(join(dir, "tree.lua"), "utf8"));
  let sprites = tree.sprites;
  if (existsSync(join(dir, "sprites.lua"))) sprites = parseLua(readFileSync(join(dir, "sprites.lua"), "utf8")).sprites;
  if (!sprites) return false;

  const sheets = [];
  const art = { sheets, sprites: {}, groups: [], masteries: {} };
  // Bloodlines' class art joins the ascendancies'
  const bloodlines = Object.keys(sprites).filter((t) => t.endsWith("Bloodline"));
  for (const type of [...types, ...bloodlines]) {
    const data = sprites[type];
    if (!data) continue;
    const zoom = data.filename ? data : data["0.3835"];
    if (!zoom) return false;
    const file = zoom.filename.replace(/\?.*$/, "").replace(/.*\//, "");
    if (!existsSync(join(dir, file))) continue;
    let sheet = sheets.indexOf(file);
    if (sheet < 0) sheet = sheets.push(file) - 1;
    const into = bloodlines.includes(type) ? "ascendancy" : type;
    const map = (art.sprites[into] ??= {});
    for (const [name, c] of Object.entries(zoom.coords)) map[name] = [sheet, c.x, c.y, c.w, c.h];
  }
  if (!art.sprites.frame) return false;

  // Groups: upstream gives one an ascendancy from its nodes
  const groups = new Map();
  const entries = (t) => (Array.isArray(t) ? t.map((v, k) => [k + 1, v]) : Object.entries(t ?? {}));
  for (const [key, g] of entries(tree.groups)) {
    const orbits = Array.isArray(g.orbits) ? g.orbits : Object.values(g.orbits ?? {});
    groups.set(String(key), { x: g.x, y: g.y, orbits, proxy: !!g.isProxy });
  }
  for (const [id, n] of entries(tree.nodes)) {
    const g = groups.get(String(n.group));
    if (g && n.ascendancyName) {
      g.ascendancy = n.ascendancyName;
      if (n.isAscendancyStart) g.start = true;
    }
    if (n.isMastery && n.inactiveIcon) art.masteries[id] = [n.inactiveIcon, n.activeIcon, n.activeEffectImage];
  }
  for (const g of groups.values()) {
    if (g.proxy) continue;
    const entry = [Math.round(g.x), Math.round(g.y)];
    if (g.ascendancy) {
      if (!g.start) continue;
      entry.push(0, g.ascendancy);
    } else {
      // The largest of orbits 1 to 3 picks the background, as upstream
      const size = g.orbits.includes(3) ? 3 : g.orbits.includes(2) ? 2 : g.orbits.includes(1) ? 1 : 0;
      if (!size) continue;
      entry.push(size);
    }
    art.groups.push(entry);
  }

  const dest = join(out, version);
  mkdirSync(dest, { recursive: true });
  for (const f of sheets) copyFileSync(join(dir, f), join(dest, f));
  writeFileSync(join(dest, "art.json"), JSON.stringify(art));
  return true;
}

// --if-present (npm run build): without the submodule, warn and go on
const args = process.argv.slice(2);
const ifPresent = args.includes("--if-present");
const asked = args.filter((a) => !a.startsWith("--"));
if (!existsSync(treeData)) {
  const msg = `${treeData} is missing: run git submodule update --init (the tree gets no art)`;
  if (!ifPresent) throw new Error(msg);
  console.warn(`build-art: ${msg}`);
  process.exit(0);
}
const versions = asked.length
  ? asked
  : readdirSync(treeData, { withFileTypes: true })
      .filter((d) => d.isDirectory() && existsSync(join(treeData, d.name, "tree.lua")))
      .map((d) => d.name);
const done = versions.filter(build);
console.log(`public/art written for ${done.length} tree versions (${done.join(", ")})`);
