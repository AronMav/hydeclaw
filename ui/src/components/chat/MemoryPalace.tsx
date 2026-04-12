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
}

interface GraphEdge {
  from: string;
  to: string;
  kind: string;
  source: string;
  target: string;
}

// ── Color palette ────────────────────────────────────────────

const PALETTE = {
  document: { core: "#60a5fa", glow: "rgba(96, 165, 250, 0.5)", text: "#93c5fd" },
  person: { core: "#fb7185", glow: "rgba(251, 113, 133, 0.5)", text: "#fda4af" },
  place: { core: "#4ade80", glow: "rgba(74, 222, 128, 0.5)", text: "#86efac" },
  concept: { core: "#c084fc", glow: "rgba(192, 132, 252, 0.5)", text: "#d8b4fe" },
  event: { core: "#fbbf24", glow: "rgba(251, 191, 36, 0.5)", text: "#fde68a" },
  organization: { core: "#fb923c", glow: "rgba(251, 146, 60, 0.5)", text: "#fdba74" },
  default: { core: "#2dd4bf", glow: "rgba(45, 212, 191, 0.5)", text: "#5eead4" },
  pinned: { ring: "#facc15", glow: "rgba(250, 204, 21, 0.6)" },
} as const;

type PaletteKey = keyof Omit<typeof PALETTE, "pinned">;

function getNodeColors(node: GraphNode) {
  if (node.kind === "document") return PALETTE.document;
  const key = (node.entity_type?.toLowerCase() ?? "default") as PaletteKey;
  return PALETTE[key] ?? PALETTE.default;
}

// ── Theme-aware canvas colors ────────────────────────────────

function isDarkMode(): boolean {
  if (typeof document === "undefined") return true;
  return document.documentElement.classList.contains("dark");
}

function themeColors() {
  const dark = isDarkMode();
  return {
    linkDefault: dark ? "rgba(255, 255, 255, 0.12)" : "rgba(0, 0, 0, 0.1)",
    linkDimmed: dark ? "rgba(255, 255, 255, 0.04)" : "rgba(0, 0, 0, 0.04)",
    linkHighlight: dark ? "rgba(107, 158, 255, 0.4)" : "rgba(59, 100, 200, 0.4)",
    pillBg: dark ? "rgba(0, 0, 0, 0.65)" : "rgba(0, 0, 0, 0.7)",
    highlightRing: dark ? "#ffffff" : "#1a1f2e",
    centerDot: dark ? "#ffffff" : "#ffffff",
    dimmedText: dark ? "rgba(255,255,255,0.15)" : "rgba(0,0,0,0.15)",
  };
}

// ── Component ────────────────────────────────────────────────

export function MemoryPalace() {
  const token = useAuthStore((s) => s.token);
  const [data, setData] = useState<{ nodes: GraphNode[]; links: GraphEdge[] } | null>(null);
  const [loading, setLoading] = useState(true);
  const [searchTerm, setSearchTerm] = useState("");
  const [highlightNodes, setHighlightNodes] = useState<Set<string>>(new Set());
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
      setData({ nodes: json.nodes, links });
    } catch {
      toast.error("Failed to load memory graph");
    } finally {
      setLoading(false);
    }
  }, [token]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

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
      const tc = themeColors();
      const isHighlighted = highlightNodes.size > 0 && highlightNodes.has(node.id);
      const isDimmed = highlightNodes.size > 0 && !highlightNodes.has(node.id);
      const isHovered = hoveredRef.current?.id === node.id;
      const time = animFrame.current;

      const baseRadius = node.kind === "document" ? 6 : 5;
      const radius = baseRadius / globalScale;

      // Glow halo
      if (!isDimmed) {
        const glowRadius = radius * (isHovered ? 5 : 3.5);
        const gradient = ctx.createRadialGradient(node.x, node.y, radius * 0.5, node.x, node.y, glowRadius);
        gradient.addColorStop(0, colors.glow);
        gradient.addColorStop(1, "transparent");
        ctx.fillStyle = gradient;
        ctx.beginPath();
        ctx.arc(node.x, node.y, glowRadius, 0, Math.PI * 2);
        ctx.fill();
      }

      // Pinned pulsing ring
      if (node.pinned && !isDimmed) {
        const pulse = Math.sin(time * 0.003) * 0.3 + 0.7;
        ctx.strokeStyle = PALETTE.pinned.ring;
        ctx.lineWidth = 1.5 / globalScale;
        ctx.globalAlpha = pulse;
        ctx.beginPath();
        ctx.arc(node.x, node.y, radius * 2.2, 0, Math.PI * 2);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }

      // Core circle
      ctx.beginPath();
      ctx.arc(node.x, node.y, radius, 0, Math.PI * 2);
      ctx.fillStyle = colors.core;
      ctx.globalAlpha = isDimmed ? 0.15 : 1;
      ctx.fill();
      ctx.globalAlpha = 1;

      // Bright center dot
      if (!isDimmed) {
        ctx.beginPath();
        ctx.arc(node.x, node.y, radius * 0.35, 0, Math.PI * 2);
        ctx.fillStyle = tc.centerDot;
        ctx.globalAlpha = 0.8;
        ctx.fill();
        ctx.globalAlpha = 1;
      }

      // Highlight ring for search matches
      if (isHighlighted) {
        ctx.strokeStyle = tc.highlightRing;
        ctx.lineWidth = 2 / globalScale;
        ctx.beginPath();
        ctx.arc(node.x, node.y, radius * 1.8, 0, Math.PI * 2);
        ctx.stroke();
      }

      // Label — only if zoomed in enough or hovered/highlighted
      if (globalScale > 1.5 || isHovered || isHighlighted) {
        const fontSize = Math.max(10, 12 / globalScale);
        ctx.font = `500 ${fontSize}px system-ui, -apple-system, sans-serif`;
        const label = node.label.length > 28 ? node.label.slice(0, 26) + "\u2026" : node.label;
        const textMetrics = ctx.measureText(label);
        const textW = textMetrics.width;
        const textH = fontSize;
        const padX = 4 / globalScale;
        const padY = 2 / globalScale;
        const labelY = node.y + radius + textH * 0.8;

        // Text background pill
        ctx.fillStyle = tc.pillBg;
        ctx.globalAlpha = isDimmed ? 0.1 : 0.85;
        const pillR = 3 / globalScale;
        const x = node.x - textW / 2 - padX;
        const y = labelY - textH / 2 - padY;
        const w = textW + padX * 2;
        const h = textH + padY * 2;
        ctx.beginPath();
        ctx.moveTo(x + pillR, y);
        ctx.lineTo(x + w - pillR, y);
        ctx.quadraticCurveTo(x + w, y, x + w, y + pillR);
        ctx.lineTo(x + w, y + h - pillR);
        ctx.quadraticCurveTo(x + w, y + h, x + w - pillR, y + h);
        ctx.lineTo(x + pillR, y + h);
        ctx.quadraticCurveTo(x, y + h, x, y + h - pillR);
        ctx.lineTo(x, y + pillR);
        ctx.quadraticCurveTo(x, y, x + pillR, y);
        ctx.closePath();
        ctx.fill();
        ctx.globalAlpha = 1;

        // Text
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillStyle = isDimmed ? tc.dimmedText : colors.text;
        ctx.fillText(label, node.x, labelY);
      }

      ctx.restore();
    },
    [highlightNodes]
  );

  // ── Link colors ────────────────────────────────────────────

  const getLinkColor = useCallback(
    (link: any) => {
      const tc = themeColors();
      if (highlightNodes.size === 0) return tc.linkDefault;
      const srcId = typeof link.source === "object" ? link.source.id : link.source;
      const tgtId = typeof link.target === "object" ? link.target.id : link.target;
      if (highlightNodes.has(srcId) || highlightNodes.has(tgtId)) {
        return tc.linkHighlight;
      }
      return tc.linkDimmed;
    },
    [highlightNodes]
  );

  // ── Hover handler ──────────────────────────────────────────

  const handleNodeHover = useCallback((node: any) => {
    hoveredRef.current = node ?? null;
    setHoveredForUI(node ?? null);
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
      {/* Dot grid background */}
      <div
        className="absolute inset-0 pointer-events-none opacity-[0.03]"
        style={{
          backgroundImage: `radial-gradient(circle, currentColor 1px, transparent 1px)`,
          backgroundSize: "24px 24px",
        }}
      />

      {/* Top controls */}
      <div className="absolute top-3 left-3 z-10 flex items-center gap-2">
        <div className="flex gap-1 p-1 rounded-lg bg-background/80 backdrop-blur-sm border border-border/50 shadow-sm">
          <Button
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            onClick={() => fgRef.current?.zoomToFit(400, 40)}
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
            const color = PALETTE[key]?.core ?? PALETTE.default.core;
            return (
              <div key={type} className="flex items-center justify-between gap-3">
                <div className="flex items-center gap-2">
                  <div
                    className="h-2 w-2 rounded-full"
                    style={{
                      backgroundColor: color,
                      boxShadow: `0 0 6px ${color}`,
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
                  className="h-2 w-2 rounded-full"
                  style={{
                    backgroundColor: PALETTE.pinned.ring,
                    boxShadow: `0 0 6px ${PALETTE.pinned.ring}`,
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
            <div className="flex items-center gap-1.5 mt-1">
              <div
                className="h-1.5 w-1.5 rounded-full"
                style={{ backgroundColor: getNodeColors(hoveredForUI).core }}
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
          const r = (node.kind === "document" ? 5 : 4) / globalScale;
          ctx.fillStyle = color;
          ctx.beginPath();
          ctx.arc(node.x, node.y, r * 2, 0, Math.PI * 2);
          ctx.fill();
        }}
        linkColor={getLinkColor}
        linkWidth={0.5}
        linkDirectionalArrowLength={3}
        linkDirectionalArrowRelPos={1}
        linkDirectionalArrowColor={getLinkColor}
        linkCurvature={0.2}
        onNodeHover={handleNodeHover}
        onNodeClick={(node: any) => {
          if (fgRef.current && node.x != null && node.y != null) {
            fgRef.current.centerAt(node.x, node.y, 600);
            fgRef.current.zoom(3, 600);
          }
        }}
        backgroundColor="transparent"
        warmupTicks={50}
        cooldownTime={3000}
        d3AlphaDecay={0.02}
        d3VelocityDecay={0.3}
      />
    </div>
  );
}
