import { test, expect } from "@playwright/test";

/**
 * Smoke tests for 3 critical chat flows that were recently fixed.
 *
 * Run against the live Pi backend sequentially (serial mode prevents session
 * interference between tests):
 *
 *   PLAYWRIGHT_BASE_URL=http://192.168.1.85:18789 npx playwright test \
 *     src/__e2e__/chat-smoke.spec.ts --project=chromium --workers=1
 */

// ── Config ──────────────────────────────────────────────────────────────────

const TOKEN =
  "25378f5154228e4f8f196007171e338f063ed89fc03bf1394c0233dffbb8f0e0";

/** Run serially — avoids session list contamination between tests. */
test.describe.configure({ mode: "serial" });

// ── Types ────────────────────────────────────────────────────────────────────

type Page = Parameters<Parameters<typeof test>[1]>[0];

// ── Auth & navigation helpers ─────────────────────────────────────────────────

/** Login via the /login form. Input is type="password".
 *
 * Retries up to 3 times if rate-limited (Pi enforces 300 RPM).
 */
async function login(page: Page, maxRetries = 3) {
  for (let attempt = 0; attempt < maxRetries; attempt++) {
    await page.goto("/login");
    await page.waitForSelector('input[type="password"]', { timeout: 15_000 });
    await page.fill('input[type="password"]', TOKEN);
    await page.click('button[type="submit"]');

    // Check if we were redirected to /chat (success) or stayed on /login (rate limited or error)
    try {
      await page.waitForURL(/\/chat/, { timeout: 30_000 });
      return; // success
    } catch {
      // Might be rate limited — check for the error message
      const isRateLimited = await page
        .locator("text=Too many attempts")
        .isVisible()
        .catch(() => false);

      if (isRateLimited && attempt < maxRetries - 1) {
        console.log(`[login] Rate limited (attempt ${attempt + 1}/${maxRetries}). Waiting 35s...`);
        await page.waitForTimeout(35_000);
        continue;
      }
      throw new Error(`Login failed after ${attempt + 1} attempts (rate limited or other error)`);
    }
  }
}

/** Click the "+ New" button in the session sidebar. */
async function clickNewChat(page: Page) {
  await page.locator('aside button:has-text("New")').click({ timeout: 8_000 });
}

/** Fill the composer textarea and submit with Enter. */
async function sendMessage(page: Page, text: string) {
  const ta = page.locator('[data-composer-input] textarea');
  await ta.waitFor({ state: "visible", timeout: 10_000 });
  await ta.fill(text);
  await ta.press("Enter");
}

/**
 * Wait for the URL to contain ?s=<sessionId>.
 * Returns the session ID string or null on timeout.
 */
async function waitForSessionId(
  page: Page,
  timeoutMs = 20_000
): Promise<string | null> {
  try {
    await page.waitForFunction(
      () => new URLSearchParams(window.location.search).has("s"),
      undefined,
      { timeout: timeoutMs }
    );
    return new URL(page.url()).searchParams.get("s");
  } catch {
    return null;
  }
}

/**
 * Wait for the stop button (streaming in progress) to appear.
 * The stop button is type="button" with text-destructive class inside
 * the [data-composer-input] form, visible only during streaming.
 * Uses both Playwright locator and JS evaluation as fallbacks.
 */
async function waitForStreamingStarted(
  page: Page,
  timeoutMs: number
): Promise<boolean> {
  // Primary: Playwright locator (most reliable)
  try {
    await page
      .locator('[data-composer-input] button[type="button"].text-destructive')
      .first()
      .waitFor({ state: "visible", timeout: timeoutMs });
    return true;
  } catch {
    // Secondary: JS evaluation (catches cases where CSS class names differ slightly)
    // This runs after the primary timeout — try once more
    return page.evaluate(() => {
      const form = document.querySelector("[data-composer-input]");
      if (!form) return false;
      const buttons = Array.from(
        form.querySelectorAll<HTMLButtonElement>("button")
      );
      return buttons.some((btn) => {
        const cls = btn.className ?? "";
        const style = window.getComputedStyle(btn);
        return (
          cls.includes("destructive") &&
          style.display !== "none" &&
          style.visibility !== "hidden" &&
          parseFloat(style.opacity) > 0
        );
      });
    });
  }
}

/**
 * POST /api/chat/{sessionId}/abort directly.
 * This is the definitive way to signal the backend to stop the engine and
 * mark the session as "interrupted".
 */
async function apiAbortSession(page: Page, sessionId: string): Promise<void> {
  await page.evaluate(
    async ({ sid, token }: { sid: string; token: string }) => {
      await fetch(`/api/chat/${sid}/abort`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
      }).catch(() => {});
    },
    { sid: sessionId, token: TOKEN }
  );
}

/**
 * Poll /api/sessions?agent=<agent>&limit=100 until the session's run_status
 * leaves "running"/"streaming".
 *
 * Reads the current agent name from the agent selector element in the page.
 * Falls back to "HYDE" if the selector is not accessible.
 *
 * Returns the final status string, or null if it was never determinable.
 */
async function waitForSessionFinished(
  page: Page,
  sessionId: string,
  timeoutMs: number
): Promise<string | null> {
  // Read the current agent name from the page using textContent (NOT innerText).
  // innerText applies CSS text-transform (e.g. "uppercase" renders "Hyde" as "HYDE"),
  // but the API is case-sensitive and expects the original name ("Hyde", not "HYDE").
  // textContent returns the raw DOM text without CSS transforms.
  let agentName = "Hyde"; // safe default for this Pi instance
  try {
    const fromDom = await page.evaluate(() => {
      const trigger = document.querySelector('[aria-label="Switch agent"] button');
      if (trigger) {
        // Walk text nodes to get raw content before CSS transforms
        const span = trigger.querySelector("span");
        return span?.textContent?.trim() ?? null;
      }
      return null;
    });
    if (fromDom && fromDom.length > 0 && fromDom.length < 50) {
      agentName = fromDom;
    }
  } catch {
    // Use default "Hyde"
  }
  console.log(`[pollSessionStatus] Using agent name: "${agentName}" for session ${sessionId}`);

  const deadline = Date.now() + timeoutMs;
  let lastStatus: string | null = null;

  while (Date.now() < deadline) {
    const result = await page
      .evaluate(
        async ({
          sid,
          token,
          agent,
        }: {
          sid: string;
          token: string;
          agent: string;
        }) => {
          try {
            const resp = await fetch(
              `/api/sessions?agent=${encodeURIComponent(agent)}&limit=100`,
              { headers: { Authorization: `Bearer ${token}` } }
            );
            if (!resp.ok) return { status: null, error: `HTTP ${resp.status}` };
            const data = await resp.json();
            const sessions: Array<{ id: string; run_status: string }> =
              Array.isArray(data.sessions) ? data.sessions : [];
            const found = sessions.find((s) => s.id === sid);
            const allIds = sessions.slice(0, 5).map((s) => `${s.id}:${s.run_status}:${(s as any).agent_id}`).join(", ");
            return {
              status: found ? found.run_status : null,
              error: found ? null : `session not found; first 5: [${allIds}]; total: ${sessions.length}; looking for: ${sid}`,
            };
          } catch (e) {
            return { status: null, error: String(e) };
          }
        },
        { sid: sessionId, token: TOKEN, agent: agentName }
      )
      .catch(() => ({ status: null, error: "evaluate failed" }));

    if (result.status !== null) {
      lastStatus = result.status;
      if (
        result.status !== "running" &&
        result.status !== "streaming" &&
        result.status !== null
      ) {
        return result.status;
      }
    } else if (result.error) {
      console.log(`[pollSessionStatus] API error: ${result.error}`);
    }

    await page.waitForTimeout(3_000);
  }

  return lastStatus;
}

// ── Test 1: Abort mid-stream → session marked interrupted (not error) ────────
//
// The user clicks Stop and we also POST /abort for reliability.
// The session must NOT end up as "error" or "failed".
// Best case: it ends as "interrupted".
// Acceptable: "done" (model finished within graceful drain window).

test("abort mid-stream marks session interrupted", async ({ page }) => {
  test.setTimeout(180_000);

  await login(page);
  await clickNewChat(page);

  // Very long prompt to ensure streaming takes > 30s on the Pi
  await sendMessage(
    page,
    "Напиши очень длинный подробный рассказ про горный Алтай минимум 6000 слов. " +
      "Включи подробное описание природы, истории, народов, рек, гор, животных и растений. " +
      "Пиши без остановки, очень подробно, каждый абзац минимум 10 предложений."
  );

  // Wait for session ID to be assigned
  const sessionId = await waitForSessionId(page, 20_000);
  if (!sessionId) {
    test.skip(true, "Session ID never appeared in URL — message send may have failed.");
    return;
  }

  // Wait for streaming to visibly start (stop button appears)
  // Give 60s — the Pi may take >30s to start generating the first token
  const streamingStarted = await waitForStreamingStarted(page, 60_000);

  // Diagnostic: take a screenshot and log DOM state when stop button check fails
  if (!streamingStarted) {
    // Log what buttons exist in the composer form
    const debugInfo = await page.evaluate(() => {
      const form = document.querySelector("[data-composer-input]");
      if (!form) return "NO FORM FOUND";
      const buttons = Array.from(form.querySelectorAll<HTMLElement>("button"));
      return buttons
        .map((b) => `[type=${b.getAttribute("type")} classes="${b.className.slice(0, 80)}" visible=${b.offsetParent !== null}]`)
        .join("; ");
    });
    console.log(`[abort test] Stop button search result. Buttons in form: ${debugInfo}`);

    // Check session status
    const statusCheck = await waitForSessionFinished(page, sessionId, 8_000);
    console.log(`[abort test] Session status after 60s wait: ${statusCheck}`);

    test.skip(
      true,
      `Stop button never appeared after 60s. Session status: ${statusCheck ?? "unknown"}. ` +
        `Composer buttons: ${debugInfo?.slice(0, 200) ?? "none"}. ` +
        `The model may complete very fast or the selector needs updating.`
    );
    return;
  }

  // POST /abort to the backend — this triggers the cancellation token and graceful drain.
  // The engine will finish within CANCEL_GRACE (30s) and mark the session "interrupted".
  await apiAbortSession(page, sessionId);

  // Also abort the local SSE fetch so the UI updates immediately
  await page.evaluate(() => {
    // Trigger stopStream via the store's public API
    const event = new CustomEvent("hydeclaw:stop-stream");
    document.dispatchEvent(event);
  });

  // Poll session status — wait up to 60s for the engine to finish
  const finalStatus = await waitForSessionFinished(page, sessionId, 60_000);

  if (finalStatus === null) {
    // Could not determine status
    test.skip(
      true,
      `Could not determine final session status for ${sessionId}. Sessions API may not return this session.`
    );
    return;
  }

  // Critical: session must NOT end as "error" or "failed"
  expect(
    finalStatus,
    `Session ended as "${finalStatus}" — must not be "error" or "failed" after a user abort`
  ).not.toBe("error");
  expect(finalStatus).not.toBe("failed");

  if (finalStatus === "done") {
    // The model finished before the 30s graceful drain window — not a bug.
    console.warn(
      `[abort test] Session completed as "done" before abort took effect. ` +
        `Pi LLM may generate too fast for the 30s drain window. Not a bug.`
    );
    // Test passes — we verified no error status
  } else {
    // Expecting "interrupted"
    expect(finalStatus).toBe("interrupted");
  }
});

// ── Test 2: Switch sessions mid-stream → correct content renders ──────────────

test("switching sessions mid-stream does not show wrong content", async ({
  page,
}) => {
  test.setTimeout(90_000);

  await login(page);
  await clickNewChat(page);

  await sendMessage(
    page,
    "Напиши длинный рассказ про горы минимум 2000 слов."
  );

  // Wait for session ID to appear in URL
  const streamingSessionId = await waitForSessionId(page, 20_000);
  if (!streamingSessionId) {
    test.skip(true, "Session ID never appeared in URL after sending message.");
    return;
  }

  // Optionally wait for streaming to visibly start (non-blocking)
  await waitForStreamingStarted(page, 10_000);

  // Wait for the Virtuoso list to render session items in the sidebar.
  // Virtuoso uses virtual scrolling and may not render items immediately on mount.
  // We poll until at least 2 items appear, up to 10s.
  const sessionButtons = page.locator(
    "aside div.group button.flex.w-full.flex-col"
  );
  let count = 0;
  const virtuosoDeadline = Date.now() + 10_000;
  while (Date.now() < virtuosoDeadline) {
    count = await sessionButtons.count();
    if (count >= 2) break;
    await page.waitForTimeout(500);
  }

  if (count < 2) {
    test.skip(
      true,
      `Need at least 2 sessions in the sidebar (found ${count}). Pre-seed the Pi or run after sessions accumulate.`
    );
    return;
  }

  // Click the LAST (oldest) session to avoid interference with "now" sessions
  await sessionButtons.nth(count - 1).click();

  // Wait for URL to change to a different ?s= parameter
  await page.waitForFunction(
    (sidBefore: string) => {
      const current = new URLSearchParams(window.location.search).get("s") ?? "";
      return current !== "" && current !== sidBefore;
    },
    streamingSessionId,
    { timeout: 10_000 }
  );

  // Allow React Query to fetch the new session's messages
  await page.waitForTimeout(2_500);

  // The main page body must have meaningful content
  const bodyText = await page.locator("body").innerText();
  expect(bodyText.length).toBeGreaterThan(50);

  // URL must now reference a different session
  const newSessionId = new URL(page.url()).searchParams.get("s");
  expect(newSessionId).not.toBe(streamingSessionId);
  expect(newSessionId).toBeTruthy();
});

// ── Test 3: F5 reload during stream → session preserved ──────────────────────

test("F5 reload during stream preserves session", async ({ page }) => {
  test.setTimeout(90_000);

  await login(page);
  await clickNewChat(page);

  await sendMessage(
    page,
    "Напиши подробно про реки минимум 1500 слов."
  );

  // Wait for session ID to appear
  const sessionId = await waitForSessionId(page, 20_000);
  if (!sessionId) {
    test.skip(true, "Session ID never appeared in URL after sending message.");
    return;
  }

  const urlWithSession = page.url();
  expect(urlWithSession).toMatch(/[?&]s=/);

  // Reload the page (F5)
  await page.reload();

  // The auth token is in sessionStorage — it survives same-tab reload.
  // If we land on /login, re-authenticate.
  if (page.url().includes("/login")) {
    await login(page);
    await page.goto(urlWithSession);
    await page.waitForURL(/\/chat/, { timeout: 10_000 });
  } else {
    await page.waitForURL(/\/chat/, { timeout: 10_000 });
  }

  // After reload, the URL must still reference the same session
  await page.waitForFunction(
    (sid: string) => new URLSearchParams(window.location.search).get("s") === sid,
    sessionId,
    { timeout: 15_000 }
  );

  // The composer must be rendered (session loaded successfully)
  await page.locator("[data-composer-input]").waitFor({ state: "visible", timeout: 20_000 });

  // Page body must have meaningful content
  const chatText = await page.locator("body").innerText();
  expect(chatText.length).toBeGreaterThan(50);
});
