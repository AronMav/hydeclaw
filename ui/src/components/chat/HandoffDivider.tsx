"use client";

import { RoleAvatar } from "@/app/(authenticated)/chat/ChatThread";
import { useAuthStore } from "@/stores/auth-store";

export function HandoffDivider({ agentName }: { agentName: string }) {
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const iconUrl = agentName && agentIcons[agentName] ? `/uploads/${agentIcons[agentName]}` : null;

  return (
    <div
      role="separator"
      aria-label={`Agent handoff to ${agentName}`}
      className="flex items-center gap-2 px-4 py-2 text-sm"
    >
      <div className="h-px flex-1 bg-border/30" />
      <RoleAvatar role="assistant" iconUrl={iconUrl} agentName={agentName} />
      <span className="font-medium text-primary">{agentName}</span>
      <div className="h-px flex-1 bg-border/30" />
    </div>
  );
}
