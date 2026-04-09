import { vi, describe, it, expect, beforeAll } from "vitest";
import { render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";

// ── Mock: next/navigation ──────────────────────────────────────────────────

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), back: vi.fn(), refresh: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
  usePathname: () => "/",
}));

// ── Mock: next/dynamic ─────────────────────────────────────────────────────

vi.mock("next/dynamic", () => ({
  default: () => {
    const Stub = () => null;
    Stub.displayName = "DynamicStub";
    return Stub;
  },
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

// ── Mock: use-ws-subscription ──────────────────────────────────────────────

vi.mock("@/hooks/use-ws-subscription", () => ({
  useWsSubscription: vi.fn(),
}));

// ── Mock: use-auto-refresh ─────────────────────────────────────────────────

vi.mock("@/hooks/use-auto-refresh", () => ({
  useAutoRefresh: vi.fn(),
}));

// ── Mock: use-tool-progress ────────────────────────────────────────────────

vi.mock("@/hooks/use-tool-progress", () => ({
  useToolProgress: () => 0,
}));

// ── Mock: stores ───────────────────────────────────────────────────────────

vi.mock("@/stores/auth-store", () => ({
  useAuthStore: Object.assign(
    (selector?: (s: Record<string, unknown>) => unknown) => {
      const state = {
        token: "test-token",
        isAuthenticated: true,
        version: "1.0.0",
        agents: ["main"],
        agentIcons: {},
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

vi.mock("@/stores/ws-store", () => ({
  useWsStore: (selector?: (s: Record<string, unknown>) => unknown) => {
    const state = {
      ws: null,
      connected: false,
      wsStatus: "disconnected",
      connect: vi.fn(),
      disconnect: vi.fn(),
    };
    return selector ? selector(state) : state;
  },
}));

vi.mock("@/stores/chat-store", () => ({
  useChatStore: Object.assign(
    (selector?: (s: Record<string, unknown>) => unknown) => {
      const agentState = {
        activeSessionId: null,
        activeSessionIds: [],
        messageSource: { mode: "new-chat" },
        streamError: null,
        messages: [],
        inputText: "",
      };
      const state: Record<string, unknown> = {
        currentAgent: "main",
        agents: { main: agentState },
        currentSessionId: null,
        messages: [],
        sessions: [],
        inputText: "",
        setCurrentAgent: vi.fn(),
        setCurrentSession: vi.fn(),
        sendMessage: vi.fn(),
        setInputText: vi.fn(),
        loadSessions: vi.fn(),
        createSession: vi.fn(),
        deleteSession: vi.fn(),
        renameSession: vi.fn(),
        cancelStream: vi.fn(),
      };
      return selector ? selector(state) : state;
    },
    {
      getState: () => ({
        currentAgent: "main",
        agents: { main: { activeSessionId: null, activeSessionIds: [], messageSource: { mode: "new-chat" }, connectionPhase: "idle" } },
        setCurrentAgent: vi.fn(),
        selectSession: vi.fn(),
        newChat: vi.fn(),
        deleteSession: vi.fn().mockResolvedValue(undefined),
        deleteAllSessions: vi.fn().mockResolvedValue(undefined),
        renameSession: vi.fn(),
        regenerate: vi.fn(),
        clearError: vi.fn(),
      }),
    },
  ),
  isActivePhase: () => false,
  getInitialAgent: (agents: string[]) => agents[0] || "main",
  getLastSessionId: () => undefined,
  MAX_INPUT_LENGTH: 32000,
  convertHistory: () => [],
}));

vi.mock("@/stores/canvas-store", () => ({
  useCanvasStore: (selector?: (s: Record<string, unknown>) => unknown) => {
    const state = {
      canvases: {},
      panelOpen: false,
      handleEvent: vi.fn(),
      clearAgent: vi.fn(),
      togglePanel: vi.fn(),
    };
    return selector ? selector(state) : state;
  },
}));

// ── Mock: zustand/react/shallow ────────────────────────────────────────────

vi.mock("zustand/react/shallow", () => ({
  useShallow: (fn: unknown) => fn,
}));

// ── Mock: @/lib/api ────────────────────────────────────────────────────────

vi.mock("@/lib/api", () => ({
  apiGet: vi.fn().mockResolvedValue({}),
  apiPost: vi.fn().mockResolvedValue({}),
  apiPut: vi.fn().mockResolvedValue({}),
  apiPatch: vi.fn().mockResolvedValue({}),
  apiDelete: vi.fn().mockResolvedValue(undefined),
  getToken: () => "test-token",
}));

// ── Mock: @/lib/ws ─────────────────────────────────────────────────────────

vi.mock("@/lib/ws", () => ({
  WsManager: vi.fn(),
}));

// ── Mock: @/lib/queries ────────────────────────────────────────────────────

const emptyQuery = { data: [], isLoading: false, error: null, refetch: vi.fn() };
const emptyObjQuery = { data: {}, isLoading: false, error: null, refetch: vi.fn() };
const emptyMutation = { mutateAsync: vi.fn(), mutate: vi.fn(), isPending: false, error: null };

vi.mock("@/lib/queries", () => ({
  qk: {
    agents: ["agents"],
    agent: (name: string) => ["agents", name],
    agentChannels: (name: string) => ["agents", name, "channels"],
    tools: ["tools"],
    yamlTools: ["yaml-tools"],
    mcpServers: ["mcp"],
    secrets: ["secrets"],
    skills: ["skills"],
    channels: ["channels"],
    activeChannels: ["channels", "active"],
    cron: ["cron"],
    cronRuns: (id: string) => ["cron", id, "runs"],
    cronRunsAll: ["cron", "runs"],
    memoryStats: ["memory", "stats"],
    audit: (p: Record<string, string>) => ["audit", p],
    config: ["config"],
    access: ["access"],
    usage: (d: number) => ["usage", d],
    dailyUsage: (d: number) => ["usage", "daily", d],
    providerModels: (p: string) => ["providers", p, "models"],
    webhooks: ["webhooks"],
    approvals: ["approvals"],
    backups: ["backups"],
    sessions: (a: string) => ["sessions", "list", a],
    sessionMessages: (id: string) => ["sessions", id, "messages"],
    providers: ["providers"],
    providerTypes: ["provider-types"],
    providerActive: ["provider-active"],
    mediaDrivers: ["media-drivers"],
    oauthAccounts: ["oauth", "accounts"],
    oauthBindings: (a: string) => ["oauth", "bindings", a],
  },
  // Query hooks
  useAgents: () => ({ ...emptyQuery, data: [] }),
  useSecrets: () => ({ ...emptyQuery, data: [] }),
  useTools: () => ({ ...emptyQuery, data: [] }),
  useYamlTools: () => ({ ...emptyQuery, data: [] }),
  useMcpServers: () => ({ ...emptyQuery, data: [] }),
  useSkills: () => ({ ...emptyQuery, data: [] }),
  useCronJobs: () => ({ ...emptyQuery, data: [] }),
  useCronRuns: () => ({ ...emptyQuery, data: [] }),
  useChannels: () => ({ ...emptyQuery, data: [] }),
  useActiveChannels: () => ({ ...emptyQuery, data: [] }),
  useMemoryStats: () => ({ ...emptyObjQuery, data: { total: 0, total_chunks: 0, pinned: 0 } }),
  useAudit: () => ({ ...emptyQuery, data: [] }),
  useUsage: () => ({ ...emptyObjQuery, data: { usage: [] } }),
  useDailyUsage: () => ({ ...emptyObjQuery, data: { daily: [] } }),
  useProviderModels: () => ({ ...emptyQuery, data: [] }),
  useApprovals: () => ({ ...emptyQuery, data: [] }),
  useWebhooks: () => ({ ...emptyQuery, data: [] }),
  useBackups: () => ({ ...emptyQuery, data: [] }),
  useSessions: () => ({ ...emptyQuery, data: { sessions: [] } }),
  useSessionMessages: () => ({ ...emptyObjQuery, data: { messages: [] } }),
  useProviders: () => ({ ...emptyQuery, data: [] }),
  useProviderTypes: () => ({ ...emptyQuery, data: [] }),
  useProviderActive: () => ({ ...emptyQuery, data: [] }),
  useMediaDrivers: () => ({ ...emptyObjQuery, data: {} }),
  useOAuthAccounts: () => ({ ...emptyQuery, data: [] }),
  useOAuthBindings: () => ({ ...emptyQuery, data: [] }),
  useAgentTasks: () => ({ ...emptyQuery, data: [] }),
  // Mutation hooks
  useUpsertSecret: () => ({ ...emptyMutation }),
  useDeleteSecret: () => ({ ...emptyMutation }),
  useUpdateAgent: () => ({ ...emptyMutation }),
  useCreateCronJob: () => ({ ...emptyMutation }),
  useUpdateCronJob: () => ({ ...emptyMutation }),
  useDeleteCronJob: () => ({ ...emptyMutation }),
  useRunCronJob: () => ({ ...emptyMutation }),
  useRestartService: () => ({ ...emptyMutation }),
  useRebuildService: () => ({ ...emptyMutation }),
  useResolveApproval: () => ({ ...emptyMutation }),
  useCreateWebhook: () => ({ ...emptyMutation }),
  useUpdateWebhook: () => ({ ...emptyMutation }),
  useDeleteWebhook: () => ({ ...emptyMutation }),
  useCreateBackup: () => ({ ...emptyMutation }),
  useCreateProvider: () => ({ ...emptyMutation }),
  useUpdateProvider: () => ({ ...emptyMutation }),
  useDeleteProvider: () => ({ ...emptyMutation }),
  useSetProviderActive: () => ({ ...emptyMutation }),
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

// ── Mock: @/lib/query-client ───────────────────────────────────────────────

vi.mock("@/lib/query-client", () => ({
  queryClient: { invalidateQueries: vi.fn(), setQueryData: vi.fn() },
}));

// ── Mock: @/providers/assistant-runtime ─────────────────────────────────────

vi.mock("@/providers/assistant-runtime", () => ({
  ChatRuntimeProvider: ({ children }: { children: React.ReactNode }) => children,
}));

// ── Mock: react-virtuoso ───────────────────────────────────────────────────

vi.mock("react-virtuoso", () => ({
  Virtuoso: () => null,
}));

// ── Mock: @/lib/cron ───────────────────────────────────────────────────────

vi.mock("@/lib/cron", () => ({
  describeCron: () => "every day",
  isValidCron: () => true,
}));

// ── Mock: @xyflow/react ────────────────────────────────────────────────────

vi.mock("@xyflow/react", () => ({
  ReactFlow: () => null,
  useNodesState: () => [[], vi.fn(), vi.fn()],
  useEdgesState: () => [[], vi.fn(), vi.fn()],
  Background: () => null,
  Controls: () => null,
  MiniMap: () => null,
  Panel: () => null,
  Handle: () => null,
  Position: { Top: "top", Bottom: "bottom", Left: "left", Right: "right" },
}));

// ── Mock: @/components/workspace/* (dynamic imports) ───────────────────────

vi.mock("@/components/workspace/code-editor", () => ({
  CodeEditor: () => null,
  getLangFromFilename: () => "text",
}));

vi.mock("@/components/workspace/markdown-editor", () => ({
  MarkdownEditor: () => null,
}));

// ── Smoke Tests ────────────────────────────────────────────────────────────

describe("Page smoke tests", () => {

  it("renders access page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/access/page");
    render(<Page />);
    expect(screen.getByText("access.title")).toBeInTheDocument();
  });

  it("renders agents page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/agents/page");
    render(<Page />);
    expect(screen.getByText("agents.title")).toBeInTheDocument();
  });

  it("renders approvals page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/approvals/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders audit page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/audit/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders backups page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/backups/page");
    render(<Page />);
    expect(screen.getByText("backups.title")).toBeInTheDocument();
  });

  it("renders canvas redirect page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/canvas/page");
    const { container } = render(<Page />);
    // Canvas page is a redirect — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders channels page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/channels/page");
    render(<Page />);
    expect(screen.getByText("channels.title")).toBeInTheDocument();
  });

  it("renders chat page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/chat/page");
    const { container } = render(<Page />);
    expect(container).toBeTruthy();
  });

  it("renders config page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/config/page");
    render(<Page />);
    expect(screen.getByText("config.title")).toBeInTheDocument();
  });

  it("renders graph redirect page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/graph/page");
    const { container } = render(<Page />);
    // Graph page is a redirect — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders integrations page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/integrations/page");
    render(<Page />);
    expect(screen.getByText("integrations.title")).toBeInTheDocument();
  });

  it("renders logs page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/logs/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders memory page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/memory/page");
    render(<Page />);
    expect(screen.getByText("memory.title")).toBeInTheDocument();
  });

  it("renders providers page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/providers/page");
    render(<Page />);
    expect(screen.getByText("providers.title")).toBeInTheDocument();
  });

  it("renders providers page with Graph active provider row", async () => {
    // Override useProviders to return a provider so Graph row renders
    const queries = await import("@/lib/queries");
    const origProviders = queries.useProviders;
    const origActive = queries.useProviderActive;
    (queries as Record<string, unknown>).useProviders = () => ({
      ...emptyQuery,
      data: [{ id: "uuid", name: "test-provider", type: "text", provider_type: "openai", base_url: null, default_model: "gpt-4o-mini", has_api_key: false, api_key: null, enabled: true, options: {}, notes: null, created_at: "", updated_at: "" }],
    });
    (queries as Record<string, unknown>).useProviderActive = () => ({
      ...emptyQuery,
      data: [{ capability: "graph_extraction", provider_name: "test-provider" }],
    });
    try {
      const { default: Page } = await import("@/app/(authenticated)/providers/page");
      render(<Page />);
      expect(screen.getByText("providers.cap_graph")).toBeInTheDocument();
    } finally {
      (queries as Record<string, unknown>).useProviders = origProviders;
      (queries as Record<string, unknown>).useProviderActive = origActive;
    }
  });

  it("renders secrets page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/secrets/page");
    render(<Page />);
    expect(screen.getByText("secrets.title")).toBeInTheDocument();
  });

  it("renders skills page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/skills/page");
    render(<Page />);
    expect(screen.getByText("skills.title")).toBeInTheDocument();
  });

  it("renders statistics page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/statistics/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders tasks page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/tasks/page");
    render(<Page />);
    // Tasks page component is actually CronPage, uses cron.title key
    expect(screen.getByText("cron.title")).toBeInTheDocument();
  });

  it("renders tools page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/tools/page");
    render(<Page />);
    expect(screen.getByText("tools.title")).toBeInTheDocument();
  });

  it("renders watchdog page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/watchdog/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders webhooks page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/webhooks/page");
    render(<Page />);
    expect(screen.getByText("webhooks.title")).toBeInTheDocument();
  });

  it("renders workspace page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/workspace/page");
    const { container } = render(<Page />);
    expect(container).toBeTruthy();
  });

  it("renders doctor page", async () => {
    const { default: Page } = await import("@/app/(authenticated)/doctor/page");
    const { container } = render(<Page />);
    // Page is a redirect stub — renders nothing
    expect(container).toBeTruthy();
  });

  it("renders setup page", async () => {
    const { default: Page } = await import("@/app/setup/page");
    render(<Page />);
    // Setup page starts at the requirements step
    expect(screen.getByText("setup.step_requirements")).toBeInTheDocument();
  });

});
