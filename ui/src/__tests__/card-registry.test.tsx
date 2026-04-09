import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import React from "react";

// ── Mocks ──────────────────────────────────────────────────────────────────

// Mock lucide-react (used by MetricCard)
vi.mock("lucide-react", () => ({
  TrendingUp: (props: Record<string, unknown>) => <span data-testid="trending-up" {...props} />,
  TrendingDown: (props: Record<string, unknown>) => <span data-testid="trending-down" {...props} />,
  Minus: (props: Record<string, unknown>) => <span data-testid="minus" {...props} />,
}));

import { CARD_REGISTRY, GenerativeUISlot, CardErrorBoundary } from "@/components/ui/card-registry";
import { TableCard, MetricCard } from "@/components/ui/rich-card";

// ── Registry lookup tests ──────────────────────────────────────────────────

describe("CARD_REGISTRY", () => {
  it("maps 'table' to TableCard component", () => {
    expect(CARD_REGISTRY.get("table")).toBe(TableCard);
  });

  it("maps 'metric' to MetricCard component", () => {
    expect(CARD_REGISTRY.get("metric")).toBe(MetricCard);
  });

  it("returns undefined for unknown card types", () => {
    expect(CARD_REGISTRY.get("unknown-type")).toBeUndefined();
  });
});

// ── GenerativeUISlot rendering tests ───────────────────────────────────────

describe("GenerativeUISlot", () => {
  it("renders TableCard for cardType='table'", () => {
    const data = { columns: ["Name", "Age"], rows: [["Alice", 30]], title: "Test Table" };
    render(<GenerativeUISlot cardType="table" data={data} />);
    expect(screen.getByText("Test Table")).toBeDefined();
    expect(screen.getByText("Name")).toBeDefined();
    expect(screen.getByText("Alice")).toBeDefined();
  });

  it("renders JSON fallback for unknown cardType", () => {
    const data = { foo: "bar", count: 42 };
    render(<GenerativeUISlot cardType="custom-widget" data={data} />);
    const pre = screen.getByText(/foo/);
    expect(pre.textContent).toContain('"foo"');
    expect(pre.textContent).toContain('"bar"');
    expect(pre.closest("pre")).toBeDefined();
  });

  it("renders CardErrorBoundary fallback when child component throws", () => {
    // Suppress React error boundary console.error noise
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    // Register a broken card temporarily
    const BrokenCard = () => {
      throw new Error("Boom!");
    };
    CARD_REGISTRY.set("broken-card", BrokenCard);

    render(<GenerativeUISlot cardType="broken-card" data={{}} />);
    expect(screen.getByText("Card rendering error")).toBeDefined();

    // Cleanup
    CARD_REGISTRY.delete("broken-card");
    spy.mockRestore();
  });
});

// ── CardErrorBoundary tests ────────────────────────────────────────────────

describe("CardErrorBoundary", () => {
  it("logs error and shows fallback text when child throws", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});

    const Thrower = () => {
      throw new Error("Test crash");
    };

    render(
      <CardErrorBoundary resetKey="test">
        <Thrower />
      </CardErrorBoundary>
    );

    expect(screen.getByText("Card rendering error")).toBeDefined();
    // Verify console.error was called with our boundary message
    expect(spy).toHaveBeenCalledWith("[CardErrorBoundary]", "Test crash");

    spy.mockRestore();
  });
});
