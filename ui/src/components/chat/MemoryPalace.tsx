"use client";

import React, { useEffect, useState, useRef } from "react";
import dynamic from "next/dynamic";
import { useAuthStore } from "@/stores/auth-store";
import { Loader2, ZoomIn, ZoomOut, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";

const ForceGraph2D = dynamic(() => import("react-force-graph-2d").catch(() => {
  return () => <div className="flex h-full w-full items-center justify-center text-muted-foreground">Graph library not loaded</div>;
}), {
  ssr: false,
});

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

export function MemoryPalace() {
  const token = useAuthStore((s) => s.token);
  const [data, setData] = useState<{ nodes: GraphNode[]; links: GraphEdge[] } | null>(null);
  const [loading, setLoading] = useState(true);
  const fgRef = useRef<any>(null);

  const fetchData = async () => {
    setLoading(true);
    try {
      const resp = await fetch("/api/memory/graph", {
        headers: { Authorization: `Bearer ${token}` },
      });
      const json = await resp.json();
      
      // Transform edges for ForceGraph2D (needs source/target)
      const links = json.edges.map((e: any) => ({
        ...e,
        source: e.from,
        target: e.to,
      }));
      
      setData({ nodes: json.nodes, links });
    } catch (err) {
      console.error("Failed to fetch graph:", err);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
  }, []);

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-background">
        <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <div className="relative h-full w-full overflow-hidden bg-background">
      <div className="absolute top-4 left-4 z-10 flex gap-2">
        <Button variant="outline" size="icon" onClick={() => fgRef.current?.zoomToFit(400)}>
          <RefreshCw className="h-4 w-4" />
        </Button>
        <Button variant="outline" size="icon" onClick={() => fgRef.current?.zoom(fgRef.current.zoom() * 1.2)}>
          <ZoomIn className="h-4 w-4" />
        </Button>
        <Button variant="outline" size="icon" onClick={() => fgRef.current?.zoom(fgRef.current.zoom() * 0.8)}>
          <ZoomOut className="h-4 w-4" />
        </Button>
      </div>

      <div className="absolute bottom-4 right-4 z-10">
        <div className="p-3 text-[10px] space-y-1 bg-background/80 backdrop-blur-sm border rounded-lg shadow-sm">
          <div className="flex items-center gap-2">
            <div className="h-2 w-2 rounded-full bg-blue-500" />
            <span>Document</span>
          </div>
          <div className="flex items-center gap-2">
            <div className="h-2 w-2 rounded-full bg-emerald-500" />
            <span>Entity</span>
          </div>
          <div className="mt-2 pt-2 border-t text-muted-foreground">
            {data?.nodes.length} nodes, {data?.links.length} edges
          </div>
        </div>
      </div>

      <ForceGraph2D
        ref={fgRef}
        graphData={data || { nodes: [], links: [] }}
        nodeLabel="label"
        nodeAutoColorBy="kind"
        nodeCanvasObject={(node: any, ctx, globalScale) => {
          const label = node.label;
          const fontSize = 12 / globalScale;
          ctx.font = `${fontSize}px Sans-Serif`;
          const textWidth = ctx.measureText(label).width;
          const bckgDimensions = [textWidth, fontSize].map(n => n + fontSize * 0.2); // some padding

          ctx.fillStyle = node.kind === "document" ? "rgba(59, 130, 246, 0.8)" : "rgba(16, 185, 129, 0.8)";
          ctx.beginPath();
          ctx.arc(node.x, node.y, 5 / globalScale, 0, 2 * Math.PI, false);
          ctx.fill();

          ctx.textAlign = "center";
          ctx.textBaseline = "middle";
          ctx.fillStyle = "currentColor";
          ctx.fillText(label, node.x, node.y + 10 / globalScale);

          node.__bckgDimensions = bckgDimensions; // to re-use in nodePointerAreaPaint
        }}
        linkColor={() => "rgba(255, 255, 255, 0.1)"}
        linkDirectionalArrowLength={3.5}
        linkDirectionalArrowRelPos={1}
        linkCurvature={0.25}
      />
    </div>
  );
}
