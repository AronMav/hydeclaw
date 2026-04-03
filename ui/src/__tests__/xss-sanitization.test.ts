import { describe, it, expect } from "vitest";
import { sanitizeUrl } from "@/lib/sanitize-url";

describe("sanitizeUrl", () => {
  it("allows relative URLs starting with /", () => {
    expect(sanitizeUrl("/uploads/file.png")).toBe("/uploads/file.png");
  });

  it("allows https URLs", () => {
    expect(sanitizeUrl("https://example.com/img.jpg")).toBe("https://example.com/img.jpg");
  });

  it("allows http URLs", () => {
    expect(sanitizeUrl("http://example.com/img.jpg")).toBe("http://example.com/img.jpg");
  });

  it("blocks javascript: protocol", () => {
    expect(sanitizeUrl("javascript:alert(1)")).toBe("#");
  });

  it("blocks data:text/html", () => {
    expect(sanitizeUrl("data:text/html,<script>alert(1)</script>")).toBe("#");
  });

  it("allows data:image for inline images", () => {
    expect(sanitizeUrl("data:image/png;base64,iVBOR")).toBe("data:image/png;base64,iVBOR");
  });

  it("blocks vbscript: protocol", () => {
    expect(sanitizeUrl("vbscript:MsgBox(1)")).toBe("#");
  });

  it("blocks JAVASCRIPT: (case-insensitive)", () => {
    expect(sanitizeUrl("JAVASCRIPT:alert(1)")).toBe("#");
  });

  it("returns # for empty string", () => {
    expect(sanitizeUrl("")).toBe("#");
  });

  it("trims whitespace before checking", () => {
    expect(sanitizeUrl("  javascript:alert(1)  ")).toBe("#");
  });

  it("blocks data:text/javascript", () => {
    expect(sanitizeUrl("data:text/javascript,alert(1)")).toBe("#");
  });

  it("allows data:image/jpeg", () => {
    expect(sanitizeUrl("data:image/jpeg;base64,/9j/4AAQ")).toBe("data:image/jpeg;base64,/9j/4AAQ");
  });

  it("allows data:image/svg+xml", () => {
    expect(sanitizeUrl("data:image/svg+xml;base64,PHN2Zz4=")).toBe("data:image/svg+xml;base64,PHN2Zz4=");
  });
});
