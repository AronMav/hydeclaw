import { describe, it, expect } from "vitest";
import { isValidCron, describeCron, CRON_PRESETS, TIMEZONES } from "@/lib/cron";

describe("isValidCron", () => {
  it("accepts valid 5-field expressions", () => {
    expect(isValidCron("* * * * *")).toBe(true);
    expect(isValidCron("0 9 * * 1-5")).toBe(true);
    expect(isValidCron("*/30 10-19 * * *")).toBe(true);
    expect(isValidCron("0 8-22/2 * * *")).toBe(true);
  });

  it("rejects invalid expressions", () => {
    expect(isValidCron("")).toBe(false);
    expect(isValidCron("* * *")).toBe(false);
    expect(isValidCron("0 9 * * * *")).toBe(false);
    expect(isValidCron("hello")).toBe(false);
  });

  it("handles whitespace", () => {
    expect(isValidCron("  0  9  *  *  *  ")).toBe(true);
  });
});

describe("describeCron", () => {
  // Mock t() that returns interpolated strings like the real one
  const t = (key: string, values?: Record<string, string | number>) => {
    const templates: Record<string, string> = {
      "agents.cron_every_n_min": "Every {{interval}} min{{hourRange}}{{dayStr}}",
      "agents.cron_every_n_hours": "Every {{interval}} h at :{{min}}{{dayStr}}",
      "agents.cron_at_min_hours": "At :{{min}} — hours {{hour}}{{dayStr}}",
      "agents.cron_at_time": "At {{hour}}:{{min}}{{dayStr}}",
      "agents.cron_weekdays": "Mon–Fri",
      "agents.cron_days": "days: {{dow}}",
    };
    let result = templates[key] ?? key;
    if (values) {
      for (const [k, v] of Object.entries(values)) {
        result = result.replace(`{{${k}}}`, String(v));
      }
    }
    return result;
  };

  it("returns raw expression for invalid cron", () => {
    expect(describeCron("invalid", t as any)).toBe("invalid");
  });

  it("handles minute intervals", () => {
    expect(describeCron("*/30 * * * *", t as any)).toBe("Every 30 min");
  });

  it("handles minute intervals with hour range", () => {
    expect(describeCron("*/30 10-19 * * *", t as any)).toBe("Every 30 min 10-19");
  });

  it("handles hour intervals", () => {
    expect(describeCron("0 */2 * * *", t as any)).toBe("Every 2 h at :00");
  });

  it("handles specific hours", () => {
    expect(describeCron("0 9,13,18 * * *", t as any)).toBe("At :00 — hours 9,13,18");
  });

  it("handles fixed time", () => {
    expect(describeCron("0 9 * * *", t as any)).toBe("At 9:00");
  });

  it("handles weekday filter", () => {
    expect(describeCron("0 9 * * 1-5", t as any)).toBe("At 9:00 (Mon–Fri)");
  });
});

describe("CRON_PRESETS", () => {
  it("has entries with valid cron values", () => {
    expect(CRON_PRESETS.length).toBeGreaterThan(5);
    for (const preset of CRON_PRESETS) {
      expect(preset).toHaveProperty("labelKey");
      expect(preset).toHaveProperty("value");
      expect(isValidCron(preset.value)).toBe(true);
    }
  });
});

describe("TIMEZONES", () => {
  it("has entries with non-empty values", () => {
    expect(TIMEZONES.length).toBeGreaterThan(5);
    for (const tz of TIMEZONES) {
      expect(tz).toHaveProperty("value");
      expect(tz.value.length).toBeGreaterThan(0);
    }
  });

  it("includes Europe/Moscow", () => {
    expect(TIMEZONES.some((tz) => tz.value === "Europe/Moscow")).toBe(true);
  });
});
