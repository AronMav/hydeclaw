"use client";

export function ContinuationSeparator() {
  return (
    <div
      role="separator"
      className="flex items-center justify-center gap-2 py-2 text-xs text-muted-foreground/50"
    >
      <hr className="flex-1 border-border/30" />
      <span
        className="opacity-100 transition-opacity duration-[2000ms] delay-[2000ms]"
        ref={(el) => {
          // Trigger the fade after mount by setting opacity-0 in next frame
          if (el) requestAnimationFrame(() => { el.style.opacity = "0"; });
        }}
      >
        ...continued
      </span>
      <hr className="flex-1 border-border/30" />
    </div>
  );
}
