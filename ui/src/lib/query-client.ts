import { QueryClient } from "@tanstack/react-query";

/**
 * Singleton QueryClient — shared between query-provider.tsx (React tree)
 * and chat-store.ts (imperative invalidation after SSE stream).
 * gcTime: 24h so IDB-restored data isn't garbage-collected immediately.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      gcTime: 24 * 60 * 60 * 1000,
      retry: 1,
      refetchOnWindowFocus: true,
    },
  },
});
