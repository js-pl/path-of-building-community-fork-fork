// The passive tree's art: upstream's sprite sheets for a tree version, as
// written by scripts/build-art.mjs to public/art/<version>. A version
// without art (old trees, or a build without the art step) resolves to
// null and the tree is drawn with plain shapes.

type Sprite = [sheet: number, x: number, y: number, w: number, h: number];

type ArtJson = {
  sheets: string[];
  sprites: Record<string, Record<string, Sprite>>;
  // [x, y, size 1-3, or 0 with the ascendancy of its start node]
  groups: [number, number, number, string?][];
  masteries: Record<string, [inactive: string, active: string, effect: string]>;
};

export type Art = ArtJson & { images: (HTMLImageElement | null)[] };

const cache = new Map<string, Promise<Art | null>>();

function image(url: string): Promise<HTMLImageElement | null> {
  return new Promise((resolve) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(img);
    img.onerror = () => resolve(null);
    img.src = url;
  });
}

// Calls `onUpdate` whenever more of the sheets have loaded, so the tree can
// show icons as soon as their sheet is there
export function loadArt(version: string, onUpdate: () => void): Promise<Art | null> {
  let p = cache.get(version);
  if (!p) {
    const base = new URL(`art/${version}/`, document.baseURI);
    p = fetch(new URL("art.json", base))
      .then((r) => (r.ok ? (r.json() as Promise<ArtJson>) : null))
      .then((json) => {
        if (!json) return null;
        const art: Art = { ...json, images: json.sheets.map(() => null) };
        json.sheets.forEach((f, i) =>
          image(new URL(f, base).href).then((img) => {
            art.images[i] = img;
            onUpdate();
          }),
        );
        return art;
      })
      .catch(() => null);
    cache.set(version, p);
  }
  return p;
}

// Draws a sprite centred on (x, y) in tree units, at upstream's size (a
// sheet pixel is 1.33 tree units either side of the centre). `half` draws
// the top half and mirrors it below, as upstream does for large groups.
export function drawSprite(ctx: CanvasRenderingContext2D, art: Art, s: Sprite | undefined, x: number, y: number, half = false): boolean {
  if (!s) return false;
  const img = art.images[s[0]];
  if (!img) return false;
  const w = s[3] * 1.33;
  const h = s[4] * 1.33;
  if (half) {
    ctx.drawImage(img, s[1], s[2], s[3], s[4], x - w, y - 2 * h, 2 * w, 2 * h);
    ctx.save();
    ctx.translate(x, y);
    ctx.scale(1, -1);
    ctx.drawImage(img, s[1], s[2], s[3], s[4], -w, -2 * h, 2 * w, 2 * h);
    ctx.restore();
  } else {
    ctx.drawImage(img, s[1], s[2], s[3], s[4], x - w, y - h, 2 * w, 2 * h);
  }
  return true;
}

// A named asset (frames, group backgrounds, class art) from any sheet
export function asset(art: Art, name: string): Sprite | undefined {
  for (const t of ["frame", "ascendancy", "startNode", "groupBackground", "jewel", "background"]) {
    const s = art.sprites[t]?.[name];
    if (s) return s;
  }
  return undefined;
}

// A repeating pattern of the tree's background tile
const patterns = new WeakMap<Art, CanvasPattern | null>();
export function backgroundPattern(ctx: CanvasRenderingContext2D, art: Art): CanvasPattern | null {
  if (patterns.has(art)) return patterns.get(art)!;
  const s = asset(art, "Background2") ?? asset(art, "Background1");
  const img = s && art.images[s[0]];
  if (!s || !img) return null;
  const tile = document.createElement("canvas");
  tile.width = s[3];
  tile.height = s[4];
  tile.getContext("2d")!.drawImage(img, s[1], s[2], s[3], s[4], 0, 0, s[3], s[4]);
  const p = ctx.createPattern(tile, "repeat");
  // Upstream draws the tile 1.33 * 2.5 tree units per pixel
  p?.setTransform(new DOMMatrix().scale(1.33 * 2.5));
  patterns.set(art, p);
  return p;
}
