"use client";

import { cleanContent } from "@/lib/format";
import { MessageContent } from "@/components/ui/message";
import { useChatStore, isActiveStream } from "@/stores/chat-store";

const proseClasses = `prose prose-sm dark:prose-invert max-w-none bg-transparent p-0 overflow-x-auto
  [&_p]:leading-relaxed [&_p]:text-foreground [&_p]:text-[15px]
  [&_pre]:my-4 [&_pre]:border [&_pre]:border-border [&_pre]:bg-muted/50 [&_pre]:shadow-inner [&_pre]:rounded-lg
  [&_table]:block [&_table]:overflow-x-auto [&_table]:w-full
  [&_a]:text-primary [&_a]:font-bold [&_a]:no-underline hover:[&_a]:underline
  [&_li]:text-foreground [&_strong]:text-foreground [&_strong]:font-bold`;

export function TextPart({ text }: { text: string }) {
  const streaming = useChatStore((s) => isActiveStream(s.agents[s.currentAgent]?.streamStatus));
  const cleaned = cleanContent(text);
  if (!cleaned) return null;

  // During streaming: render plain text with whitespace preservation (no markdown parsing).
  // After stream ends: full markdown rendering with syntax highlighting.
  // This eliminates "jerky" re-renders from markdown parser rebuilding DOM on every delta.
  if (streaming) {
    return (
      <div className={`${proseClasses} whitespace-pre-wrap`}>
        {cleaned}
      </div>
    );
  }

  return (
    <MessageContent
      markdown
      className={proseClasses}
    >
      {cleaned}
    </MessageContent>
  );
}
