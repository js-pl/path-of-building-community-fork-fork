// Passive tree: drawn on a canvas from the engine's tree geometry. Clicking
// a node does what a left click does in upstream's tree view; hovering
// shows upstream's node tooltip, with the stat changes of allocating it.

import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import type { TabProps } from "../app";
import { engine } from "../app";
import type { NodePower, Tree, TreeNode, TreeState, TooltipLine } from "../engine/types";
import { FloatingTooltip } from "./Tooltip";
import { ColorText } from "./ColorText";

const radius: Record<string, number> = {
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

type Mastery = { id: number; options: { label: string; id: number }[] };

export function TreeTab({ act, rev }: TabProps) {
  const [tree, setTree] = useState<Tree | null>(null);
  const [ts, setTs] = useState<TreeState | null>(null);
  const [hover, setHover] = useState<{ node: TreeNode; x: number; y: number } | null>(null);
  const [tip, setTip] = useState<TooltipLine[] | null>(null);
  const [path, setPath] = useState<Set<number>>(new Set());
  const [search, setSearch] = useState("");
  const [powerStats, setPowerStats] = useState<{ stat?: string; label: string }[]>([]);
  const [powerIndex, setPowerIndex] = useState(0);
  const [power, setPower] = useState<NodePower | null>(null);
  const [powerBusy, setPowerBusy] = useState(false);
  const [mastery, setMastery] = useState<Mastery | null>(null);
  const [specs, setSpecs] = useState<{ active: number; list: string[] } | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const view = useRef<View | null>(null);
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  const [, redraw] = useState(0);

  useEffect(() => {
    (async () => {
      const [t, s, sp] = await Promise.all([
        engine.call<Tree>("tree"),
        engine.call<TreeState>("treeState"),
        engine.call<{ active: number; list: string[] }>("treeSpecs"),
      ]);
      setTree(t);
      setTs(s);
      setSpecs(sp);
    })();
  }, [rev]);

  useEffect(() => {
    engine.call<{ stat?: string; label: string }[]>("powerStats").then(setPowerStats);
  }, []);

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

  // Fit the tree into the canvas the first time
  useEffect(() => {
    const c = canvas.current;
    if (!c || !tree || view.current) return;
    const b = tree.bounds;
    const scale = Math.min(c.clientWidth / (b.maxX - b.minX), c.clientHeight / (b.maxY - b.minY)) * 0.95;
    view.current = { scale, x: c.clientWidth / 2 - ((b.minX + b.maxX) / 2) * scale, y: c.clientHeight / 2 - ((b.minY + b.maxY) / 2) * scale };
    redraw((n) => n + 1);
  }, [tree]);

  useEffect(() => {
    const c = canvas.current;
    if (!c || !tree || !ts || !view.current) return;
    draw(c, view.current, tree, byId, alloc, ts, path, matches, power);
  });

  useEffect(() => {
    const onResize = () => redraw((n) => n + 1);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const toTree = (e: MouseEvent) => {
    const c = canvas.current!;
    const r = c.getBoundingClientRect();
    const v = view.current!;
    return { x: (e.clientX - r.left - v.x) / v.scale, y: (e.clientY - r.top - v.y) / v.scale };
  };

  const nodeAt = (e: MouseEvent): TreeNode | null => {
    if (!tree || !view.current) return null;
    const p = toTree(e);
    let best: TreeNode | null = null;
    let bestD = Infinity;
    const minR = 12 / view.current.scale;
    for (const n of tree.nodes) {
      const d = Math.hypot(n.x - p.x, n.y - p.y);
      const r = Math.max(radius[n.kind] ?? 24, minR) * 1.2;
      if (d < r && d < bestD) {
        best = n;
        bestD = d;
      }
    }
    return best;
  };

  // Tooltip and path of the hovered node, fetched after a short pause
  useEffect(() => {
    if (!hover) {
      setTip(null);
      setPath(new Set());
      return;
    }
    let stale = false;
    const id = hover.node.id;
    const t = setTimeout(async () => {
      const [tt, p] = await Promise.all([
        engine.call<{ lines: TooltipLine[] }>("nodeTooltip", { id }),
        engine.call<{ path: number[] }>("nodePath", { id }),
      ]);
      if (!stale) {
        setTip(tt.lines);
        setPath(new Set(p.path));
      }
    }, 120);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [hover?.node.id, rev]);

  const onMove = (e: MouseEvent) => {
    if (drag.current) {
      const v = view.current!;
      v.x += e.clientX - drag.current.x;
      v.y += e.clientY - drag.current.y;
      drag.current.moved ||= Math.abs(e.clientX - drag.current.x) + Math.abs(e.clientY - drag.current.y) > 2;
      drag.current.x = e.clientX;
      drag.current.y = e.clientY;
      redraw((n) => n + 1);
      return;
    }
    const n = nodeAt(e);
    if (n?.id !== hover?.node.id) setHover(n ? { node: n, x: e.clientX, y: e.clientY } : null);
    else if (n && hover) setHover({ node: n, x: e.clientX, y: e.clientY });
  };

  const onUp = async (e: MouseEvent) => {
    const d = drag.current;
    drag.current = null;
    if (d?.moved) return;
    const n = nodeAt(e);
    if (!n) return;
    const r = await act<{ mastery?: { label: string; id: number }[] }>("clickNode", { id: n.id });
    if (r?.mastery) setMastery({ id: n.id, options: r.mastery });
  };

  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const v = view.current;
    if (!v) return;
    const c = canvas.current!.getBoundingClientRect();
    const mx = e.clientX - c.left;
    const my = e.clientY - c.top;
    const f = Math.exp(-e.deltaY * 0.0015);
    v.x = mx - (mx - v.x) * f;
    v.y = my - (my - v.y) * f;
    v.scale *= f;
    redraw((n) => n + 1);
  };

  return (
    <div class="tree-tab">
      <div class="tree-toolbar">
        {specs && specs.list.length > 1 ? (
          <select value={String(specs.active)} onChange={(e) => act("selectTreeSpec", { index: Number((e.target as HTMLSelectElement).value) })}>
            {specs.list.map((t, i) => (
              <option key={i} value={String(i + 1)}>
                {t}
              </option>
            ))}
          </select>
        ) : null}
        <input type="search" placeholder="Search nodes" value={search} onInput={(e) => setSearch((e.target as HTMLInputElement).value)} />
        <label>
          Node power:{" "}
          <select value={String(powerIndex)} onChange={(e) => setPowerIndex(Number((e.target as HTMLSelectElement).value))}>
            <option value="0">Off</option>
            {powerStats.map((s, i) => (
              <option key={i} value={String(i + 1)}>
                {s.label}
              </option>
            ))}
          </select>
        </label>
        {powerBusy ? <span class="busy">calculating node power...</span> : null}
        <button onClick={() => act("resetTree")}>Reset tree</button>
        <button onClick={() => act("undo")}>Undo</button>
      </div>
      <canvas
        ref={canvas}
        class="tree-canvas"
        onMouseDown={(e) => (drag.current = { x: e.clientX, y: e.clientY, moved: false })}
        onMouseMove={onMove}
        onMouseUp={onUp}
        onMouseLeave={() => {
          drag.current = null;
          setHover(null);
        }}
        onWheel={onWheel}
      />
      {hover && tip ? <FloatingTooltip lines={tip} x={hover.x} y={hover.y} /> : null}
      {mastery ? (
        <div class="modal-back" onClick={() => setMastery(null)}>
          <div class="modal" onClick={(e) => e.stopPropagation()}>
            <h3>Choose a mastery effect</h3>
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

function powerColor(p: NodePower, id: number): string | null {
  const n = p.nodes[String(id)];
  if (!n) return null;
  if (p.combined) {
    const o = p.max.offence ? Math.max(0, (n.offence ?? 0) / p.max.offence) : 0;
    const d = p.max.defence ? Math.max(0, (n.defence ?? 0) / p.max.defence) : 0;
    if (o <= 0 && d <= 0) return null;
    return `rgb(${Math.round(255 * Math.sqrt(o))}, 0, ${Math.round(255 * Math.sqrt(d))})`;
  }
  const s = p.max.single ? (n.single ?? 0) / p.max.single : 0;
  if (s <= 0) return null;
  return `rgb(${Math.round(255 * Math.sqrt(s))}, 0, 0)`;
}

function draw(
  c: HTMLCanvasElement,
  v: View,
  tree: Tree,
  byId: Map<number, TreeNode>,
  alloc: Set<number>,
  ts: TreeState,
  path: Set<number>,
  matches: Set<number> | null,
  power: NodePower | null,
) {
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth;
  const h = c.clientHeight;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
  }
  const ctx = c.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.fillStyle = "#0b0d10";
  ctx.fillRect(0, 0, w, h);
  ctx.setTransform(dpr * v.scale, 0, 0, dpr * v.scale, dpr * v.x, dpr * v.y);
  const px = 1 / v.scale;
  const otherAscendancy = (a?: string) => a && a !== ts.ascendancy && a !== ts.secondaryAscendancy;

  // Connectors
  for (const conn of tree.connectors) {
    const a = byId.get(conn.a);
    const b = byId.get(conn.b);
    if (!a || !b) continue;
    const active = alloc.has(a.id) && alloc.has(b.id);
    const inPath = (alloc.has(a.id) || path.has(a.id)) && (alloc.has(b.id) || path.has(b.id)) && (path.has(a.id) || path.has(b.id));
    ctx.strokeStyle = active ? "#c8a050" : inPath ? "#6fa8dc" : otherAscendancy(conn.ascendancy) ? "#262626" : "#3a3a3a";
    ctx.lineWidth = Math.max(active || inPath ? 9 : 6, 1.5 * px);
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
    ctx.stroke();
  }

  // Nodes
  for (const n of tree.nodes) {
    const r = radius[n.kind] ?? 24;
    const isAlloc = alloc.has(n.id);
    const faded = otherAscendancy(n.ascendancy);
    let fill = "#1d2228";
    if (n.kind === "Mastery") fill = isAlloc ? "#5a4a9a" : "#2a2440";
    else if (n.kind === "Keystone") fill = isAlloc ? "#d8b060" : "#3a3020";
    else if (n.kind === "Notable") fill = isAlloc ? "#d8b060" : "#30302a";
    else if (n.kind === "Socket" || n.kind === "ClusterSocket") fill = isAlloc ? "#4a90c0" : "#1a2a3a";
    else if (n.kind === "ClassStart") fill = isAlloc ? "#806030" : "#202020";
    else if (isAlloc) fill = "#c8a050";
    if (power && !isAlloc) fill = powerColor(power, n.id) ?? fill;
    ctx.globalAlpha = faded ? 0.35 : 1;
    ctx.beginPath();
    ctx.arc(n.x, n.y, r, 0, 2 * Math.PI);
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.lineWidth = Math.max(4, 1 * px);
    ctx.strokeStyle = path.has(n.id) && !isAlloc ? "#6fa8dc" : isAlloc ? "#f0d080" : "#555";
    ctx.stroke();
    if (matches?.has(n.id)) {
      ctx.beginPath();
      ctx.arc(n.x, n.y, r + 18, 0, 2 * Math.PI);
      ctx.lineWidth = Math.max(8, 2 * px);
      ctx.strokeStyle = "#ff4040";
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;

  // Names of notables and keystones when zoomed in
  if (v.scale > 0.12) {
    ctx.fillStyle = "#ddd";
    ctx.textAlign = "center";
    ctx.font = `${12 * px}px sans-serif`;
    for (const n of tree.nodes) {
      if ((n.kind === "Keystone" || n.kind === "Notable" || (v.scale > 0.3 && n.kind !== "Mastery")) && n.name) {
        ctx.fillText(n.name, n.x, n.y + (radius[n.kind] ?? 24) + 14 * px);
      }
    }
  }
}
