import { describe, it, expect } from "vitest";
import { convertHistory, resolveActivePath } from "@/stores/chat-history";
import type { MessageRow } from "@/types/api";

// Helper to build a MessageRow with sensible defaults; only overrides need
// to be spelled out per test case.
function makeRow(overrides: Partial<MessageRow> & { id: string; created_at?: string }): MessageRow {
  return {
    role: "assistant",
    content: "",
    tool_calls: null,
    tool_call_id: null,
    created_at: overrides.created_at ?? new Date().toISOString(),
    agent_id: null,
    status: "complete",
    feedback: 0,
    edited_at: null,
    parent_message_id: null,
    branch_from_message_id: null,
    abort_reason: null,
    thinking_blocks: null,
    ...overrides,
  };
}

describe("convertHistory — streaming placeholder does not shadow tree root (Bug 2)", () => {
  it("resolveActivePath_ignores_NULL_parent_streaming_row", () => {
    // Arrange a conversation where a streaming placeholder row with
    // parent_message_id=null appears AFTER the real user/assistant/user
    // chain. Today (pre-fix) resolveActivePath picks the streaming
    // placeholder as roots[0] and drops the real tree, so the
    // second user message "follow-up" disappears from the rendered path.
    const rows: MessageRow[] = [
      makeRow({
        id: "u1",
        role: "user",
        parent_message_id: null,
        status: "complete",
        content: "hi",
        created_at: "2026-04-20T10:00:00Z",
      }),
      makeRow({
        id: "a1",
        role: "assistant",
        parent_message_id: "u1",
        status: "complete",
        content: "hello",
        agent_id: "Arty",
        created_at: "2026-04-20T10:00:01Z",
      }),
      makeRow({
        id: "u2",
        role: "user",
        parent_message_id: "a1",
        status: "complete",
        content: "follow-up",
        created_at: "2026-04-20T10:00:02Z",
      }),
      // The villain: a streaming placeholder with NULL parent and later
      // created_at. Under the pre-fix convertHistory this row becomes
      // the second root, but roots.sort() + roots[0] picks u1 anyway —
      // UNLESS the placeholder is ordered earlier. To make the shadow
      // reproducible we set its created_at BEFORE u1 so resolveActivePath
      // picks it and walks no children.
      makeRow({
        id: "s1",
        role: "assistant",
        parent_message_id: null,
        status: "streaming",
        content: "",
        agent_id: "Arty",
        created_at: "2026-04-20T09:59:59Z",
      }),
    ];

    // selectedBranches is empty — the rendered path is just "walk newest child".
    const messages = convertHistory(rows, true, {});

    // Expected: streaming placeholder is filtered out BEFORE resolveActivePath,
    // so u1 is the sole root, and the walk reaches a1 → u2.
    const roles = messages.map(m => m.role);
    expect(roles).toContain("user");
    // The regression gate: the "follow-up" user message MUST be present.
    const userContents = messages
      .filter(m => m.role === "user")
      .flatMap(m => m.parts)
      .filter((p): p is { type: "text"; text: string } => p.type === "text")
      .map(p => p.text);
    expect(userContents).toContain("follow-up");

    // And: no streaming placeholder leaks into the output.
    const ids = messages.map(m => m.id);
    expect(ids).not.toContain("s1");

    // Sanity: the real chain is in render order.
    expect(ids).toEqual(["u1", "a1", "u2"]);
  });

  it("sensitivity probe: resolveActivePath picks the NULL-parent streaming row as root if filter runs AFTER", () => {
    // Informational probe — proves the bug is real. If we call
    // resolveActivePath DIRECTLY on rows (no pre-filter), the streaming
    // placeholder shadows u1 because its created_at is earlier.
    const rows: MessageRow[] = [
      makeRow({
        id: "u1",
        role: "user",
        parent_message_id: null,
        status: "complete",
        content: "hi",
        created_at: "2026-04-20T10:00:00Z",
      }),
      makeRow({
        id: "a1",
        role: "assistant",
        parent_message_id: "u1",
        status: "complete",
        content: "hello",
        created_at: "2026-04-20T10:00:01Z",
      }),
      makeRow({
        id: "u2",
        role: "user",
        parent_message_id: "a1",
        status: "complete",
        content: "follow-up",
        created_at: "2026-04-20T10:00:02Z",
      }),
      makeRow({
        id: "s1",
        role: "assistant",
        parent_message_id: null,
        status: "streaming",
        content: "",
        created_at: "2026-04-20T09:59:59Z",
      }),
    ];

    const path = resolveActivePath(rows, {});
    const ids = path.map(r => r.id);
    // Bug shape: s1 is the earliest root → path is just [s1] with no children.
    // If convertHistory ran resolveActivePath FIRST then filtered streaming
    // rows AFTER, u1/a1/u2 would be lost. This probe guarantees the pre-fix
    // order is demonstrably broken.
    expect(ids[0]).toBe("s1");
    expect(ids).not.toContain("u2");
  });
});
