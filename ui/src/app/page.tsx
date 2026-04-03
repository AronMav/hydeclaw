"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useAuthStore } from "@/stores/auth-store";

export default function RootPage() {
  const router = useRouter();
  const isAuthenticated = useAuthStore((s) => s.isAuthenticated);
  const restore = useAuthStore((s) => s.restore);

  useEffect(() => {
    restore().then((ok) => {
      router.replace(ok ? "/chat" : "/login");
    });
  }, [restore, router]);

  if (isAuthenticated) {
    router.replace("/chat");
    return null;
  }

  return (
    <div className="flex h-dvh items-center justify-center">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
    </div>
  );
}
