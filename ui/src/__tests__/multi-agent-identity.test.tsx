"use client";

import { vi, describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
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

// ── Mock: translation hook ─────────────────────────────────────────────────

vi.mock("@/hooks/use-translation", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    locale: "en",
  }),
}));

// ── Mock: use-tool-progress ────────────────────────────────────────────────

vi.mock("@/hooks/use-tool-progress", () => ({
  useToolProgress: () => 0,
}));

// ── Mock: stores ───────────────────────────────────────────────────────────

const mockChatStoreState: Record<string, unknown> = {
  currentAgent: "TestAgent",
  agents: {
    TestAgent: {
      activeSessionId: null,
      activeSessionIds: [],
      viewMode: "live",
      streamStatus: "idle",
      streamError: null,
      liveMessages: [],
      inputText: "",
      pendingTargetAgent: null,
    },
  },
};

vi.mock("@/stores/auth-store", () => ({
  useAuthStore: Object.assign(
    (selector?: (s: Record<string, unknown>) => unknown) => {
      const state = {
        token: "test-token",
        isAuthenticated: true,
        version: "1.0.0",
        agents: ["TestAgent"],
        agentIcons: { Arty: "arty-icon.png", Helper: "helper-icon.png" },
        lastFetched: Date.now(),
        login: vi.fn(),
        logout: vi.fn(),
        restore: vi.fn(),
        refreshIfStale: vi.fn(),
      };
      return selector ? selector(state) : state;
    },
    { getState: () => ({ token: "test-token", logout: vi.fn() }) },
  ),
}));

vi.mock("@/stores/chat-store", () => ({
  useChatStore: Object.assign(
    (selector?: (s: Record<string, unknown>) => unknown) => {
      return selector ? selector(mockChatStoreState) : mockChatStoreState;
    },
    {
      getState: () => ({
        currentAgent: "TestAgent",
        agents: { TestAgent: { activeSessionId: null, activeSessionIds: [], viewMode: "live", streamStatus: "idle" } },
        regenerate: vi.fn(),
        clearError: vi.fn(),
        sendMessage: vi.fn(),
        deleteMessage: vi.fn().mockResolvedValue(undefined),
        editMessage: vi.fn(),
        exportSession: vi.fn(),
      }),
    },
  ),
  isActiveStream: () => false,
  convertHistory: () => [],
  MAX_INPUT_LENGTH: 32000,
}));

// ── Mock: @/lib/queries ────────────────────────────────────────────────────

vi.mock("@/lib/queries", () => ({
  useSessions: () => ({ data: { sessions: [] }, isLoading: false, error: null, refetch: vi.fn() }),
  useSessionMessages: () => ({ data: { messages: [] }, isLoading: false, error: null, refetch: vi.fn() }),
  useAgents: () => ({ data: [], isLoading: false, error: null, refetch: vi.fn() }),
  useProviders: () => ({ data: [], isLoading: false, error: null, refetch: vi.fn() }),
  useProviderModels: () => ({ data: [], isLoading: false, error: null, refetch: vi.fn() }),
}));

// ── Mock: @/lib/sanitize-url ───────────────────────────────────────────────

vi.mock("@/lib/sanitize-url", () => ({
  sanitizeUrl: (url: string) => url,
}));

// ── Mock: @/lib/api ────────────────────────────────────────────────────────

vi.mock("@/lib/api", () => ({
  apiGet: vi.fn().mockResolvedValue({}),
  apiPost: vi.fn().mockResolvedValue({}),
  apiPut: vi.fn().mockResolvedValue({}),
  apiDelete: vi.fn().mockResolvedValue(undefined),
  getToken: () => "test-token",
}));

// ── Mock: @/lib/query-client ───────────────────────────────────────────────

vi.mock("@/lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn(), setQueryData: vi.fn() },
}));

// ── Mock: @tanstack/react-query ────────────────────────────────────────────

vi.mock("@tanstack/react-query", async () => {
  const actual = await vi.importActual("@tanstack/react-query");
  return {
    ...actual,
    useQueryClient: () => ({ invalidateQueries: vi.fn(), setQueryData: vi.fn() }),
    useQuery: () => ({ data: undefined, isLoading: false, error: null, refetch: vi.fn() }),
  };
});

// ── Mock: zustand/react/shallow ────────────────────────────────────────────

vi.mock("zustand/react/shallow", () => ({
  useShallow: (fn: unknown) => fn,
}));

// ── Mock: react-virtuoso (no real layout in jsdom) ────────────────────────

vi.mock("react-virtuoso", () => {
  const React = require("react");
  return {
    Virtuoso: React.forwardRef(function MockVirtuoso(props: Record<string, unknown>, ref: unknown) {
      const data = (props.data ?? []) as unknown[];
      const itemContent = props.itemContent as ((index: number, item: unknown) => React.ReactNode) | undefined;
      const components = props.components as { Header?: () => React.ReactNode; Footer?: () => React.ReactNode } | undefined;
      return React.createElement("div", { "data-testid": "virtuoso-mock", ref },
        components?.Header ? React.createElement(components.Header) : null,
        ...(data.map((item: unknown, i: number) =>
          React.createElement("div", { key: i }, itemContent ? itemContent(i, item) : null)
        )),
        components?.Footer ? React.createElement(components.Footer) : null,
      );
    }),
  };
});

// ── Mock: markdown rendering ───────────────────────────────────────────────

vi.mock("@/components/ui/markdown", () => ({
  Markdown: ({ children }: { children: string }) => <div data-testid="markdown">{children}</div>,
}));

// ── Mock: rich-card ────────────────────────────────────────────────────────

vi.mock("@/components/ui/rich-card", () => ({
  RichCard: ({ part }: { part: unknown }) => <div data-testid="rich-card">{JSON.stringify(part)}</div>,
}));

// ── Import components under test ───────────────────────────────────────────

import { MessageList } from "@/app/(authenticated)/chat/MessageList";
import { MessageItem } from "@/app/(authenticated)/chat/MessageItem";
import type { ChatMessage } from "@/stores/chat-store";

// ── Helper: build messages ─────────────────────────────────────────────────

function makeMsg(overrides: Partial<ChatMessage> & { id: string; role: ChatMessage["role"] }): ChatMessage {
  return {
    parts: [{ type: "text", text: "Hello" }],
    ...overrides,
  };
}

// ── Tests ──────────────────────────────────────────────────────────────────

describe("Multi-Agent Identity (MAID)", () => {
  // MAID-01: Agent turn separator between consecutive different-agent assistant messages
  describe("MAID-01: Agent turn separators", () => {
    it("renders separator between consecutive assistant messages from different agents — Phase 15 VSEP-01", () => {
      const messages: ChatMessage[] = [
        makeMsg({ id: "1", role: "assistant", agentId: "Arty", parts: [{ type: "text", text: "I am Arty" }] }),
        makeMsg({ id: "2", role: "assistant", agentId: "Helper", parts: [{ type: "text", text: "I am Helper" }] }),
      ];

      render(
        <MessageList
          messages={messages}
          isStreaming={false}
          showThinking={false}
          isLoadingHistory={false}
          emptyState={<div />}
          hiddenCount={0}
          onLoadEarlier={() => {}}
        />,
      );

      const separator = screen.getByTestId("agent-turn-separator");
      expect(separator).toBeInTheDocument();
      expect(separator).toHaveTextContent("chat.agent_responding");
    });

    it("does NOT render separator between consecutive assistant messages from the SAME agent", () => {
      const messages: ChatMessage[] = [
        makeMsg({ id: "1", role: "assistant", agentId: "Arty", parts: [{ type: "text", text: "First" }] }),
        makeMsg({ id: "2", role: "assistant", agentId: "Arty", parts: [{ type: "text", text: "Second" }] }),
      ];

      render(
        <MessageList
          messages={messages}
          isStreaming={false}
          showThinking={false}
          isLoadingHistory={false}
          emptyState={<div />}
          hiddenCount={0}
          onLoadEarlier={() => {}}
        />,
      );

      expect(screen.queryByTestId("agent-turn-separator")).not.toBeInTheDocument();
    });

    it("does NOT render separator when user message sits between different-agent assistants", () => {
      const messages: ChatMessage[] = [
        makeMsg({ id: "1", role: "assistant", agentId: "Arty", parts: [{ type: "text", text: "Arty says" }] }),
        makeMsg({ id: "2", role: "user", parts: [{ type: "text", text: "User question" }] }),
        makeMsg({ id: "3", role: "assistant", agentId: "Helper", parts: [{ type: "text", text: "Helper says" }] }),
      ];

      render(
        <MessageList
          messages={messages}
          isStreaming={false}
          showThinking={false}
          isLoadingHistory={false}
          emptyState={<div />}
          hiddenCount={0}
          onLoadEarlier={() => {}}
        />,
      );

      expect(screen.queryByTestId("agent-turn-separator")).not.toBeInTheDocument();
    });
  });

  // MAID-02: ThinkingMessage shows pendingTargetAgent name
  describe("MAID-02: ThinkingMessage agent display", () => {
    it("renders ThinkingMessage with currentAgent name when no pendingTargetAgent", () => {
      render(
        <MessageList
          messages={[]}
          isStreaming={true}
          showThinking={true}
          isLoadingHistory={false}
          emptyState={<div />}
          hiddenCount={0}
          onLoadEarlier={() => {}}
        />,
      );

      expect(screen.getByText("TestAgent")).toBeInTheDocument();
    });
  });

  // MAID-03: History messages show correct agent avatars from agentId in DB
  describe("MAID-03: History message agent identity", () => {
    it("renders assistant message with agentId from message prop (not from store)", () => {
      const msg = makeMsg({
        id: "hist-1",
        role: "assistant",
        agentId: "HistoryAgent",
        parts: [{ type: "text", text: "Historical reply" }],
      });

      render(<MessageItem message={msg} />);
      expect(screen.getByText("HistoryAgent")).toBeInTheDocument();
    });
  });

  // MAID-04: No assistant-ui hooks for agent identity
  describe("MAID-04: No assistant-ui identity dependency", () => {
    it("MessageItem uses agentId from message prop directly without assistant-ui hooks", () => {
      // This test verifies the implementation approach: agentId comes from ChatMessage.agentId
      // not from useMessage() or useMessageRuntime() or any assistant-ui context
      const msg = makeMsg({
        id: "direct-1",
        role: "assistant",
        agentId: "DirectAgent",
        parts: [{ type: "text", text: "Direct agent reply" }],
      });

      render(<MessageItem message={msg} />);
      // Agent name appears -- proving it comes from prop, not assistant-ui context
      expect(screen.getByText("DirectAgent")).toBeInTheDocument();
    });
  });
});
