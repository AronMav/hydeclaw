"use client";

import { cleanContent } from "@/lib/format";
import { MessageContent } from "@/components/ui/message";

export function TextPart({ text }: { text: string }) {
  const cleaned = cleanContent(text);
  if (!cleaned) return null;
  return (
    <MessageContent
      markdown
      className="prose prose-sm dark:prose-invert max-w-none bg-transparent p-0 overflow-x-auto
        [&_p]:leading-relaxed [&_p]:text-foreground [&_p]:text-[15px]
        [&_pre]:my-4 [&_pre]:border [&_pre]:border-border [&_pre]:bg-muted/50 [&_pre]:shadow-inner [&_pre]:rounded-lg
        [&_table]:block [&_table]:overflow-x-auto [&_table]:w-full
        [&_a]:text-primary [&_a]:font-bold [&_a]:no-underline hover:[&_a]:underline
        [&_li]:text-foreground [&_strong]:text-foreground [&_strong]:font-bold"
    >
      {cleaned}
    </MessageContent>
  );
}
