/**
 * HydeClaw Channels — unified connector v2.1.
 * Reads channel configuration from Core API (DB).
 * Credentials come from channel config JSON (bot_token field).
 */

import { buildEnvConfig, wsToHttp } from "./config";
import { spawnSessionLoop, type SessionConfig } from "./session";
import { initHealth, startHealthServer } from "./health";
import { getFormattingPrompt } from "./formatting";

const HEALTH_PORT = Number(process.env.HEALTH_PORT ?? "3000");
const POLL_INTERVAL = Number(process.env.CHANNEL_POLL_INTERVAL ?? "10000"); // ms

interface DbChannel {
  id: string;
  agent_name: string;
  channel_type: string;
  display_name: string;
  config: Record<string, string>;
  status: string;
  error_msg: string | null;
}

// ── Driver Registry ──────────────────────────────────────────────────────────
// Adding a new channel driver requires only one entry here.
// Each driver is lazy-loaded via dynamic import() — unused drivers don't consume startup memory.

type DriverFactory = (config: SessionConfig) => unknown;
type DriverLoader = () => Promise<{ default: DriverFactory } | { [key: string]: DriverFactory }>;

const DRIVER_REGISTRY: Record<string, { module: string; export: string }> = {
  telegram:  { module: "./drivers/telegram",  export: "createTelegramDriver" },
  discord:   { module: "./drivers/discord",   export: "createDiscordDriver" },
  matrix:    { module: "./drivers/matrix",    export: "createMatrixDriver" },
  irc:       { module: "./drivers/irc",       export: "createIrcDriver" },
  slack:     { module: "./drivers/slack",     export: "createSlackDriver" },
  whatsapp:  { module: "./drivers/whatsapp",  export: "createWhatsAppDriver" },
};

async function getDriverFactory(channelType: string): Promise<DriverFactory> {
  const entry = DRIVER_REGISTRY[channelType];
  if (!entry) {
    const available = Object.keys(DRIVER_REGISTRY).join(", ");
    throw new Error(`Unknown channel type: ${channelType}. Available: ${available}`);
  }
  const mod = await import(entry.module);
  return mod[entry.export] as DriverFactory;
}

/** Extract credential (bot token) from channel config JSON. */
function extractCredential(ch: DbChannel): string | null {
  const cfg = ch.config || {};
  return cfg.bot_token || cfg.access_token || cfg.password || null;
}

/** Fetch channels from Core API. */
async function fetchChannels(httpBase: string, authToken: string): Promise<DbChannel[]> {
  try {
    const resp = await fetch(`${httpBase}/api/channels?reveal=true`, {
      headers: { Authorization: `Bearer ${authToken}` },
    });
    if (!resp.ok) {
      console.error(`[api] failed to fetch channels: ${resp.status}`);
      return [];
    }
    const data = (await resp.json()) as { channels: DbChannel[] };
    return data.channels || [];
  } catch (e: any) {
    console.error(`[api] fetch channels error: ${e.message}`);
    return [];
  }
}

// Active sessions keyed by channel DB id
const activeSessions = new Map<string, { controller: AbortController; ch: DbChannel }>();

// FIX #1: Guard against concurrent reconcile runs
let reconciling = false;

function startChannel(ch: DbChannel, envConfig: ReturnType<typeof buildEnvConfig>) {
  const credential = extractCredential(ch);
  if (!credential) {
    console.error(`[${ch.agent_name}] no credential in config for ${ch.channel_type} '${ch.display_name}', skipping`);
    return false;
  }

  const sessionConfig: SessionConfig = {
    agentName: ch.agent_name,
    channelType: ch.channel_type,
    credential,
    coreWs: envConfig.coreWs,
    authToken: envConfig.authToken,
    reconnectInterval: envConfig.reconnectInterval,
    channelConfig: ch.config,
    formattingPrompt: getFormattingPrompt(ch.channel_type),
  };

  const driverFactory = await getDriverFactory(ch.channel_type);
  const controller = spawnSessionLoop(sessionConfig, driverFactory);
  activeSessions.set(ch.id, { controller, ch });
  console.log(`[${ch.agent_name}] ${ch.channel_type} '${ch.display_name}' started`);
  return true;
}

function stopChannel(id: string) {
  const session = activeSessions.get(id);
  if (session) {
    console.log(`[${session.ch.agent_name}] stopping ${session.ch.channel_type} '${session.ch.display_name}'`);
    session.controller.abort();
    activeSessions.delete(id);
  }
}

// FIX #3: Retry ack with backoff instead of catch-all
async function ackChannelStatus(
  httpBase: string, authToken: string, ch: DbChannel, status: "running" | "stopped",
) {
  const url = `${httpBase}/api/agents/${encodeURIComponent(ch.agent_name)}/channels/${ch.id}/ack`;
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      const resp = await fetch(url, {
        method: "POST",
        headers: { Authorization: `Bearer ${authToken}`, "Content-Type": "application/json" },
        body: JSON.stringify({ status }),
      });
      if (resp.ok || resp.status === 404) return; // 404 = channel deleted, skip
      if (resp.status === 401 || resp.status === 403) {
        console.error(`[ack] auth error ${resp.status} for ${ch.id}, not retrying`);
        return;
      }
      // 5xx — retry
      console.warn(`[ack] attempt ${attempt + 1} failed: ${resp.status}`);
    } catch (e: any) {
      console.warn(`[ack] attempt ${attempt + 1} network error: ${e.message}`);
    }
    if (attempt < 2) await new Promise(r => setTimeout(r, (attempt + 1) * 1000));
  }
  console.error(`[ack] failed after 3 attempts for channel ${ch.id}`);
}

async function reconcile(envConfig: ReturnType<typeof buildEnvConfig>) {
  // FIX #1: Prevent overlapping reconciles
  if (reconciling) return;
  reconciling = true;
  try {
    await doReconcile(envConfig);
  } finally {
    reconciling = false;
  }
}

async function doReconcile(envConfig: ReturnType<typeof buildEnvConfig>) {
  const httpBase = wsToHttp(envConfig.coreWs);
  const dbChannels = await fetchChannels(httpBase, envConfig.authToken);
  if (dbChannels.length === 0 && activeSessions.size === 0) return;

  const dbIds = new Set(dbChannels.map(ch => ch.id));

  // Stop removed channels + FIX #4: notify core they're stopped
  for (const [id, session] of activeSessions) {
    if (!dbIds.has(id)) {
      stopChannel(id);
      await ackChannelStatus(httpBase, envConfig.authToken, session.ch, "stopped");
    }
  }

  // Start new channels, restart channels marked pending_restart
  for (const ch of dbChannels) {
    const existing = activeSessions.get(ch.id);
    if (existing) {
      if (ch.status === "pending_restart") {
        console.log(`[${ch.agent_name}] restarting ${ch.channel_type} '${ch.display_name}'`);
        // FIX #1: Stop first, wait a tick for cleanup, then start
        stopChannel(ch.id);
        await new Promise(r => setTimeout(r, 100)); // let abort propagate
        if (startChannel(ch, envConfig)) {
          await ackChannelStatus(httpBase, envConfig.authToken, ch, "running");
        }
      }
      continue;
    }
    // New channel — start it
    if (startChannel(ch, envConfig)) {
      await ackChannelStatus(httpBase, envConfig.authToken, ch, "running");
    }
  }

  // Update health (support multi-channel per agent: join types with comma)
  const agents = [...new Set(dbChannels.map(ch => ch.agent_name))];
  const channelMap: Record<string, string> = {};
  for (const ch of dbChannels) {
    channelMap[ch.agent_name] = channelMap[ch.agent_name]
      ? `${channelMap[ch.agent_name]},${ch.channel_type}`
      : ch.channel_type;
  }
  initHealth(agents, channelMap);
}

async function main() {
  console.log("HydeClaw Channels v2.1.0 starting (DB-driven)...");

  const envConfig = buildEnvConfig(process.env as Record<string, string | undefined>);

  // Initial reconciliation
  await reconcile(envConfig);

  console.log(`[poll] watching for channel changes every ${POLL_INTERVAL / 1000}s`);

  // Start health server
  startHealthServer(HEALTH_PORT);

  // Poll for changes
  setInterval(() => reconcile(envConfig), POLL_INTERVAL);

  // Graceful shutdown — FIX #4: notify core channels are stopped
  const shutdown = async () => {
    console.log("Shutting down...");
    const httpBase = wsToHttp(envConfig.coreWs);
    for (const [id, session] of activeSessions) {
      stopChannel(id);
      await ackChannelStatus(httpBase, envConfig.authToken, session.ch, "stopped").catch(() => {});
    }
    process.exit(0);
  };
  process.on("SIGTERM", () => shutdown());
  process.on("SIGINT", () => shutdown());
}

main().catch((err) => {
  console.error("Fatal error:", err);
  process.exit(1);
});
