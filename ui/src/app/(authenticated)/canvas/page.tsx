"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

/** Canvas moved into chat as split panel — redirect old route */
export default function CanvasRedirect() {
  const router = useRouter();
  useEffect(() => { router.replace("/chat"); }, [router]);
  return null;
}
