// ── Channel Connector Protocol (Core ↔ Adapter over WebSocket) ──
// Port of crates/hydeclaw-types/src/lib.rs:138-325

export type MediaType = "image" | "audio" | "video" | "document";

export interface MediaAttachment {
  url: string;
  media_type: MediaType;
  mime_type?: string;
  file_name?: string;
  file_size?: number;
}

export const PHASES = {
  THINKING: "thinking",
  CALLING_TOOL: "calling_tool",
  COMPOSING: "composing",
} as const;

export interface IncomingMessageDto {
  user_id: string;
  display_name?: string;
  text?: string;
  attachments: MediaAttachment[];
  context: Record<string, unknown>;
  timestamp: string; // ISO 8601
}

export interface ChannelActionDto {
  action: string;
  params: Record<string, unknown>;
  context: Record<string, unknown>;
}

// ── ChannelInbound: adapter → core ──

export type ChannelInbound =
  | { type: "message"; request_id: string; msg: IncomingMessageDto }
  | { type: "action_result"; action_id: string; success: boolean; error?: string }
  | { type: "access_check"; request_id: string; user_id: string }
  | { type: "pairing_create"; request_id: string; user_id: string; display_name?: string }
  | { type: "pairing_approve"; request_id: string; code: string }
  | { type: "pairing_reject"; request_id: string; code: string }
  | { type: "ping" }
  | { type: "ready"; adapter_type: string; version: string; formatting_prompt?: string }
  | { type: "cancel"; request_id: string };

// ── ChannelOutbound: core → adapter ──

export type ChannelOutbound =
  | { type: "chunk"; request_id: string; text: string }
  | { type: "phase"; request_id: string; phase: string; tool_name?: string }
  | { type: "done"; request_id: string; text: string }
  | { type: "error"; request_id: string; message: string }
  | { type: "action"; action_id: string; action: ChannelActionDto }
  | { type: "access_result"; request_id: string; allowed: boolean; is_owner: boolean }
  | { type: "pairing_code"; request_id: string; code: string }
  | { type: "pairing_result"; request_id: string; success: boolean; error?: string }
  | { type: "pong" }
  | { type: "reload" }
  | { type: "config"; language: string; owner_id?: string; typing_mode: string };

export const CHANNEL_TYPES = ["telegram", "discord", "matrix", "irc", "slack", "whatsapp"] as const;
export type ChannelType = (typeof CHANNEL_TYPES)[number];
