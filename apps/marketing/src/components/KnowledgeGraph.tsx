import { useEffect, useMemo, useRef, useState, type JSX } from 'react';
import { forceCenter, forceCollide, forceLink, forceManyBody, forceSimulation, type SimulationNodeDatum } from 'd3-force';
import { buildDemoGraphData, type DemoNode } from '../data/demo-graph';

interface LayoutNode extends DemoNode {
  x: number;
  y: number;
  pr: number;
  rank: number;
  ord: number;
}

interface GraphData {
  nodes: LayoutNode[];
  edges: [string, string][];
  adj: Record<string, Set<string>>;
  byId: Record<string, LayoutNode>;
  codeCount: number;
}

interface SimNode extends SimulationNodeDatum {
  id: string;
}

const W = 820;
const H = 540;

/** Layout + centrality, computed once per mount. Position/physics uses the
 * same d3-force setup as the real product's graph (apps/web/src/components
 * /graph/graph-canvas.tsx) instead of hand-rolled physics -- personalized
 * PageRank for hotspot ranking and BFS reveal-order are small, self-contained
 * algorithms d3-force doesn't provide, so those stay hand-rolled. */
function buildGraph(): GraphData {
  const { nodes: rawNodes, edges: rawEdges } = buildDemoGraphData();

  const simNodes: SimNode[] = rawNodes.map((n) => ({ id: n.id }));
  const simLinks = rawEdges.map((e) => ({ source: e.a, target: e.b }));

  const simulation = forceSimulation(simNodes)
    .force('charge', forceManyBody().strength(-90))
    .force('link', forceLink(simLinks).id((d) => (d as SimNode).id).distance(46))
    .force('center', forceCenter(W / 2, H / 2))
    .force('collide', forceCollide(14))
    .stop();
  simulation.tick(300);

  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  for (const n of simNodes) {
    x0 = Math.min(x0, n.x ?? 0);
    x1 = Math.max(x1, n.x ?? 0);
    y0 = Math.min(y0, n.y ?? 0);
    y1 = Math.max(y1, n.y ?? 0);
  }
  const scale = Math.min((W - 150) / (x1 - x0 || 1), (H - 70) / (y1 - y0 || 1));
  const posById: Record<string, { x: number; y: number }> = {};
  for (const n of simNodes) {
    posById[n.id] = {
      x: (W - (x1 - x0) * scale) / 2 + ((n.x ?? 0) - x0) * scale - 20,
      y: (H - (y1 - y0) * scale) / 2 + ((n.y ?? 0) - y0) * scale - 6,
    };
  }

  const adj: Record<string, Set<string>> = {};
  for (const n of rawNodes) adj[n.id] = new Set();
  for (const { a, b } of rawEdges) {
    adj[a].add(b);
    adj[b].add(a);
  }

  // Global PageRank (damping 0.85) over the demo graph -- used only to rank
  // "hotspot" centrality for the overlay, not a personalized/seeded variant.
  const N = rawNodes.length;
  let pr: Record<string, number> = {};
  for (const n of rawNodes) pr[n.id] = 1 / N;
  for (let k = 0; k < 50; k++) {
    const next: Record<string, number> = {};
    for (const n of rawNodes) next[n.id] = 0.15 / N;
    for (const n of rawNodes) {
      const out = adj[n.id];
      if (out.size === 0) continue;
      for (const m of out) next[m] += (0.85 * pr[n.id]) / out.size;
    }
    pr = next;
  }
  const codeNodes = rawNodes.filter((n) => n.kind === 'file' || n.kind === 'symbol');
  const ranked = [...codeNodes].sort((a, b) => pr[b.id] - pr[a.id]);
  const rankPos: Record<string, number> = {};
  ranked.forEach((n, i) => (rankPos[n.id] = i));
  const prVals = rawNodes.map((n) => pr[n.id]);
  const prMin = Math.min(...prVals);
  const prMax = Math.max(...prVals);

  // BFS order from `server`, used only to stagger the reveal-on-scan animation.
  const order: Record<string, number> = {};
  const queue = ['server'];
  order.server = 0;
  let o = 1;
  while (queue.length) {
    const cur = queue.shift()!;
    for (const m of adj[cur]) {
      if (order[m] == null) {
        order[m] = o++;
        queue.push(m);
      }
    }
  }

  const byId: Record<string, LayoutNode> = {};
  const nodes: LayoutNode[] = rawNodes.map((n) => {
    const node: LayoutNode = {
      ...n,
      x: posById[n.id].x,
      y: posById[n.id].y,
      pr: (pr[n.id] - prMin) / (prMax - prMin || 1),
      rank: rankPos[n.id] ?? codeNodes.length,
      ord: order[n.id] ?? o++,
    };
    byId[n.id] = node;
    return node;
  });

  return { nodes, edges: rawEdges.map((e) => [e.a, e.b]), adj, byId, codeCount: codeNodes.length };
}

interface Theme {
  edge: string;
  edgeHi: string;
  file: string;
  fileFill: string;
  sym: string;
  g: string;
  d: string;
  hot: string;
  hot2: string;
  cold: string;
  label: string;
  labelHi: string;
  halo: string;
  ring: string;
}

const DARK: Theme = {
  edge: '#45475a', edgeHi: '#a6adc8', file: '#7f849c', fileFill: '#181825', sym: '#b4befe',
  g: '#fab387', d: '#94e2d5', hot: '#fab387', hot2: '#f9e2af', cold: '#585b70',
  label: '#7f849c', labelHi: '#cdd6f4', halo: '#11111b', ring: '#cdd6f4',
};

const LIGHT: Theme = {
  edge: '#45475a', edgeHi: '#cdd6f4', file: '#9399b2', fileFill: '#1e1e2e', sym: '#b4befe',
  g: '#fab387', d: '#94e2d5', hot: '#fab387', hot2: '#f9e2af', cold: '#585b70',
  label: '#9399b2', labelHi: '#cdd6f4', halo: '#1e1e2e', ring: '#cdd6f4',
};

export interface KnowledgeGraphProps {
  dark?: boolean;
  animate?: boolean;
}

export default function KnowledgeGraph({ dark = false, animate = true }: KnowledgeGraphProps) {
  const graph = useMemo(() => buildGraph(), []);
  const [pos, setPos] = useState<Record<string, { x: number; y: number }>>(() => {
    const o: Record<string, { x: number; y: number }> = {};
    for (const n of graph.nodes) o[n.id] = { x: n.x, y: n.y };
    return o;
  });
  const [selected, setSelected] = useState('hybrid_search');
  const [hover, setHover] = useState<string | null>(null);
  const [hot, setHot] = useState(true);
  const [showNotes, setShowNotes] = useState(true);
  const [depth, setDepth] = useState<number | 'all'>('all');
  const [reveal, setReveal] = useState(animate ? 0 : graph.nodes.length + 1);

  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<{ id: string; sx: number; sy: number; moved: boolean } | null>(null);

  useEffect(() => {
    if (!animate) return;
    let i = 0;
    const t = setInterval(() => {
      i += 1;
      setReveal(i);
      if (i > graph.nodes.length) clearInterval(t);
    }, 38);
    return () => clearInterval(t);
  }, [animate, graph.nodes.length]);

  const T = dark ? DARK : LIGHT;

  let vis = new Set(graph.nodes.filter((n) => showNotes || (n.kind !== 'gotcha' && n.kind !== 'decision')).map((n) => n.id));
  if (depth !== 'all' && vis.has(selected)) {
    const keep = new Set([selected]);
    let frontier = [selected];
    for (let k = 0; k < depth; k++) {
      const next: string[] = [];
      for (const id of frontier) {
        for (const m of graph.adj[id]) {
          if (vis.has(m) && !keep.has(m)) {
            keep.add(m);
            next.push(m);
          }
        }
      }
      frontier = next;
    }
    vis = keep;
  }

  const focus = hover || selected;
  const fset = new Set(focus ? [focus, ...graph.adj[focus]] : []);
  const hotRank = graph.codeCount;

  const fillFor = (n: LayoutNode) => {
    if (n.kind === 'gotcha') return T.g;
    if (n.kind === 'decision') return T.d;
    if (!hot) return n.kind === 'file' ? T.fileFill : T.sym;
    if (n.rank < hotRank * 0.12) return T.hot;
    if (n.rank < hotRank * 0.32) return T.hot2;
    return n.kind === 'file' ? T.fileFill : T.cold;
  };

  const toSvg = (e: React.PointerEvent) => {
    const s = svgRef.current!;
    const pt = s.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    return pt.matrixTransform(s.getScreenCTM()!.inverse());
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    if (Math.abs(e.clientX - d.sx) + Math.abs(e.clientY - d.sy) > 4) d.moved = true;
    if (d.moved) {
      const pt = toSvg(e);
      setPos((prev) => ({ ...prev, [d.id]: { x: Math.max(10, Math.min(810, pt.x)), y: Math.max(10, Math.min(530, pt.y)) } }));
    }
  };

  const onUp = () => {
    const d = drag.current;
    if (d && !d.moved) setSelected(d.id);
    drag.current = null;
  };

  const edgeEls = graph.edges.map(([a, b], i) => {
    if (!vis.has(a) || !vis.has(b)) return null;
    const A = pos[a];
    const B = pos[b];
    const on = !!focus && (a === focus || b === focus);
    const shown = reveal > graph.byId[a].ord && reveal > graph.byId[b].ord;
    return (
      <line
        key={`e${i}`}
        x1={A.x}
        y1={A.y}
        x2={B.x}
        y2={B.y}
        stroke={on ? T.edgeHi : T.edge}
        strokeWidth={on ? 1.4 : 1}
        opacity={shown ? (focus && !on ? 0.45 : 1) : 0}
        style={{ transition: 'opacity .5s' }}
      />
    );
  });

  const nodeEls: JSX.Element[] = [];
  const labelEls: JSX.Element[] = [];
  for (const n of graph.nodes) {
    if (!vis.has(n.id)) continue;
    const P = pos[n.id];
    const shown = reveal > n.ord;
    const dim = !!focus && !fset.has(n.id);
    const op = shown ? (dim ? 0.3 : 1) : 0;
    const fill = fillFor(n);
    const r = n.kind === 'symbol' ? 3.2 + n.pr * 6 : 5.5;

    const kids: JSX.Element[] = [];
    if (hot && (n.kind === 'file' || n.kind === 'symbol') && n.rank < 3) {
      kids.push(
        <circle
          key="pulse"
          cx={P.x}
          cy={P.y}
          r={r + 2}
          fill="none"
          stroke={T.hot}
          strokeWidth={1.2}
          style={{
            transformBox: 'view-box',
            transformOrigin: `${P.x}px ${P.y}px`,
            animation: 'aoPulse 2.4s ease-out infinite',
            animationDelay: `${n.rank * 0.6}s`,
            pointerEvents: 'none',
          }}
        />,
      );
    }
    if (n.id === selected) {
      kids.push(<circle key="sel" cx={P.x} cy={P.y} r={r + 6} fill="none" stroke={T.ring} strokeWidth={1} strokeDasharray="2 2.5" />);
    }
    kids.push(<circle key="hit" cx={P.x} cy={P.y} r={r + 7} fill="transparent" />);
    if (n.kind === 'file') {
      kids.push(<rect key="shape" x={P.x - 5} y={P.y - 5} width={10} height={10} fill={fill} stroke={T.file} strokeWidth={1.5} />);
    } else if (n.kind === 'symbol') {
      kids.push(<circle key="shape" cx={P.x} cy={P.y} r={r} fill={fill} />);
    } else {
      kids.push(<rect key="shape" x={P.x - 4.5} y={P.y - 4.5} width={9} height={9} fill={fill} transform={`rotate(45 ${P.x} ${P.y})`} />);
    }

    nodeEls.push(
      <g
        key={n.id}
        opacity={op}
        style={{ transition: 'opacity .45s', cursor: 'grab' }}
        onPointerDown={(e) => {
          drag.current = { id: n.id, sx: e.clientX, sy: e.clientY, moved: false };
          svgRef.current?.setPointerCapture(e.pointerId);
        }}
        onPointerEnter={() => setHover(n.id)}
        onPointerLeave={() => setHover(null)}
      >
        {kids}
      </g>,
    );

    const showLabel = shown && (n.kind === 'file' || fset.has(n.id));
    if (showLabel) {
      const hi = fset.has(n.id);
      labelEls.push(
        <text
          key={`l${n.id}`}
          x={P.x + (n.kind === 'symbol' ? r + 5 : 10)}
          y={P.y + 3.5}
          fontSize={n.kind === 'file' ? 10.5 : 10}
          fontFamily="'JetBrains Mono', monospace"
          fontWeight={n.kind === 'file' ? 600 : 400}
          fill={hi ? T.labelHi : T.label}
          stroke={T.halo}
          strokeWidth={3.5}
          paintOrder="stroke"
          opacity={dim ? 0.35 : 1}
          style={{ pointerEvents: 'none', transition: 'opacity .3s' }}
        >
          {n.label}
        </text>,
      );
    }
  }

  const count = Math.min(reveal, graph.nodes.length);
  const statusText = `${count < graph.nodes.length ? 'scanning… ' : 'scanned · '}${count}/${graph.nodes.length} nodes · ${graph.edges.length} edges`;

  const sel = graph.byId[selected];
  const selAtt = [...graph.adj[selected]]
    .map((id) => graph.byId[id])
    .filter((m) => (sel.kind === 'gotcha' || sel.kind === 'decision' ? m.kind === 'symbol' : m.kind === 'gotcha' || m.kind === 'decision'));
  const selColor = (k: string) => (k === 'gotcha' ? T.g : k === 'decision' ? T.d : T.labelHi);

  return (
    <div style={{ display: 'flex', flexWrap: 'wrap' }}>
      <div style={{ flex: '3 1 560px', minWidth: 0, position: 'relative' }}>
        <svg
          ref={svgRef}
          viewBox={`0 0 ${W} ${H}`}
          style={{ display: 'block', width: '100%', height: 'auto', touchAction: 'none', userSelect: 'none' }}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={onUp}
        >
          {edgeEls}
          {nodeEls}
          {labelEls}
          <text x={14} y={528} fontSize={10} fontFamily="'JetBrains Mono', monospace" fill={T.label}>
            {statusText}
          </text>
        </svg>
        <div
          style={{
            position: 'absolute',
            left: 16,
            bottom: 30,
            display: 'flex',
            flexWrap: 'wrap',
            gap: 14,
            fontFamily: "'JetBrains Mono', monospace",
            fontSize: 11,
            color: '#a6adc8',
          }}
        >
          <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 9, height: 9, border: '1.5px solid #9399b2' }} />file</span>
          <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 8, height: 8, borderRadius: '50%', background: '#b4befe' }} />symbol</span>
          <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 8, height: 8, background: '#fab387', transform: 'rotate(45deg)' }} />gotcha</span>
          <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}><span style={{ width: 8, height: 8, background: '#94e2d5', transform: 'rotate(45deg)' }} />decision</span>
        </div>
        <div style={{ position: 'absolute', top: 12, right: 14, display: 'flex', gap: 6, flexWrap: 'wrap', fontFamily: "'JetBrains Mono', monospace", fontSize: 11.5 }}>
          <button
            onClick={() => setHot((v) => !v)}
            style={{ font: 'inherit', cursor: 'pointer', padding: '6px 12px', borderRadius: 999, border: '1px solid #45475a', background: hot ? '#cba6f7' : 'transparent', color: hot ? '#11111b' : '#cdd6f4' }}
          >
            Hotspots
          </button>
          <button
            onClick={() => setShowNotes((v) => !v)}
            style={{ font: 'inherit', cursor: 'pointer', padding: '6px 12px', borderRadius: 999, border: '1px solid #45475a', background: showNotes ? '#cba6f7' : 'transparent', color: showNotes ? '#11111b' : '#cdd6f4' }}
          >
            Notes
          </button>
          <span style={{ display: 'flex', border: '1px solid #45475a', borderRadius: 999, overflow: 'hidden' }}>
            <span style={{ padding: '6px 10px', color: '#a6adc8' }}>Depth</span>
            {[1, 2, 'all' as const].map((d) => (
              <button
                key={String(d)}
                onClick={() => setDepth(d)}
                style={{ font: 'inherit', cursor: 'pointer', padding: '6px 11px', border: 0, background: depth === d ? '#cba6f7' : 'transparent', color: depth === d ? '#11111b' : '#cdd6f4' }}
              >
                {d === 'all' ? 'All' : d}
              </button>
            ))}
          </span>
        </div>
      </div>
      <div style={{ flex: '1 1 260px', minWidth: 0, borderLeft: '1px solid #313244', background: '#181825', padding: 20, display: 'flex', flexDirection: 'column', gap: 18 }}>
        <span style={{ fontFamily: "'JetBrains Mono', monospace", fontSize: 10.5, letterSpacing: '.08em', textTransform: 'uppercase', color: '#cba6f7' }}>What the agent gets</span>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, padding: 14, borderRadius: 12, background: '#1e1e2e', border: '1px solid #313244' }}>
          <span style={{ fontFamily: "'JetBrains Mono', monospace", fontSize: 10.5, color: '#7f849c', textTransform: 'uppercase', letterSpacing: '.06em' }}>{sel.kind}</span>
          <span style={{ fontFamily: "'JetBrains Mono', monospace", fontSize: 15, fontWeight: 600, overflowWrap: 'anywhere', color: '#cdd6f4' }}>{sel.label}</span>
          <span style={{ fontFamily: "'JetBrains Mono', monospace", fontSize: 11, color: '#a6adc8', overflowWrap: 'anywhere' }}>{sel.path}</span>
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
          <div style={{ padding: '12px 14px', borderRadius: 12, background: '#1e1e2e', border: '1px solid #313244', display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={{ fontSize: 24, fontWeight: 800, letterSpacing: '-0.02em', color: '#fab387' }}>#{sel.rank + 1}</span>
            <span style={{ fontSize: 12, color: '#a6adc8' }}>centrality of {graph.codeCount}</span>
          </div>
          <div style={{ padding: '12px 14px', borderRadius: 12, background: '#1e1e2e', border: '1px solid #313244', display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={{ fontSize: 24, fontWeight: 800, letterSpacing: '-0.02em', color: '#89b4fa' }}>{graph.adj[selected].size}</span>
            <span style={{ fontSize: 12, color: '#a6adc8' }}>direct edges</span>
          </div>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <span style={{ fontFamily: "'JetBrains Mono', monospace", fontSize: 10.5, letterSpacing: '.08em', textTransform: 'uppercase', color: '#7f849c' }}>
            {sel.kind === 'gotcha' || sel.kind === 'decision' ? 'Attached to' : 'Attached notes'}
          </span>
          {selAtt.map((a) => (
            <div key={a.id} style={{ display: 'flex', gap: 10, alignItems: 'flex-start', fontSize: 13, lineHeight: 1.4, color: '#cdd6f4' }}>
              <span style={{ flex: 'none', width: 8, height: 8, marginTop: 5, background: selColor(a.kind), transform: 'rotate(45deg)' }} />
              <span>{a.label}</span>
            </div>
          ))}
          {selAtt.length === 0 && <span style={{ fontSize: 13, color: '#7f849c' }}>None on this node. Try hybrid_search or read_only_filter.</span>}
        </div>
        <span style={{ marginTop: 'auto', fontFamily: "'JetBrains Mono', monospace", fontSize: 11, color: '#6c7086' }}>drag nodes &middot; click to inspect</span>
      </div>
    </div>
  );
}
