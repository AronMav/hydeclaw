import { describe, test, expect } from "bun:test";
import { buildHealthResponse } from "../health";

describe("buildHealthResponse", () => {
  test("returns ok with agent list", () => {
    const resp = buildHealthResponse(["main"], { main: "telegram" }, 123);
    expect(resp.ok).toBe(true);
    expect(resp.agents).toEqual(["main"]);
    expect(resp.channels.main).toBe("telegram");
    expect(resp.uptime_seconds).toBe(123);
    expect(resp.version).toBe("1.0.0");
  });

  test("returns empty when no channels", () => {
    const resp = buildHealthResponse([], {}, 0);
    expect(resp.ok).toBe(true);
    expect(resp.agents).toEqual([]);
    expect(resp.uptime_seconds).toBe(0);
  });

  test("multiple agents", () => {
    const resp = buildHealthResponse(
      ["main", "analyst"],
      { main: "telegram", analyst: "discord" },
      456,
    );
    expect(resp.agents).toHaveLength(2);
    expect(resp.channels.analyst).toBe("discord");
  });
});
