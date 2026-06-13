import { useEffect, useLayoutEffect, useRef, useState } from "react";
import ForceGraph2D from "react-force-graph-2d";
import { Network, Loader2, RefreshCw } from "lucide-react";
import { useStore } from "../state/store";
import { api, type PageGraph } from "../lib/ipc";

type GNode = { id: number; title: string; parentId?: number; degree: number; x?: number; y?: number };

/** The Obsidian-style connection graph: every vault page as a node, every wikilink as an edge.
 *  Node size scales with link degree; click a node to open that page. */
export default function GraphPane() {
  const openPage = useStore((s) => s.openPage);
  const [graph, setGraph] = useState<PageGraph | null>(null);
  const [loading, setLoading] = useState(true);
  const [hovered, setHovered] = useState<number | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  const load = () => {
    setLoading(true);
    api
      .pageGraph()
      .then(setGraph)
      .catch(() => setGraph({ nodes: [], edges: [] }))
      .finally(() => setLoading(false));
  };
  useEffect(load, []);

  // Track the container size so the canvas fills the pane and reflows on resize.
  useLayoutEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  // react-force-graph mutates node objects (x/y/velocity), so hand it fresh copies each load.
  const data = graph
    ? {
        nodes: graph.nodes.map((n) => ({ ...n })) as GNode[],
        links: graph.edges.map((e) => ({ source: e.source, target: e.target })),
      }
    : { nodes: [], links: [] };

  const empty = !loading && data.nodes.length === 0;

  return (
    <div className="h-full w-full flex flex-col">
      <div className="h-12 shrink-0 border-b border-white/10 flex items-center justify-between px-4">
        <h1 className="text-sm font-semibold flex items-center gap-2">
          <Network className="size-4 text-indigo-400" /> Connection graph
          {graph && <span className="text-gray-600 font-normal">· {graph.nodes.length} pages, {graph.edges.length} links</span>}
        </h1>
        <button onClick={load} title="Refresh" className="p-1.5 rounded-md text-gray-500 hover:text-white hover:bg-white/5">
          <RefreshCw className={`size-4 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>

      <div ref={wrapRef} className="flex-1 min-h-0 relative">
        {loading && (
          <div className="absolute inset-0 grid place-items-center text-gray-500">
            <Loader2 className="size-5 animate-spin" />
          </div>
        )}
        {empty && (
          <div className="absolute inset-0 grid place-items-center text-gray-500">
            <div className="flex flex-col items-center gap-2 text-center">
              <Network className="size-8 text-gray-600" />
              <p className="text-sm">No pages yet. Create notes and link them with <span className="text-gray-300">[[</span> to see the graph.</p>
            </div>
          </div>
        )}
        {!empty && size.w > 0 && (
          <ForceGraph2D
            width={size.w}
            height={size.h}
            graphData={data}
            backgroundColor="#0b0e14"
            linkColor={() => "rgba(255,255,255,0.12)"}
            linkWidth={1}
            cooldownTicks={120}
            onNodeClick={(node) => openPage((node as GNode).id)}
            onNodeHover={(node) => setHovered(node ? (node as GNode).id : null)}
            nodeCanvasObject={(node, ctx, globalScale) => {
              const n = node as GNode;
              const r = 3 + Math.min(n.degree, 12) * 1.1;
              const isHot = hovered === n.id;
              ctx.beginPath();
              ctx.arc(n.x ?? 0, n.y ?? 0, r, 0, 2 * Math.PI);
              ctx.fillStyle = isHot ? "#a5b4fc" : "#6366f1";
              ctx.fill();
              const fontSize = Math.max(11 / globalScale, 2);
              ctx.font = `${fontSize}px Inter, system-ui, sans-serif`;
              ctx.textAlign = "center";
              ctx.textBaseline = "top";
              ctx.fillStyle = isHot ? "rgba(255,255,255,0.95)" : "rgba(229,231,235,0.7)";
              ctx.fillText(n.title, n.x ?? 0, (n.y ?? 0) + r + 1.5);
            }}
            nodePointerAreaPaint={(node, color, ctx) => {
              const n = node as GNode;
              const r = 3 + Math.min(n.degree, 12) * 1.1;
              ctx.beginPath();
              ctx.arc(n.x ?? 0, n.y ?? 0, r + 2, 0, 2 * Math.PI);
              ctx.fillStyle = color;
              ctx.fill();
            }}
          />
        )}
      </div>
    </div>
  );
}
