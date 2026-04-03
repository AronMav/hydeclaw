"use client";

import { vi, describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

// ── Import tested exports ──────────────────────────────────────────────────

import {
  AutocompletePopover,
  type AutocompleteItem,
} from "./AutocompletePopover";

// ── Helpers ────────────────────────────────────────────────────────────────

function makeMockAnchorRef() {
  return {
    current: {
      getBoundingClientRect: () => ({
        left: 0,
        top: 500,
        width: 400,
        height: 40,
        right: 400,
        bottom: 540,
        x: 0,
        y: 500,
        toJSON: () => {},
      }),
    },
  } as React.RefObject<HTMLTextAreaElement | null>;
}

function makeItems(count: number): AutocompleteItem[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `item-${i}`,
    label: `Label ${i}`,
    description: i % 2 === 0 ? `Description ${i}` : undefined,
    value: `value-${i}`,
  }));
}

// ── Tests ──────────────────────────────────────────────────────────────────

describe("AutocompletePopover", () => {
  it("renders items inside a portal when visible=true", () => {
    const items = makeItems(3);
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );

    // All 3 items should be rendered
    expect(screen.getByText("Label 0")).toBeInTheDocument();
    expect(screen.getByText("Label 1")).toBeInTheDocument();
    expect(screen.getByText("Label 2")).toBeInTheDocument();
  });

  it("renders nothing when visible=false", () => {
    const items = makeItems(3);
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    const { container } = render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={false}
      />
    );

    expect(screen.queryByText("Label 0")).not.toBeInTheDocument();
    expect(screen.queryByText("Label 1")).not.toBeInTheDocument();
  });

  it("renders nothing when items is empty array", () => {
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    render(
      <AutocompletePopover
        items={[]}
        activeIndex={0}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );

    // No buttons should be rendered
    expect(screen.queryAllByRole("button")).toHaveLength(0);
  });

  it("highlights the item at activeIndex with bg-accent", () => {
    const items = makeItems(3);
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    render(
      <AutocompletePopover
        items={items}
        activeIndex={1}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );

    const buttons = screen.getAllByRole("button");
    expect(buttons[1].className).toContain("bg-accent");
    // Non-active items use hover:bg-muted/50 (not static bg-accent)
    expect(buttons[0].className).not.toContain("bg-accent");
  });

  it("calls onSelect with correct item on mouseDown and prevents default", () => {
    const items = makeItems(3);
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );

    const buttons = screen.getAllByRole("button");
    const event = fireEvent.mouseDown(buttons[1]);

    expect(onSelect).toHaveBeenCalledWith(items[1]);
    // fireEvent.mouseDown returns false if preventDefault was called
    expect(event).toBe(false);
  });

  it("calls onActiveIndexChange on mouseEnter", () => {
    const items = makeItems(3);
    const onSelect = vi.fn();
    const onActiveIndexChange = vi.fn();

    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={onSelect}
        onActiveIndexChange={onActiveIndexChange}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );

    const buttons = screen.getAllByRole("button");
    fireEvent.mouseEnter(buttons[2]);

    expect(onActiveIndexChange).toHaveBeenCalledWith(2);
  });

  it("renders avatar with fallback initial when icon is provided", () => {
    // Note: Radix AvatarImage defers rendering until image loads (never in jsdom).
    // We verify the avatar container is rendered and the fallback initial is correct.
    const items: AutocompleteItem[] = [
      { id: "agent-1", label: "@Arty", description: "claude-sonnet", icon: "/uploads/arty.png", value: "Arty" },
    ];
    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={vi.fn()}
        onActiveIndexChange={vi.fn()}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );
    // Avatar container is rendered (data-slot="avatar")
    const avatar = document.querySelector('[data-slot="avatar"]');
    expect(avatar).toBeInTheDocument();
    // Fallback initial "A" from "Arty"
    expect(screen.getByText("A")).toBeInTheDocument();
    // Description subtitle rendered
    expect(screen.getByText("claude-sonnet")).toBeInTheDocument();
  });

  it("renders avatar fallback initial when icon is null", () => {
    const items: AutocompleteItem[] = [
      { id: "agent-1", label: "@Cleo", description: "gpt-4o", icon: null, value: "Cleo" },
    ];
    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={vi.fn()}
        onActiveIndexChange={vi.fn()}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );
    expect(screen.getByText("C")).toBeInTheDocument();
  });

  it("does not render avatar for slash commands (no icon field)", () => {
    const items: AutocompleteItem[] = [
      { id: "/new", label: "/new", description: "New chat", value: "/new" },
    ];
    render(
      <AutocompletePopover
        items={items}
        activeIndex={0}
        onSelect={vi.fn()}
        onActiveIndexChange={vi.fn()}
        anchorRef={makeMockAnchorRef()}
        visible={true}
      />
    );
    // No avatar elements should be present
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });
});
