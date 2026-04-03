"use client";

import { useLayoutEffect, useEffect, useRef, useState, useCallback } from "react";
import { createPortal } from "react-dom";
import { Avatar, AvatarImage, AvatarFallback } from "@/components/ui/avatar";

// ── Types ──────────────────────────────────────────────────────────────────

export interface AutocompleteItem {
  id: string;
  label: string;
  description?: string;
  icon?: string | null;
  value: string;
}

export interface AutocompletePopoverProps {
  items: AutocompleteItem[];
  activeIndex: number;
  onSelect: (item: AutocompleteItem) => void;
  onActiveIndexChange: (index: number) => void;
  anchorRef: React.RefObject<HTMLTextAreaElement | null>;
  visible: boolean;
}

// ── Component ──────────────────────────────────────────────────────────────

export function AutocompletePopover({
  items,
  activeIndex,
  onSelect,
  onActiveIndexChange,
  anchorRef,
  visible,
}: AutocompletePopoverProps) {
  const [position, setPosition] = useState<{
    left: number;
    bottom: number;
    width: number;
  } | null>(null);
  const rafRef = useRef<number | null>(null);

  const recalcPosition = useCallback(() => {
    const el = anchorRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPosition({
      left: rect.left,
      bottom: window.innerHeight - rect.top + 8,
      width: rect.width,
    });
  }, [anchorRef]);

  // Recalculate position when visible changes
  useLayoutEffect(() => {
    if (visible && items.length > 0) {
      recalcPosition();
    }
  }, [visible, items.length, recalcPosition]);

  // Scroll/resize listener with rAF throttle
  useEffect(() => {
    if (!visible || items.length === 0) return;

    const handleReposition = () => {
      if (rafRef.current !== null) return;
      rafRef.current = requestAnimationFrame(() => {
        recalcPosition();
        rafRef.current = null;
      });
    };

    window.addEventListener("scroll", handleReposition, { passive: true });
    window.addEventListener("resize", handleReposition, { passive: true });

    return () => {
      window.removeEventListener("scroll", handleReposition);
      window.removeEventListener("resize", handleReposition);
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [visible, items.length, recalcPosition]);

  if (!visible || items.length === 0) return null;

  const dropdown = (
    <div
      className="fixed z-[9999] rounded-xl border border-border bg-card shadow-lg animate-in fade-in-0 duration-150"
      style={
        position
          ? {
              left: position.left,
              bottom: position.bottom,
              width: position.width,
            }
          : undefined
      }
    >
      {items.map((item, index) => (
        <button
          key={item.id}
          id={`autocomplete-item-${item.id}`}
          data-index={index}
          className={`w-full flex items-center gap-3 px-3 py-2 text-sm text-left transition-colors ${
            index === activeIndex ? "bg-accent text-accent-foreground" : "hover:bg-muted/50"
          }`}
          onMouseDown={(e) => {
            e.preventDefault();
            onSelect(item);
          }}
          onMouseEnter={() => onActiveIndexChange(index)}
        >
          {item.icon !== undefined && (
            <Avatar size="sm" className="shrink-0">
              {item.icon && <AvatarImage src={item.icon} alt={item.label} />}
              <AvatarFallback className="text-[10px] font-bold">
                {item.value[0]?.toUpperCase() ?? "?"}
              </AvatarFallback>
            </Avatar>
          )}
          <div className="flex flex-col min-w-0">
            <span className="font-medium truncate">{item.label}</span>
            {item.description && (
              <span className="text-muted-foreground text-xs truncate">{item.description}</span>
            )}
          </div>
        </button>
      ))}
    </div>
  );

  return createPortal(dropdown, document.body);
}
