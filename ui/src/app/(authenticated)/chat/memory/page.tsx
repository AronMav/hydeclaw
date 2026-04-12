"use client";

import { MemoryPalace } from "@/components/chat/MemoryPalace";

export default function MemoryPalacePage() {
  return (
    <div className="flex flex-col h-[calc(100vh-4rem)] md:h-screen w-full">
      <div className="flex-none p-4 border-b bg-background/50 backdrop-blur-md">
        <h1 className="text-xl font-semibold tracking-tight">Memory Palace</h1>
        <p className="text-sm text-muted-foreground">Explore the temporal graph of memories and entities.</p>
      </div>
      <div className="flex-1 min-h-0">
        <MemoryPalace />
      </div>
    </div>
  );
}
