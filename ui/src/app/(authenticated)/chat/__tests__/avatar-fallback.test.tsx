"use client";

import { vi, describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

// ── Polyfill: ResizeObserver (not available in jsdom) ──────────────────────

globalThis.ResizeObserver = class ResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof globalThis.ResizeObserver;

// ── Polyfill: IntersectionObserver (not available in jsdom) ─────────────────

globalThis.IntersectionObserver = class IntersectionObserver {
  constructor() {}
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof globalThis.IntersectionObserver;

// ── Polyfill: Element.scrollIntoView (not available in jsdom) ───────────────

Element.prototype.scrollIntoView = vi.fn();

// ── Mock: next/navigation ──────────────────────────────────────────────────

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), back: vi.fn(), refresh: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/",
}));

// ── Mock: sonner toast ─────────────────────────────────────────────────────

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

// ── Import tested exports ──────────────────────────────────────────────────

import { RoleAvatar, hashAgentName, AGENT_COLORS } from "@/app/(authenticated)/chat/avatar/RoleAvatar";

// ── Tests ──────────────────────────────────────────────────────────────────

describe("hashAgentName", () => {
  it("returns same value for same input (deterministic)", () => {
    const a = hashAgentName("Agent1");
    const b = hashAgentName("Agent1");
    expect(a).toBe(b);
  });

  it("returns different values for different inputs", () => {
    const a = hashAgentName("Agent1");
    const b = hashAgentName("Helper");
    expect(a).not.toBe(b);
  });

  it("produces a valid AGENT_COLORS index via modulo", () => {
    const idx = hashAgentName("Agent1") % 8;
    expect(idx).toBeGreaterThanOrEqual(0);
    expect(idx).toBeLessThan(8);
  });
});

describe("AGENT_COLORS", () => {
  it("has 8 entries", () => {
    expect(AGENT_COLORS).toHaveLength(8);
  });
});

describe("RoleAvatar", () => {
  it("shows initial letter when no iconUrl is provided", () => {
    render(<RoleAvatar role="assistant" agentName="Agent1" />);
    expect(screen.getByText("A")).toBeInTheDocument();
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("renders Avatar container with fallback when iconUrl is provided", () => {
    const { container } = render(<RoleAvatar role="assistant" iconUrl="/uploads/test.png" agentName="Agent1" />);
    // Radix Avatar doesn't render <img> in jsdom (no image load events), so fallback is shown
    expect(container.querySelector('[data-slot="avatar"]')).toBeInTheDocument();
    expect(screen.getByText("A")).toBeInTheDocument();
  });

  it("renders fallback letter when no iconUrl is provided", () => {
    render(<RoleAvatar role="assistant" agentName="Helper" />);
    expect(screen.getByText("H")).toBeInTheDocument();
  });
});
