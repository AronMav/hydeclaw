import { useAuthStore } from "@/stores/auth-store";

const REQUEST_TIMEOUT = 30_000;

export function getToken(): string {
  return useAuthStore.getState().token;
}

let redirecting = false;
/** Reset redirect guard (for tests only). */
export function _resetRedirecting() { redirecting = false; }
function handleUnauthorized() {
  if (redirecting) return;
  redirecting = true;
  useAuthStore.getState().logout();
  window.location.href = "/login";
}

async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  // If already redirecting to login, don't make more requests (prevents rate limit lockout)
  if (redirecting) {
    throw new Error("Session expired");
  }

  const token = getToken();
  if (!token) {
    handleUnauthorized();
    throw new Error("Session expired");
  }

  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(init?.headers as Record<string, string>),
  };
  headers["Authorization"] = `Bearer ${token}`;

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), REQUEST_TIMEOUT);

  try {
    const signal = init?.signal
      ? AbortSignal.any([init.signal, controller.signal])
      : controller.signal;

    const resp = await fetch(path, {
      ...init,
      headers,
      signal,
    });
    if (resp.status === 401) {
      handleUnauthorized();
      throw new Error("Session expired");
    }
    if (resp.status === 429) {
      throw new Error("Too many failed attempts. Try again later.");
    }
    return resp;
  } finally {
    clearTimeout(timeout);
  }
}

async function extractError(resp: Response): Promise<string> {
  const text = await resp.text().catch(() => "");
  try {
    const data = JSON.parse(text);
    if (data && typeof data === "object" && "error" in data) {
      return (data as { error: string }).error;
    }
  } catch {
    // not JSON
  }
  return text || `HTTP ${resp.status}`;
}

export async function apiGet<T>(path: string): Promise<T> {
  const resp = await apiFetch(path);
  if (!resp.ok) throw new Error(await extractError(resp));
  return resp.json();
}

export async function apiPost<T>(path: string, body?: unknown, extraHeaders?: Record<string, string>): Promise<T> {
  const resp = await apiFetch(path, {
    method: "POST",
    body: body != null ? JSON.stringify(body) : undefined,
    headers: extraHeaders,
  });
  if (!resp.ok) throw new Error(await extractError(resp));
  return resp.json();
}

export async function apiPut<T>(path: string, body?: unknown): Promise<T> {
  const resp = await apiFetch(path, {
    method: "PUT",
    body: body != null ? JSON.stringify(body) : undefined,
  });
  if (!resp.ok) throw new Error(await extractError(resp));
  return resp.json();
}

export async function apiPatch<T>(path: string, body?: unknown): Promise<T> {
  const resp = await apiFetch(path, {
    method: "PATCH",
    body: body != null ? JSON.stringify(body) : undefined,
  });
  if (!resp.ok) throw new Error(await extractError(resp));
  return resp.json();
}

export async function apiDelete(path: string): Promise<void> {
  const resp = await apiFetch(path, { method: "DELETE" });
  if (!resp.ok) throw new Error(await extractError(resp));
}

export async function inviteAgent(sessionId: string, agentName: string): Promise<{ participants: string[] }> {
  return apiPost<{ participants: string[] }>(`/api/sessions/${sessionId}/invite`, { agent_name: agentName });
}
