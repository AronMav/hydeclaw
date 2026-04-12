"use client";

import { useEffect, useState, useRef, useCallback, useMemo } from "react";
import dynamic from "next/dynamic";
import { useAuthStore } from "@/stores/auth-store";
import { Loader2, ZoomIn, ZoomOut, Maximize2, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { toast } from "sonner";

const ForceGraph2D = dynamic(
  () =>
    import("react-force-graph-2d").catch(() => {
      return () => (
        <div className="flex h-full w-full items-center justify-center text-muted-foreground">
          Graph library not loaded
        </div>
      );
    }),
  { ssr: false }
);

// ── Types ────────────────────────────────────────────────────

interface GraphNode {
  id: string;
  kind: "document" | "entity";
  label: string;
  source?: string;
  pinned?: boolean;
  entity_type?: string;
  x?: number;
  y?: number;
  _degree?: number;
}

interface GraphEdge {
  from: string;
  to: string;
  kind: string;
  source: string;
  target: string;
}

// ── Color palette — yFiles dual-tone: dark fill + bright ring ─

const PALETTE = {
  document:     { ring: "#5B8AF5", fill: "#1a2040", text: "#93BBFD" },
  person:       { ring: "#E06C75", fill: "#2a1520", text: "#F0A0A8" },
  place:        { ring: "#98C379", fill: "#152a18", text: "#B8E0A0" },
  concept:      { ring: "#C678DD", fill: "#221530", text: "#DCA8F0" },
  event:        { ring: "#E6C84A", fill: "#2a2510", text: "#F0E090" },
  organization: { ring: "#E07B54", fill: "#2a1810", text: "#F0A880" },
  default:      { ring: "#4ECDC4", fill: "#102828", text: "#80E8E0" },
  pinned:       { ring: "#facc15" },
} as const;

type PaletteKey = keyof Omit<typeof PALETTE, "pinned">;

function getNodeColors(node: GraphNode) {
  if (node.kind === "document") return PALETTE.document;
  const key = (node.entity_type?.toLowerCase() ?? "default") as PaletteKey;
  return PALETTE[key] ?? PALETTE.default;
}

// ── Theme detection ──────────────────────────────────────────

function isDarkMode(): boolean {
  if (typeof document === "undefined") return true;
  return document.documentElement.classList.contains("dark");
}

function getThemeColors() {
  const dark = isDarkMode();
  return {
    edgeDefault: dark ? "rgba(70, 85, 105, 0.35)" : "rgba(70, 85, 105, 0.25)",
    edgeDimmed: dark ? "rgba(70, 85, 105, 0.06)" : "rgba(70, 85, 105, 0.06)",
    edgeHighlight: dark ? "rgba(150, 170, 200, 0.85)" : "rgba(59, 100, 200, 0.5)",
    pillBg: dark ? "rgba(8, 12, 18, 0.75)" : "rgba(0, 0, 0, 0.7)",
    labelColor: dark ? "#CBD5E0" : "#2D3748",
    labelDimmed: dark ? "rgba(203,213,224,0.15)" : "rgba(0,0,0,0.12)",
    searchRing: dark ? "#E2E8F0" : "#1a1f2e",
    particleColor: dark ? "rgba(150, 180, 220, 0.8)" : "rgba(59, 100, 200, 0.6)",
  };
}

// ── Component ────────────────────────────────────────────────

export function MemoryPalace() {
  const token = useAuthStore((s) => s.token);
  const [data, setData] = useState<{ nodes: GraphNode[]; links: GraphEdge[] } | null>(null);
  const [loading, setLoading] = useState(true);
  const [searchTerm, setSearchTerm] = useState("");
  const [highlightNodes, setHighlightNodes] = useState<Set<string>>(new Set());
  const [neighborNodes, setNeighborNodes] = useState<Set<string>>(new Set());
  const fgRef = useRef<any>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const animFrame = useRef(0);
  const hoveredRef = useRef<GraphNode | null>(null);
  const [hoveredForUI, setHoveredForUI] = useState<GraphNode | null>(null);

  // ── Data fetching ──────────────────────────────────────────

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const resp = await fetch("/api/memory/graph", {
        headers: { Authorization: `Bearer ${token}` },
      });
      const json = await resp.json();
      const links = json.edges.map((e: GraphEdge) => ({
        ...e,
        source: e.from,
        target: e.to,
      }));

      // Pre-compute degree for LOD labels
      const degreeMap = new Map<string, number>();
      for (const edge of json.edges) {
        degreeMap.set(edge.from, (degreeMap.get(edge.from) ?? 0) + 1);
        degreeMap.set(edge.to, (degreeMap.get(edge.to) ?? 0) + 1);
      }
      const nodes = json.nodes.map((n: GraphNode) => ({
        ...n,
        _degree: degreeMap.get(n.id) ?? 0,
      }));

      setData({ nodes, links });
    } catch {
      toast.error("Failed to load memory graph");
    } finally {
      setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // ── Adjacency map for neighborhood highlighting ────────────

  const adjacencyMap = useMemo(() => {
    if (!data) return new Map<string, Set<string>>();
    const map = new Map<string, Set<string>>();
    for (const link of data.links) {
      const srcId = typeof link.source === "object" ? (link.source as unknown as GraphNode).id : link.source;
      const tgtId = typeof link.target === "object" ? (link.target as unknown as GraphNode).id : link.target;
      if (!map.has(srcId)) map.set(srcId, new Set());
      if (!map.has(tgtId)) map.set(tgtId, new Set());
      map.get(srcId)!.add(tgtId);
      map.get(tgtId)!.add(srcId);
    }
    return map;
  }, [data]);

  // ── Search highlighting ────────────────────────────────────

  useEffect(() => {
    if (!searchTerm.trim() || !data) {
      setHighlightNodes(new Set());
      return;
    }
    const term = searchTerm.toLowerCase();
    const matched = new Set<string>();
    for (const node of data.nodes) {
      if (
        node.label.toLowerCase().includes(term) ||
        node.entity_type?.toLowerCase().includes(term) ||
        node.source?.toLowerCase().includes(term)
      ) {
        matched.add(node.id);
      }
    }
    setHighlightNodes(matched);
  }, [searchTerm, data]);

  // ── Stats ──────────────────────────────────────────────────

  const stats = useMemo(() => {
    if (!data) return { nodes: 0, edges: 0, documents: 0, entities: 0, pinned: 0 };
    return {
      nodes: data.nodes.length,
      edges: data.links.length,
      documents: data.nodes.filter((n) => n.kind === "document").length,
      entities: data.nodes.filter((n) => n.kind === "entity").length,
      pinned: data.nodes.filter((n) => n.pinned).length,
    };
  }, [data]);

  // ── Entity type legend ─────────────────────────────────────

  const entityTypes = useMemo(() => {
    if (!data) return [];
    const types = new Map<string, number>();
    types.set("document", 0);
    for (const node of data.nodes) {
      if (node.kind === "document") {
        types.set("document", (types.get("document") ?? 0) + 1);
      } else {
        const t = node.entity_type?.toLowerCase() ?? "default";
        types.set(t, (types.get(t) ?? 0) + 1);
      }
    }
    return Array.from(types.entries()).sort((a, b) => b[1] - a[1]);
  }, [data]);

  // ── Animation tick ─────────────────────────────────────────

  useEffect(() => {
    let running = true;
    const tick = () => {
      animFrame.current = performance.now();
      if (running) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return () => {
      running = false;
    };
  }, []);

  // ── Canvas node rendering ──────────────────────────────────

  const paintNode = useCallback(
    (node: any, ctx: CanvasRenderingContext2D, globalScale: number) => {
      if (node.x == null || node.y == null) return;

      ctx.save();

      const colors = getNodeColors(node);
      const tc = getThemeColors();
      const isSearchMatch = highlightNodes.size > 0 && highlightNodes.has(node.id);
      const isSearchDimmed = highlightNodes.size > 0 && !highlightNodes.has(node.id);
      const isHovered = hoveredRef.current?.id === node.id;
      const isNeighbor = neighborNodes.has(node.id);
      const hasHoverFocus = neighborNodes.size > 0;
      const isHoverDimmed = hasHoverFocus && !isNeighbor && !isHovered;
      const isDimmed = isSearchDimmed || isHoverDimmed;
      const time = animFrame.current;

      // Node size — scale with degree (sqrt)
      const degree = node._degree ?? 0;
      const baseRadius = 4 + Math.sqrt(degree) * 1.5;
      const clampedRadius = Math.min(baseRadius, 16);
      const radius = clampedRadius / globalScale;

      // Enlarge neighbors slightly on hover
      const displayRadius = isNeighbor && !isHovered ? radius * 1.1 : radius;

      if (isDimmed) {
        // Dimmed node — minimal rendering
        ctx.beginPath();
        ctx.arc(node.x, node.y, displayRadius * 0.7, 0, Math.PI * 2);
        ctx.fillStyle = colors.ring;
        ctx.globalAlpha = 0.12;
        ctx.fill();
        ctx.restore();
        return;
      }

      // Hover glow — canvas shadowBlur (real GPU glow)
      if (isHovered) {
        ctx.shadowColor = colors.ring;
        ctx.shadowBlur = 24;
        ctx.beginPath();
        ctx.arc(node.x, node.y, displayRadius, 0, Math.PI * 2);
        ctx.fillStyle = colors.ring;
        ctx.fill();
        // Second pass for stronger center bloom
        ctx.shadowBlur = 10;
        ctx.fill();
        ctx.shadowBlur = 0;
      }

      // Dark fill circle (yFiles pattern)
      ctx.beginPath();
      ctx.arc(node.x, node.y, displayRadius, 0, Math.PI * 2);
      ctx.fillStyle = colors.fill;
      ctx.globalAlpha = 1;
      ctx.fill();

      // Bright ring
      ctx.strokeStyle = colors.ring;
      ctx.lineWidth = (isHovered ? 2.5 : 1.5) / globalScale;
      ctx.globalAlpha = isNeighbor ? 1 : 0.85;
      ctx.stroke();
      ctx.globalAlpha = 1;

      // Pinned pulsing outer ring
      if (node.pinned) {
        const pulse = Math.sin(time * 0.003) * 0.3 + 0.7;
        ctx.strokeStyle = PALETTE.pinned.ring;
        ctx.lineWidth = 1.5 / globalScale;
        ctx.globalAlpha = pulse;
        ctx.beginPath();
        ctx.arc(node.x, node.y, displayRadius + 4 / globalScale, 0, Math.PI * 2);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }

      // Search match ring
      if (isSearchMatch) {
        ctx.strokeStyle = tc.searchRing;
        ctx.lineWidth = 2.5 / globalScale;
        ctx.beginPath();
        ctx.arc(node.x, node.y, displayRadius + 3 / globalScale, 0, Math.PI * 2);
        ctx.stroke();
      }

      // ── LOD labels ─────────────────────────────────────────
      // zoom < 0.4  → hide all
      // zoom 0.4–1.0 → show only high-degree (>4) or hovered/pinned
      // zoom > 1.0  → show all non-dimmed
      const showLabel =
        isHovered ||
        isSearchMatch ||
        (globalScale > 1.0) ||
        (globalScale > 0.4 && (degree > 4 || node.pinned));

      if (showLabel) {
        const fontSize = Math.max(9, Math.min(14, 12 / globalScale));
        const fontWeight = globalScale < 0.8 ? "600" : "500";
        ctx.font = `${fontWeight} ${fontSize}px system-ui, -apple-system, sans-serif`;

        const maxLen = globalScale < 0.8 ? 18 : 26;
        const label = node.label.length > maxLen ? node.label.slice(0, maxLen - 2) + "\u2026" : node.label;
        const textW = ctx.measureText(label).width;
        const textH = fontSize;
        const padX = 4 / globalScale;
        const padY = 2 / globalScale;
        const labelY = node.y + displayRadius + textH * 0.7;

        // Semi-transparent pill backdrop
        const pillR = 3 / globalScale;
        const px = node.x - textW / 2 - padX;
        const py = labelY - textH / 2 - padY;
        const pw = textW + padX * 2;
        const ph = textH + padY * 2;

        ctx.fillStyle = tc.pillBg;
        ctx.globalAlpha = 0.82;
        ctx.beginPath();
        ctx.moveTo(px + pillR, py);
        ctx.lineTo(px + pw - pillR, py);
        ctx.quadraticCurveTo(px + pw, py, px + pw, py + pillR);
        ctx.lineTo(px + pw, py + ph - pillR);
        ctx.quadraticCurveTo(px + pw, py + ph, px + pw - pillR, py + ph);
        ctx.lineTo(px + pillR, py + ph);
        ctx.quadraticCurveTo(px, py + ph, px, py + ph - pillR);
        ctx.lineTo(px, py + pillR);
        ctx.quadraticCurveTo(px, py, px + pillR, py);
        ctx.closePath();
        ctx.fill();
        ctx.globalAlpha = 1;

        // Label text — use type color for hovered, neutral for rest
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillStyle = isHovered ? colors.text : tc.labelColor;
        ctx.fillText(label, node.x, labelY);
      }

      ctx.restore();
    },
    [highlightNodes, neighborNodes]
  );

  // ── Link colors ────────────────────────────────────────────

  const getLinkColor = useCallback(
    (link: any) => {
      const tc = getThemeColors();
      const srcId = typeof link.source === "object" ? link.source.id : link.source;
      const tgtId = typeof link.target === "object" ? link.target.id : link.target;

      // Search highlighting
      if (highlightNodes.size > 0) {
        if (highlightNodes.has(srcId) || highlightNodes.has(tgtId)) return tc.edgeHighlight;
        return tc.edgeDimmed;
      }

      // Hover neighborhood highlighting
      if (neighborNodes.size > 0) {
        const hovId = hoveredRef.current?.id;
        if (hovId && (srcId === hovId || tgtId === hovId)) return tc.edgeHighlight;
        return tc.edgeDimmed;
      }

      return tc.edgeDefault;
    },
    [highlightNodes, neighborNodes]
  );

  // ── Link width ─────────────────────────────────────────────

  const getLinkWidth = useCallback(
    (link: any) => {
      if (neighborNodes.size === 0 && highlightNodes.size === 0) return 0.5;
      const srcId = typeof link.source === "object" ? link.source.id : link.source;
      const tgtId = typeof link.target === "object" ? link.target.id : link.target;
      const hovId = hoveredRef.current?.id;

      if (hovId && (srcId === hovId || tgtId === hovId)) return 1.5;
      if (highlightNodes.has(srcId) || highlightNodes.has(tgtId)) return 1.2;
      return 0.3;
    },
    [neighborNodes, highlightNodes]
  );

  // ── Hover handler with neighborhood computation ────────────

  const handleNodeHover = useCallback(
    (node: any) => {
      hoveredRef.current = node ?? null;
      setHoveredForUI(node ?? null);

      if (node) {
        const neighbors = adjacencyMap.get(node.id) ?? new Set<string>();
        const expanded = new Set(neighbors);
        expanded.add(node.id);
        setNeighborNodes(expanded);
      } else {
        setNeighborNodes(new Set());
      }
    },
    [adjacencyMap]
  );

  // ── Node drag pinning (Obsidian pattern) ───────────────────

  const handleNodeDragEnd = useCallback((node: any) => {
    node.fx = node.x;
    node.fy = node.y;
  }, []);

  // ── Render ─────────────────────────────────────────────────

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center">
        <div className="flex flex-col items-center gap-3">
          <Loader2 className="h-8 w-8 animate-spin text-primary/60" />
          <span className="text-xs text-muted-foreground">Loading memory graph…</span>
        </div>
      </div>
    );
  }

  if (!data || data.nodes.length === 0) {
    return (
      <div className="flex h-full w-full items-center justify-center">
        <div className="flex flex-col items-center gap-3 text-muted-foreground">
          <div className="h-16 w-16 rounded-full border-2 border-dashed border-muted-foreground/20 flex items-center justify-center">
            <Search className="h-6 w-6 opacity-30" />
          </div>
          <span className="text-sm">No memory data to visualize</span>
        </div>
      </div>
    );
  }

  return (
    <div ref={containerRef} className="relative h-full w-full overflow-hidden bg-background">
      {/* Radial vignette for depth */}
      <div
        className="absolute inset-0 pointer-events-none z-[1]"
        style={{
          background: `radial-gradient(ellipse 70% 70% at 50% 50%, transparent 30%, rgba(4, 6, 10, 0.4) 100%)`,
        }}
      />

      {/* Top controls */}
      <div className="absolute top-3 left-3 z-10 flex items-center gap-2">
        <div className="flex gap-1 p-1 rounded-lg bg-background/80 backdrop-blur-sm border border-border/50 shadow-sm">
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => fgRef.current?.zoomToFit(400, 50)}
            title="Fit to view"
          >
            <Maximize2 className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => {
              const z = fgRef.current?.zoom();
              if (typeof z === "number") fgRef.current.zoom(z * 1.3, 300);
            }}
          >
            <ZoomIn className="h-3.5 w-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => {
              const z = fgRef.current?.zoom();
              if (typeof z === "number") fgRef.current.zoom(z * 0.7, 300);
            }}
          >
            <ZoomOut className="h-3.5 w-3.5" />
          </Button>
        </div>

        <div className="relative">
          <Search className="absolute left-2 top-1/2 -translate-y-1/2 h-3 w-3 text-muted-foreground/60" />
          <Input
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            placeholder="Filter nodes…"
            className="h-7 w-40 pl-7 pr-7 text-xs bg-background/80 backdrop-blur-sm border-border/50"
          />
          {searchTerm && (
            <button
              onClick={() => setSearchTerm("")}
              className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
            >
              <X className="h-3 w-3" />
            </button>
          )}
        </div>

        {highlightNodes.size > 0 && (
          <Badge variant="secondary" className="h-6 text-[10px] bg-primary/10 text-primary border-none">
            {highlightNodes.size} match{highlightNodes.size !== 1 ? "es" : ""}
          </Badge>
        )}
      </div>

      {/* Legend — bottom right */}
      <div className="absolute bottom-3 right-3 z-10">
        <div className="p-3 space-y-1.5 bg-background/80 backdrop-blur-sm border border-border/50 rounded-lg shadow-sm min-w-[140px]">
          {entityTypes.map(([type, count]) => {
            const key = type as PaletteKey;
            const ringColor = PALETTE[key]?.ring ?? PALETTE.default.ring;
            const fillColor = PALETTE[key]?.fill ?? PALETTE.default.fill;
            return (
              <div key={type} className="flex items-center justify-between gap-3">
                <div className="flex items-center gap-2">
                  <div
                    className="h-2.5 w-2.5 rounded-full border-[1.5px]"
                    style={{
                      borderColor: ringColor,
                      backgroundColor: fillColor,
                      boxShadow: `0 0 4px ${ringColor}`,
                    }}
                  />
                  <span className="text-[10px] text-muted-foreground capitalize">{type}</span>
                </div>
                <span className="text-[10px] font-mono text-muted-foreground/60">{count}</span>
              </div>
            );
          })}
          {stats.pinned > 0 && (
            <div className="flex items-center justify-between gap-3 pt-1.5 mt-1.5 border-t border-border/50">
              <div className="flex items-center gap-2">
                <div
                  className="h-2.5 w-2.5 rounded-full border-[1.5px]"
                  style={{
                    borderColor: PALETTE.pinned.ring,
                    backgroundColor: "transparent",
                    boxShadow: `0 0 4px ${PALETTE.pinned.ring}`,
                  }}
                />
                <span className="text-[10px] text-muted-foreground">pinned</span>
              </div>
              <span className="text-[10px] font-mono text-muted-foreground/60">{stats.pinned}</span>
            </div>
          )}
          <div className="pt-1.5 mt-1.5 border-t border-border/50 text-[10px] text-muted-foreground/50 tabular-nums">
            {stats.nodes} nodes · {stats.edges} edges
          </div>
        </div>
      </div>

      {/* Hover tooltip */}
      {hoveredForUI && (
        <div className="absolute top-3 right-3 z-10 max-w-[220px]">
          <div className="p-3 bg-background/90 backdrop-blur-md border border-border/50 rounded-lg shadow-lg">
            <div className="text-xs font-semibold text-foreground truncate">{hoveredForUI.label}</div>
            <div className="flex items-center gap-1.5 mt-1.5">
              <div
                className="h-2 w-2 rounded-full border"
                style={{
                  borderColor: getNodeColors(hoveredForUI).ring,
                  backgroundColor: getNodeColors(hoveredForUI).fill,
                }}
              />
              <span className="text-[10px] text-muted-foreground capitalize">
                {hoveredForUI.kind === "document" ? "document" : hoveredForUI.entity_type ?? "entity"}
              </span>
              {hoveredForUI.pinned && (
                <Badge variant="secondary" className="h-4 text-[9px] px-1 py-0 bg-yellow-500/10 text-yellow-500 border-none ml-1">
                  pinned
                </Badge>
              )}
            </div>
            {hoveredForUI.source && (
              <div className="text-[10px] text-muted-foreground/60 mt-1 truncate">
                {hoveredForUI.source}
              </div>
            )}
            {(hoveredForUI._degree ?? 0) > 0 && (
              <div className="text-[10px] text-muted-foreground/40 mt-0.5">
                {hoveredForUI._degree} connection{hoveredForUI._degree !== 1 ? "s" : ""}
              </div>
            )}
          </div>
        </div>
      )}

      <ForceGraph2D
        ref={fgRef}
        graphData={data}
        nodeLabel=""
        nodeCanvasObject={paintNode}
        nodePointerAreaPaint={(node: any, color, ctx, globalScale) => {
          if (node.x == null || node.y == null) return;
          const degree = node._degree ?? 0;
          const r = Math.min(4 + Math.sqrt(degree) * 1.5, 16) / globalScale;
          ctx.fillStyle = color;
          ctx.beginPath();
          ctx.arc(node.x, node.y, r * 1.5, 0, Math.PI * 2);
          ctx.fill();
        }}
        linkColor={getLinkColor}
        linkWidth={getLinkWidth}
        linkDirectionalArrowLength={3}
        linkDirectionalArrowRelPos={1}
        linkDirectionalArrowColor={getLinkColor}
        linkCurvature={0.25}
        linkDirectionalParticles={1}
        linkDirectionalParticleWidth={2}
        linkDirectionalParticleSpeed={0.004}
        linkDirectionalParticleColor={() => getThemeColors().particleColor}
        onNodeHover={handleNodeHover}
        onNodeClick={(node: any) => {
          if (fgRef.current && node.x != null && node.y != null) {
            fgRef.current.centerAt(node.x, node.y, 400);
            fgRef.current.zoom(2.5, 400);
          }
        }}
        onNodeDragEnd={handleNodeDragEnd}
        backgroundColor="transparent"
        warmupTicks={50}
        cooldownTime={3000}
        d3AlphaDecay={0.02}
        d3VelocityDecay={0.3}
      />
    </div>
  );
}
