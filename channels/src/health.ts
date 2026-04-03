/**
 * Health endpoint — GET /health on port 3000.
 */

export interface HealthResponse {
  ok: boolean;
  agents: string[];
  channels: Record<string, string>;
  uptime_seconds: number;
  version: string;
}

export function buildHealthResponse(
  agents: string[],
  channels: Record<string, string>,
  uptimeSeconds: number,
): HealthResponse {
  return {
    ok: true,
    agents,
    channels,
    uptime_seconds: uptimeSeconds,
    version: "1.0.0",
  };
}

let startTime: number;
let agentsRef: string[] = [];
let channelsRef: Record<string, string> = {};

export function initHealth(
  agents: string[],
  channels: Record<string, string>,
): void {
  if (!startTime) startTime = Date.now(); // only set once
  agentsRef = agents;
  channelsRef = channels;
}

export function startHealthServer(port = 3000): void {
  Bun.serve({
    port,
    fetch(req) {
      const url = new URL(req.url);
      if (req.method === "GET" && url.pathname === "/health") {
        const uptime = Math.floor((Date.now() - startTime) / 1000);
        const body = buildHealthResponse(agentsRef, channelsRef, uptime);
        return new Response(JSON.stringify(body), {
          headers: { "Content-Type": "application/json" },
        });
      }
      return new Response("Not Found", { status: 404 });
    },
  });
  console.log(`Health server listening on :${port}`);
}
