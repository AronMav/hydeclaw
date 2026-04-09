import { vi, describe, it, expect, beforeEach } from "vitest";

// ── Draft persistence helpers tests ──────────────────────────────────────────
// These tests import the draft helper functions that will be exported from
// ChatThread.tsx. They test localStorage save/restore/clear behavior.

// Import the helpers — these will be added to ChatThread.tsx
import { saveDraft, loadDraft, clearDraft } from "@/app/(authenticated)/chat/ChatThread";

describe("Draft persistence helpers", () => {
  beforeEach(() => {
    // Clear localStorage before each test
    localStorage.clear();
  });

  it("Test 1: saveDraft writes to localStorage key hydeclaw.draft.{agent}", () => {
    saveDraft("Aria", "Hello world");
    expect(localStorage.getItem("hydeclaw.draft.Aria")).toBe("Hello world");
  });

  it("Test 2: loadDraft returns stored text", () => {
    localStorage.setItem("hydeclaw.draft.Aria", "Stored text");
    expect(loadDraft("Aria")).toBe("Stored text");
  });

  it("Test 2b: loadDraft returns empty string when no draft stored", () => {
    expect(loadDraft("NonExistentAgent")).toBe("");
  });

  it("Test 3: saveDraft with empty string removes the key", () => {
    localStorage.setItem("hydeclaw.draft.Aria", "Some text");
    saveDraft("Aria", "");
    expect(localStorage.getItem("hydeclaw.draft.Aria")).toBeNull();
  });

  it("Test 4: clearDraft removes the key", () => {
    localStorage.setItem("hydeclaw.draft.Aria", "Some text");
    clearDraft("Aria");
    expect(localStorage.getItem("hydeclaw.draft.Aria")).toBeNull();
  });

  it("Draft keys are per-agent (no cross-agent contamination)", () => {
    saveDraft("Aria", "Aria's draft");
    saveDraft("Bob", "Bob's draft");
    expect(loadDraft("Aria")).toBe("Aria's draft");
    expect(loadDraft("Bob")).toBe("Bob's draft");
    clearDraft("Aria");
    expect(loadDraft("Aria")).toBe("");
    expect(loadDraft("Bob")).toBe("Bob's draft");
  });
});
