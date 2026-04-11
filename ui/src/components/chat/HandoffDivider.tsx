"use client";

import { RoleAvatar } from "@/app/(authenticated)/chat/ChatThread";
import { useAuthStore } from "@/stores/auth-store";

export function HandoffDivider({ agentName }: { agentName: string }) {
  const agentIcons = useAuthStore((s) => s.agentIcons);
  const iconUrl = agentName && agentIcons[agentName] ? `/uploads/${agentIcons[agentName]}` : null;

  return (
    <div
      role="separator"
      aria-label={`Agent switch to ${agentName}`}
      className="flex items-center gap-2 px-4 py-2 text-sm animate-in fade-in slide-in-from-bottom-2 duration-300 ease-out"
    >
      <div className="h-px flex-1 bg-border/30" />
      <div className="scale-90 hover:scale-100 transition-transform duration-200">
        <RoleAvatar role="assistant" iconUrl={iconUrl} agentName={agentName} />
      </div>
      <span className="font-medium text-primary/80">{agentName}</span>
      <div className="h-px flex-1 bg-border/30" />
    </div>
  );
}
