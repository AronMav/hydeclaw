import { describe, test, expect } from "bun:test";
import { getFormattingPrompt } from "../formatting";

describe("getFormattingPrompt", () => {
  test("returns prompt for all supported channels", () => {
    for (const ch of ["telegram", "discord", "slack", "matrix", "irc", "whatsapp"]) {
      const prompt = getFormattingPrompt(ch);
      expect(prompt).toBeDefined();
      expect(prompt!.length).toBeGreaterThan(100);
    }
  });

  test("returns undefined for unknown channel", () => {
    expect(getFormattingPrompt("smoke-signal")).toBeUndefined();
  });

  test("telegram prompt mentions MarkdownV2", () => {
    expect(getFormattingPrompt("telegram")).toContain("MarkdownV2");
  });

  test("irc prompt says plain text", () => {
    expect(getFormattingPrompt("irc")).toContain("plain text");
  });

  test("slack prompt mentions mrkdwn", () => {
    expect(getFormattingPrompt("slack")).toContain("mrkdwn");
  });
});
