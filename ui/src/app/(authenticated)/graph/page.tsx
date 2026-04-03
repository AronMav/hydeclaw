"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

export default function GraphRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/memory/"); }, [router]);
  return null;
}
