// Passive tree: drawn on a canvas from the engine's tree geometry and
// upstream's sprite sheets (see treeArt.ts), falling back to plain shapes
// for trees without art. Clicking a node does what a left click does in
// upstream's tree view; hovering shows upstream's node tooltip, with the
// stat changes of allocating it.
//
// Drawing runs in animation frames off refs, so panning and zooming never
// re-render the page.

import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { NodePower, Tree, TreeNode, TreeState, TooltipLine } from "../engine/types";
import { FloatingTooltip } from "./Tooltip";
import { ColorText } from "./ColorText";
import { type Art, asset, backgroundPattern, drawSprite, loadArt } from "./treeArt";

// Hit radius of a node in tree units: upstream's frame size (art width * 1.33)
const hitRadius: Record<string, number> = {
  Normal: 53,
  Notable: 77,
  Keystone: 112,
  Mastery: 86,
  Socket: 77,
  ClusterSocket: 77,
  ClassStart: 90,
  AscendClassStart: 40,
};

// Radius of the plain shapes when there is no art
const shapeRadius: Record<string, number> = {
  Normal: 24,
  Notable: 36,
  Keystone: 50,
  Mastery: 38,
  Socket: 34,
  ClusterSocket: 34,
  ClassStart: 70,
  AscendClassStart: 24,
};

type View = { scale: number; x: number; y: number };

type Mastery = { id: number; name: string; options: { label: string; id: number }[] };

// What a frame draws; kept in a ref so drawing never waits for a render
type Scene = {
  tree: Tree;
  byId: Map<number, TreeNode>;
  alloc: Set<number>;
  ts: TreeState;
  path: Set<number>;
  matches: Set<number> | null;
  power: NodePower | null;
  art: Art | null;
  hover: number | null;
  // Changes whenever the tree without hover highlights does
  key: object;
};

// The view of each tree version, kept when leaving the tab
const savedViews = new Map<string, View>();

const MAX_SCALE = 1.6;

export function TreeTab({ act, rev }: TabProps) {
  const [tree, setTree] = useState<Tree | null>(null);
  const [ts, setTs] = useState<TreeState | null>(null);
  const [art, setArt] = useState<Art | null>(null);
  // Counts the art's sheets as they load
  const [artTick, setArtTick] = useState(0);
  const [hover, setHover] = useState<{ node: TreeNode; x: number; y: number; r: number } | null>(null);
  const [tip, setTip] = useState<{ id: number; lines: TooltipLine[] } | null>(null);
  const [path, setPath] = useState<Set<number>>(new Set());
  const [search, setSearch] = useState("");
  const [powerStats, setPowerStats] = useState<{ stat?: string; label: string }[]>([]);
  const [powerIndex, setPowerIndex] = useState(0);
  const [power, setPower] = useState<NodePower | null>(null);
  const [powerBusy, setPowerBusy] = useState(false);
  const [mastery, setMastery] = useState<Mastery | null>(null);
  const [specs, setSpecs] = useState<{ active: number; list: string[] } | null>(null);
  const [cursor, setCursor] = useState<"grab" | "grabbing" | "pointer">("grab");
  const canvas = useRef<HTMLCanvasElement>(null);
  const view = useRef<View | null>(null);
  const scene = useRef<Scene | null>(null);
  const frame = useRef(0);
  const layer = useRef<Layer | null>(null);
  const zoomTarget = useRef<{ scale: number; mx: number; my: number } | null>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const drag = useRef<{ moved: boolean; pinch?: number } | null>(null);
  const tips = useRef(new Map<number, { lines: TooltipLine[] | null; path: number[] }>());

  useEffect(() => {
    let stale = false;
    (async () => {
      const [t, s, sp] = await Promise.all([
        engine.call<Tree>("tree"),
        engine.call<TreeState>("treeState"),
        engine.call<{ active: number; list: string[] }>("treeSpecs"),
      ]);
      if (stale) return;
      tips.current.clear();
      setTree(t);
      setTs(s);
      setSpecs(sp);
    })();
    return () => {
      stale = true;
    };
  }, [rev]);

  useEffect(() => {
    engine.call<{ stat?: string; label: string }[]>("powerStats").then(setPowerStats);
  }, []);

  // The art of the tree's version; frames redraw as its sheets arrive
  const version = tree?.version;
  useEffect(() => {
    if (!version) return;
    let stale = false;
    loadArt(version, () => setArtTick((t) => t + 1)).then((a) => !stale && setArt(a));
    return () => {
      stale = true;
    };
  }, [version]);

  // Node power runs on the background worker after every change
  useEffect(() => {
    if (!powerIndex) {
      setPower(null);
      return;
    }
    let stale = false;
    setPowerBusy(true);
    engine
      .background_<NodePower>("nodePower", { index: powerIndex })
      .then((p) => !stale && setPower(p))
      .finally(() => !stale && setPowerBusy(false));
    return () => {
      stale = true;
    };
  }, [powerIndex, rev]);

  const byId = useMemo(() => {
    const m = new Map<number, TreeNode>();
    for (const n of tree?.nodes ?? []) m.set(n.id, n);
    return m;
  }, [tree]);

  const alloc = useMemo(() => new Set(ts ? [...ts.alloc, ...ts.granted] : []), [ts]);

  const matches = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q || !tree) return null;
    return new Set(tree.nodes.filter((n) => n.name?.toLowerCase().includes(q) || n.stats.some((s) => s.toLowerCase().includes(q))).map((n) => n.id));
  }, [search, tree]);

  // The nodes' extent: bloodline ascendancies lie outside the tree's bounds
  const bounds = useMemo(() => {
    if (!tree?.nodes.length) return tree?.bounds;
    const b = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
    for (const n of tree.nodes) {
      b.minX = Math.min(b.minX, n.x);
      b.minY = Math.min(b.minY, n.y);
      b.maxX = Math.max(b.maxX, n.x);
      b.maxY = Math.max(b.maxY, n.y);
    }
    const pad = 400;
    return { minX: b.minX - pad, minY: b.minY - pad, maxX: b.maxX + pad, maxY: b.maxY + pad };
  }, [tree]);

  const fitView = (c: HTMLCanvasElement, t: Tree): View => {
    const b = bounds ?? t.bounds;
    const scale = Math.min(c.clientWidth / (b.maxX - b.minX), c.clientHeight / (b.maxY - b.minY)) * 0.95;
    return { scale, x: c.clientWidth / 2 - ((b.minX + b.maxX) / 2) * scale, y: c.clientHeight / 2 - ((b.minY + b.maxY) / 2) * scale };
  };

  const minScale = () => {
    const c = canvas.current;
    if (!c || !tree) return 0.01;
    return fitView(c, tree).scale * 0.6;
  };

  // The saved view of this version, or the whole tree the first time
  useEffect(() => {
    const c = canvas.current;
    if (!c || !tree) return;
    if (!view.current) view.current = savedViews.get(tree.version) ?? fitView(c, tree);
    savedViews.set(tree.version, view.current);
    schedule();
  }, [tree]);

  const schedule = () => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const c = canvas.current;
      const v = view.current;
      if (!c || !v || !scene.current) return;
      // A wheel zoom eases towards its target over a few frames
      const z = zoomTarget.current;
      if (z) {
        const f = Math.abs(z.scale / v.scale - 1) < 0.002 ? z.scale / v.scale : Math.pow(z.scale / v.scale, 0.3);
        v.x = z.mx - (z.mx - v.x) * f;
        v.y = z.my - (z.my - v.y) * f;
        v.scale *= f;
        if (v.scale === z.scale || f === z.scale / v.scale) zoomTarget.current = null;
        else schedule();
      }
      draw(c, v, scene.current, layer, !zoomTarget.current && !drag.current?.pinch);
    });
  };
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  const key = useMemo(() => ({}), [tree, alloc, ts, matches, power, art, artTick]);
  if (tree && ts) scene.current = { tree, byId, alloc, ts, path, matches, power, art, hover: hover?.node.id ?? null, key };
  useEffect(() => schedule());

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ro = new ResizeObserver(() => schedule());
    ro.observe(c);
    return () => ro.disconnect();
  }, []);

  const local = (e: { clientX: number; clientY: number }) => {
    const r = canvas.current!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const nodeAt = (e: PointerEvent): TreeNode | null => {
    const v = view.current;
    if (!tree || !v) return null;
    const l = local(e);
    const p = { x: (l.x - v.x) / v.scale, y: (l.y - v.y) / v.scale };
    let best: TreeNode | null = null;
    let bestD = Infinity;
    // Small nodes stay easy to hit when zoomed out
    const minR = 10 / v.scale;
    for (const n of tree.nodes) {
      const dx = n.x - p.x;
      const dy = n.y - p.y;
      if (Math.abs(dx) > 300 && Math.abs(dx) > minR) continue;
      const d = Math.hypot(dx, dy);
      const r = Math.max(art ? (hitRadius[n.kind] ?? 53) : (shapeRadius[n.kind] ?? 24) * 1.2, minR);
      if (d < r && d < bestD) {
        best = n;
        bestD = d;
      }
    }
    return best;
  };

  const anchor = (n: TreeNode) => {
    const v = view.current!;
    const r = canvas.current!.getBoundingClientRect();
    const rad = (art ? (hitRadius[n.kind] ?? 53) : (shapeRadius[n.kind] ?? 24)) * v.scale;
    return { node: n, x: r.left + v.x + n.x * v.scale, y: r.top + v.y + n.y * v.scale, r: rad };
  };

  // Tooltip and path of the hovered node, kept until the build changes
  useEffect(() => {
    if (!hover) {
      setPath(new Set());
      return;
    }
    const id = hover.node.id;
    const known = tips.current.get(id);
    if (known) {
      setTip({ id, lines: known.lines ?? [] });
      setPath(new Set(known.path));
      return;
    }
    let stale = false;
    const t = setTimeout(async () => {
      const [tt, p] = await Promise.all([
        // No tooltip where the engine has none
        engine.call<{ lines: TooltipLine[] }>("nodeTooltip", { id }).catch(() => null),
        engine.call<{ path: number[] }>("nodePath", { id }).catch(() => ({ path: [] })),
      ]);
      tips.current.set(id, { lines: tt?.lines ?? null, path: p.path });
      if (!stale) {
        setTip({ id, lines: tt?.lines ?? [] });
        setPath(new Set(p.path));
      }
    }, 60);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [hover?.node.id, rev]);

  const onDown = (e: PointerEvent) => {
    canvas.current!.setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, local(e));
    if (pointers.current.size === 2) {
      const [a, b] = [...pointers.current.values()];
      drag.current = { moved: true, pinch: Math.hypot(a.x - b.x, a.y - b.y) };
    } else drag.current = { moved: false };
  };

  const onMove = (e: PointerEvent) => {
    const prev = pointers.current.get(e.pointerId);
    const v = view.current;
    if (prev && drag.current && v) {
      const p = local(e);
      pointers.current.set(e.pointerId, p);
      zoomTarget.current = null;
      if (drag.current.pinch && pointers.current.size === 2) {
        const [a, b] = [...pointers.current.values()];
        const d = Math.hypot(a.x - b.x, a.y - b.y);
        const mx = (a.x + b.x) / 2;
        const my = (a.y + b.y) / 2;
        const f = clamp(v.scale * (d / drag.current.pinch), minScale(), MAX_SCALE) / v.scale;
        v.x = mx - (mx - v.x) * f;
        v.y = my - (my - v.y) * f;
        v.scale *= f;
        drag.current.pinch = d;
      } else {
        v.x += p.x - prev.x;
        v.y += p.y - prev.y;
        if (Math.abs(p.x - prev.x) + Math.abs(p.y - prev.y) > 0) drag.current.moved ||= true;
      }
      if (drag.current.moved) {
        if (hover) setHover(null);
        setCursor("grabbing");
      }
      schedule();
      return;
    }
    if (e.pointerType !== "mouse") return;
    const n = nodeAt(e);
    if (n?.id !== hover?.node.id) setHover(n ? anchor(n) : null);
    setCursor(n ? "pointer" : "grab");
  };

  const onUp = async (e: PointerEvent) => {
    pointers.current.delete(e.pointerId);
    const d = drag.current;
    if (pointers.current.size) {
      if (d) d.pinch = undefined;
      return;
    }
    drag.current = null;
    setCursor("grab");
    schedule();
    if (d?.moved) return;
    const n = nodeAt(e);
    if (!n) return;
    const r = await act<{ mastery?: { label: string; id: number }[] }>("clickNode", { id: n.id });
    if (r?.mastery) setMastery({ id: n.id, name: n.name, options: r.mastery });
  };

  const zoomBy = (factor: number, mx?: number, my?: number) => {
    const c = canvas.current;
    const v = view.current;
    if (!c || !v) return;
    const cur = zoomTarget.current?.scale ?? v.scale;
    const scale = clamp(cur * factor, minScale(), MAX_SCALE);
    zoomTarget.current = { scale, mx: mx ?? c.clientWidth / 2, my: my ?? c.clientHeight / 2 };
    if (hover) setHover(null);
    schedule();
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const l = local(e);
    // Lines and pixels: a mouse wheel notch or a trackpad's small steps
    const dy = e.deltaMode === 1 ? e.deltaY * 33 : e.deltaY;
    zoomBy(Math.exp(-clamp(dy, -150, 150) * 0.0025), l.x, l.y);
  };

  const fit = () => {
    const c = canvas.current;
    if (!c || !tree) return;
    const f = fitView(c, tree);
    view.current!.x = f.x;
    view.current!.y = f.y;
    view.current!.scale = f.scale;
    zoomTarget.current = null;
    schedule();
  };

  const showTip = hover && tip && tip.id === hover.node.id && tip.lines.length > 0;

  return (
    <div class="tree-tab">
      <div class="tree-toolbar">
        {specs && specs.list.length > 1 ? (
          <select
            title="Passive tree"
            value={String(specs.active)}
            onChange={(e) => act("selectTreeSpec", { index: Number((e.target as HTMLSelectElement).value) })}
          >
            {specs.list.map((t, i) => (
              <option key={i} value={String(i + 1)}>
                {t}
              </option>
            ))}
          </select>
        ) : null}
        <div class="search-box">
          <input type="search" placeholder="Search nodes" value={search} onInput={(e) => setSearch((e.target as HTMLInputElement).value)} />
          <span class="search-count">{matches ? `${matches.size} found` : ""}</span>
        </div>
        <label class="inline">
          <span>Node power</span>
          <select value={String(powerIndex)} onChange={(e) => setPowerIndex(Number((e.target as HTMLSelectElement).value))}>
            <option value="0">Off</option>
            {powerStats.map((s, i) => (
              <option key={i} value={String(i + 1)}>
                {s.label}
              </option>
            ))}
          </select>
        </label>
        <span class="spacer" />
        <div class="button-group">
          <button title="Zoom out" onClick={() => zoomBy(1 / 1.5)}>
            −
          </button>
          <button title="Show the whole tree" onClick={fit}>
            Fit
          </button>
          <button title="Zoom in" onClick={() => zoomBy(1.5)}>
            +
          </button>
        </div>
        <button onClick={() => act("undo")}>Undo</button>
        <button class="danger" onClick={() => act("resetTree")}>
          Reset tree
        </button>
      </div>
      <div class="tree-stage">
        <canvas
          ref={canvas}
          class="tree-canvas"
          style={{ cursor }}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={(e) => {
            pointers.current.delete(e.pointerId);
            drag.current = null;
          }}
          onPointerLeave={(e) => {
            if (e.pointerType === "mouse" && !drag.current) setHover(null);
          }}
          onWheel={onWheel}
        />
        <div class={"stage-status" + (powerBusy || !tree ? " shown" : "")}>
          <span class="spinner" /> {tree ? "Calculating node power" : "Loading tree"}
        </div>
        {power ? <PowerLegend combined={power.combined} /> : null}
      </div>
      {showTip ? <FloatingTooltip lines={tip!.lines} x={hover!.x} y={hover!.y} gap={hover!.r} /> : null}
      {mastery ? (
        <div class="modal-back" onClick={() => setMastery(null)}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h3>{mastery.name || "Choose a mastery effect"}</h3>
            <p class="muted">Choose a mastery effect</p>
            {mastery.options.map((o) => (
              <button
                key={o.id}
                class="mastery-option"
                onClick={async () => {
                  setMastery(null);
                  await act("selectMastery", { id: mastery.id, effect: o.id });
                }}
              >
                <ColorText text={o.label} />
              </button>
            ))}
            <div class="buttons">
              <button onClick={() => setMastery(null)}>Cancel</button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}

function PowerLegend({ combined }: { combined: boolean }) {
  return (
    <div class="power-legend">
      {combined ? (
        <>
          <span class="swatch" style={{ background: "rgb(230,0,0)" }} /> Offence <span class="swatch" style={{ background: "rgb(0,0,230)" }} /> Defence{" "}
          <span class="swatch" style={{ background: "rgb(230,180,230)" }} /> Both
        </>
      ) : (
        <>
          Low <span class="gradient" /> High
        </>
      )}
    </div>
  );
}

function clamp(v: number, lo: number, hi: number) {
  return Math.max(lo, Math.min(hi, v));
}

// Upstream's heat map colors (the RED/BLUE theme)
function powerColor(p: NodePower, id: number): string | null {
  const n = p.nodes[String(id)];
  const col = (v: number | undefined, max: number | undefined) => (max ? Math.min(1, Math.pow((Math.max(v ?? 0, 0) / max) * 1.5, 0.5)) : 0);
  if (p.combined) {
    const o = col(n?.offence, p.max.offence);
    const d = col(n?.defence, p.max.defence);
    const mix = (Math.max(o - 0.5, 0) + Math.max(d - 0.5, 0)) / 2;
    return `rgb(${Math.round(255 * o)}, ${Math.round(255 * mix)}, ${Math.round(255 * d)})`;
  }
  return `rgb(${Math.round(255 * col(n?.single, p.max.single))}, 0, 0)`;
}

// The tree without hover highlights, drawn once per view into an offscreen
// layer a quarter of the canvas larger on each side: panning only copies
// it, and a zoom scales it until the view settles
type Layer = { canvas: HTMLCanvasElement; key: object; view: View; w: number; h: number };

function draw(c: HTMLCanvasElement, v: View, s: Scene, layers: { current: Layer | null }, settled: boolean) {
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth;
  const h = c.clientHeight;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
  }
  const ctx = c.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.globalAlpha = 1;
  ctx.fillStyle = "#07080a";
  ctx.fillRect(0, 0, w, h);

  let L = layers.current;
  const k = L ? v.scale / L.view.scale : 1;
  const dx = L ? v.x - L.view.x * k : 0;
  const dy = L ? v.y - L.view.y * k : 0;
  const covers = !!L && k === 1 && dx <= 0 && dy <= 0 && dx + L.w >= w && dy + L.h >= h;
  if (!L || L.key !== s.key || (settled && !covers)) {
    const lw = Math.ceil(w * 1.5);
    const lh = Math.ceil(h * 1.5);
    const ldpr = Math.min(dpr, 2);
    const canvas = L?.canvas ?? document.createElement("canvas");
    if (canvas.width !== Math.round(lw * ldpr) || canvas.height !== Math.round(lh * ldpr)) {
      canvas.width = Math.round(lw * ldpr);
      canvas.height = Math.round(lh * ldpr);
    }
    const lv = { scale: v.scale, x: v.x + w * 0.25, y: v.y + h * 0.25 };
    const lctx = canvas.getContext("2d")!;
    lctx.setTransform(ldpr, 0, 0, ldpr, 0, 0);
    lctx.clearRect(0, 0, lw, lh);
    paint(lctx, lv, lw, lh, ldpr, { ...s, path: new Set(), hover: null }, null);
    L = layers.current = { canvas, key: s.key, view: lv, w: lw, h: lh };
    ctx.drawImage(canvas, -w * 0.25, -h * 0.25, lw, lh);
  } else {
    ctx.drawImage(L.canvas, dx, dy, L.w * k, L.h * k);
  }

  // The hovered node and its path on top
  if (s.hover !== null || s.path.size) {
    const only = new Set(s.path);
    if (s.hover !== null) only.add(s.hover);
    ctx.setTransform(dpr * v.scale, 0, 0, dpr * v.scale, dpr * v.x, dpr * v.y);
    paint(ctx, v, w, h, dpr, s, only);
  }
}

// Draws the tree at view `v` on a `w` x `h` canvas: all of it, or with
// `only`, the path highlights and those nodes
function paint(ctx: CanvasRenderingContext2D, v: View, w: number, h: number, dpr: number, s: Scene, only: Set<number> | null) {
  ctx.setTransform(dpr * v.scale, 0, 0, dpr * v.scale, dpr * v.x, dpr * v.y);
  ctx.imageSmoothingQuality = "high";

  // The visible part of the tree, with room for the largest art
  const margin = 700;
  const box = { x0: -v.x / v.scale - margin, y0: -v.y / v.scale - margin, x1: (w - v.x) / v.scale + margin, y1: (h - v.y) / v.scale + margin };
  const visible = (x: number, y: number) => x > box.x0 && x < box.x1 && y > box.y0 && y < box.y1;
  const { tree, byId, alloc, ts, path, matches, art, hover } = s;
  const px = 1 / v.scale;
  const otherAscendancy = (a?: string) => !!a && a !== ts.ascendancy && a !== ts.secondaryAscendancy;

  if (art && !only) {
    const bg = backgroundPattern(ctx, art);
    if (bg) {
      ctx.fillStyle = bg;
      ctx.fillRect(box.x0, box.y0, box.x1 - box.x0, box.y1 - box.y0);
    }
    for (const [x, y, size, asc] of art.groups) {
      if (!visible(x, y) && !asc) continue;
      if (asc) {
        ctx.globalAlpha = otherAscendancy(asc) ? 0.5 : 1;
        drawSprite(ctx, art, asset(art, "Classes" + asc), x, y);
        ctx.globalAlpha = 1;
      } else drawSprite(ctx, art, asset(art, `PSGroupBackground${size}`), x, y, size === 3);
    }
  }

  // Connectors: a dark casing under a coloured core, gold when allocated
  // and blue on the path to the hovered node
  const pathOf = (id: number) => alloc.has(id) || path.has(id) || id === hover;
  const strokes: { conn: Tree["connectors"][number]; state: 0 | 1 | 2; faded: boolean }[] = [];
  for (const conn of tree.connectors) {
    const a = byId.get(conn.a);
    const b = byId.get(conn.b);
    if (!a || !b || (!visible(a.x, a.y) && !visible(b.x, b.y))) continue;
    const state = alloc.has(a.id) && alloc.has(b.id) ? 2 : (path.size || hover) && pathOf(a.id) && pathOf(b.id) && (path.has(a.id) || path.has(b.id)) ? 1 : 0;
    if (only && state !== 1) continue;
    strokes.push({ conn, state, faded: otherAscendancy(conn.ascendancy) });
  }
  const trace = (conn: Tree["connectors"][number]) => {
    const a = byId.get(conn.a)!;
    const b = byId.get(conn.b)!;
    ctx.beginPath();
    if (conn.arc) {
      const { x, y, r } = conn.arc;
      const a1 = Math.atan2(a.y - y, a.x - x);
      const a2 = Math.atan2(b.y - y, b.x - x);
      let d = a2 - a1;
      while (d > Math.PI) d -= 2 * Math.PI;
      while (d < -Math.PI) d += 2 * Math.PI;
      ctx.arc(x, y, r, a1, a1 + d, d < 0);
    } else {
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
    }
  };
  ctx.lineCap = "round";
  const casing = Math.max(art ? 20 : 12, 2.5 * px);
  for (const { conn, faded } of only ? [] : strokes) {
    ctx.globalAlpha = faded ? 0.4 : 1;
    trace(conn);
    ctx.strokeStyle = "#0d0c0a";
    ctx.lineWidth = casing;
    ctx.stroke();
  }
  // Allocated and path lines get a soft halo under their core
  const colors = ["#5c5544", "#78b4ff", "#e8c06a"];
  const halos = ["", "rgba(120, 180, 255, 0.28)", "rgba(255, 196, 90, 0.28)"];
  for (const pass of [0, 1, 2] as const) {
    for (const { conn, state, faded } of strokes) {
      if (state !== pass) continue;
      ctx.globalAlpha = faded ? 0.4 : 1;
      trace(conn);
      if (state) {
        ctx.strokeStyle = halos[state];
        ctx.lineWidth = Math.max(26, 5 * px);
        ctx.stroke();
      }
      ctx.strokeStyle = colors[state];
      ctx.lineWidth = Math.max(state ? 10 : 7, (state ? 1.6 : 1) * px);
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;

  // Nodes
  for (const n of only ? [...only].map((id) => byId.get(id)!).filter(Boolean) : tree.nodes) {
    if (!visible(n.x, n.y)) continue;
    const isAlloc = alloc.has(n.id);
    ctx.globalAlpha = otherAscendancy(n.ascendancy) ? 0.55 : 1;
    if (art) drawArtNode(ctx, art, n, isAlloc, s);
    else drawShapeNode(ctx, n, isAlloc, s, px);
    if (matches?.has(n.id) && !only) {
      const r = (art ? (hitRadius[n.kind] ?? 53) : (shapeRadius[n.kind] ?? 24) + 12) + 10;
      ctx.beginPath();
      ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
      ctx.lineWidth = Math.max(28, 7 * px);
      ctx.strokeStyle = "rgba(255, 90, 60, 0.3)";
      ctx.stroke();
      ctx.lineWidth = Math.max(10, 2.5 * px);
      ctx.strokeStyle = "#ff5a3c";
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;

  // Names of keystones, and of notables once zoomed in (hovering shows the rest)
  if (v.scale > 0.09 && !only) {
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    ctx.lineJoin = "round";
    for (const n of tree.nodes) {
      const big = n.kind === "Keystone" || (n.kind === "Notable" && v.scale > 0.22);
      if (!big || !n.name || !visible(n.x, n.y)) continue;
      const size = n.kind === "Keystone" ? 13 : 12;
      ctx.font = `600 ${size * px}px system-ui, sans-serif`;
      const y = n.y + (art ? (hitRadius[n.kind] ?? 53) : (shapeRadius[n.kind] ?? 24)) + 4 * px;
      ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
      ctx.lineWidth = 3.5 * px;
      ctx.strokeText(n.name, n.x, y);
      ctx.fillStyle = alloc.has(n.id) ? "#f3dca0" : n.kind === "Keystone" ? "#e9e2cf" : "#c9c3b4";
      ctx.fillText(n.name, n.x, y);
    }
  }
}

function drawArtNode(ctx: CanvasRenderingContext2D, art: Art, n: TreeNode, isAlloc: boolean, s: Scene) {
  const hovered = n.id === s.hover;
  const heat = !!s.power && !isAlloc && n.kind !== "ClassStart" && n.kind !== "AscendClassStart";
  const state = isAlloc || hovered || s.power ? "alloc" : s.path.has(n.id) ? "path" : "unalloc";
  const frameState = state === "alloc" ? "Allocated" : state === "path" ? "CanAllocate" : "Normal";
  let base: Parameters<typeof drawSprite>[2];
  let overlay: string | undefined;
  let effect: Parameters<typeof drawSprite>[2];
  switch (n.kind) {
    case "ClassStart":
      overlay = isAlloc && n.classStart ? n.classStart : "PSStartNodeBackgroundInactive";
      break;
    case "AscendClassStart":
      overlay = "AscendancyMiddle";
      break;
    case "Socket":
    case "ClusterSocket": {
      const alt = n.kind === "ClusterSocket";
      const name = alt
        ? { alloc: "JewelSocketAltActive", path: "JewelSocketAltCanAllocate", unalloc: "JewelSocketAltNormal" }[state]
        : { alloc: "JewelFrameAllocated", path: "JewelFrameCanAllocate", unalloc: "JewelFrameUnallocated" }[state];
      base = asset(art, name);
      break;
    }
    case "Mastery": {
      const m = art.masteries[String(n.id)];
      if (n.mastery && m) {
        if (isAlloc) {
          base = art.sprites.masteryActiveSelected?.[m[1]];
          effect = art.sprites.masteryActiveEffect?.[m[2]];
        } else if (hovered) base = art.sprites.masteryConnected?.[m[0]];
        else base = art.sprites.masteryInactive?.[m[0]];
      }
      base ??= art.sprites.mastery?.[n.icon ?? ""];
      break;
    }
    default: {
      const kind = n.kind === "Notable" || n.kind === "Keystone" ? n.kind.toLowerCase() : "normal";
      base = art.sprites[kind + (isAlloc || heat ? "Active" : "Inactive")]?.[n.icon ?? ""];
      if (n.kind === "Keystone") overlay = `KeystoneFrame${frameState === "Normal" ? "Unallocated" : frameState}`;
      else if (n.ascendancy) overlay = `AscendancyFrame${n.kind === "Notable" ? "Large" : "Small"}${frameState}`;
      else if (n.kind === "Notable") overlay = `NotableFrame${frameState === "Normal" ? "Unallocated" : frameState}`;
      else overlay = { alloc: "PSSkillFrameActive", path: "PSSkillFrameHighlighted", unalloc: "PSSkillFrame" }[state];
    }
  }
  if (effect) drawSprite(ctx, art, effect, n.x, n.y);
  if (base && drawSprite(ctx, art, base, n.x, n.y) && heat && s.power) {
    // Tint the icon with its power, as upstream's heat map
    const r = base[3] * 1.33;
    ctx.save();
    ctx.globalCompositeOperation = "multiply";
    ctx.fillStyle = powerColor(s.power, n.id) ?? "#000";
    ctx.beginPath();
    ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
    ctx.fill();
    ctx.restore();
  }
  if (overlay) {
    const o = asset(art, overlay);
    // Sheets still loading: a placeholder ring keeps the node visible
    if (!drawSprite(ctx, art, o, n.x, n.y) && !base) {
      ctx.beginPath();
      ctx.arc(n.x, n.y, (hitRadius[n.kind] ?? 53) * 0.7, 0, 2 * Math.PI);
      ctx.fillStyle = isAlloc ? "#8a6a2c" : "#22252b";
      ctx.fill();
    }
  }
}

function drawShapeNode(ctx: CanvasRenderingContext2D, n: TreeNode, isAlloc: boolean, s: Scene, px: number) {
  const r = shapeRadius[n.kind] ?? 24;
  let fill = "#1d2228";
  if (n.kind === "Mastery") fill = isAlloc ? "#5a4a9a" : "#2a2440";
  else if (n.kind === "Keystone") fill = isAlloc ? "#d8b060" : "#3a3020";
  else if (n.kind === "Notable") fill = isAlloc ? "#d8b060" : "#30302a";
  else if (n.kind === "Socket" || n.kind === "ClusterSocket") fill = isAlloc ? "#4a90c0" : "#1a2a3a";
  else if (n.kind === "ClassStart") fill = isAlloc ? "#806030" : "#202020";
  else if (isAlloc) fill = "#c8a050";
  if (s.power && !isAlloc) fill = powerColor(s.power, n.id) ?? fill;
  ctx.beginPath();
  ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
  ctx.fillStyle = fill;
  ctx.fill();
  ctx.lineWidth = Math.max(4, 1 * px);
  ctx.strokeStyle = n.id === s.hover ? "#ffffff" : s.path.has(n.id) && !isAlloc ? "#78b4ff" : isAlloc ? "#f0d080" : "#555";
  ctx.stroke();
}
