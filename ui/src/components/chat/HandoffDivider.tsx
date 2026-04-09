"use client";

import { RoleAvatar } from "@/app/(authenticated)/chat/ChatThread";

export function HandoffDivider({ agentName }: { agentName: string }) {
  return (
    <div
      role="separator"
      aria-label={`Agent handoff to ${agentName}`}
      className="flex items-center gap-2 px-4 py-2 text-sm"
    >
      <div className="h-px flex-1 bg-border/30" />
      <RoleAvatar role="assistant" agentName={agentName} />
      <span className="font-medium text-primary">{agentName}</span>
      <div className="h-px flex-1 bg-border/30" />
    </div>
  );
}
