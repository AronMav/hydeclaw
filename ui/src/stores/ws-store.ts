import { create } from "zustand";
import { devtools } from "zustand/middleware";
import { WsManager } from "@/lib/ws";

export type WsStatus = "disconnected" | "connecting" | "connected" | "error";

interface WsState {
  ws: WsManager | null;
  connected: boolean;
  wsStatus: WsStatus;
  connect: (token: string) => void;
  disconnect: () => void;
}

function getWsUrl(): string {
  const loc = typeof window !== "undefined" ? window.location : null;
  if (!loc) return "ws://localhost:18789/ws";
  const proto = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${loc.host}/ws`;
}

export const useWsStore = create<WsState>()(
  devtools(
    (set, get) => ({
      ws: null,
      connected: false,
      wsStatus: "disconnected" as WsStatus,

      connect: (token: string) => {
        const existing = get().ws;
        if (existing) existing.disconnect();

        const ws = new WsManager(getWsUrl(), token);
        ws.addConnectionListener((connected) => {
          set({ connected, wsStatus: connected ? "connected" : "error" });
        });
        void ws.connect();
        set({ ws, connected: false, wsStatus: "connecting" });
      },

      disconnect: () => {
        const ws = get().ws;
        if (ws) {
          ws.disconnect();
          set({ ws: null, connected: false, wsStatus: "disconnected" });
        }
      },
    }),
    { name: "WsStore", enabled: process.env.NODE_ENV !== "production" },
  ),
);
