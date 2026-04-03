"use client"
import { PersistQueryClientProvider } from "@tanstack/react-query-persist-client"
import { createIDBPersister } from "@/lib/idb-persister"
import { queryClient } from "@/lib/query-client"

const persister = createIDBPersister()

export function QueryProvider({ children }: { children: React.ReactNode }) {
  return (
    <PersistQueryClientProvider
      client={queryClient}
      persistOptions={{
        persister,
        maxAge: 24 * 60 * 60 * 1000,
        dehydrateOptions: {
          // Only persist session-related queries — admin page caches not needed across refreshes
          shouldDehydrateQuery: (query) =>
            (query.queryKey as unknown[])[0] === "sessions",
        },
      }}
    >
      {children}
    </PersistQueryClientProvider>
  )
}
