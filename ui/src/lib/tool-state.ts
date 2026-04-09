import type { ToolPartState } from "@/stores/chat-store";

export function mapToolPartState(state: ToolPartState): "calling" | "running" | "complete" | "error" | "denied" {
  switch (state) {
    case "input-streaming":
      return "calling";
    case "input-available":
      return "running";
    case "output-available":
      return "complete";
    case "output-error":
      return "error";
    case "output-denied":
      return "denied";
  }
}
