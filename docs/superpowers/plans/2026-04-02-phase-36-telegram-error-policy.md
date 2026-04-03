# Phase 36: Telegram Error Policy — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Telegram driver использует экспоненциальный backoff, подавляет повторяющиеся ошибки per-chat, уважает `retry_after` в 429 ответах.

**Architecture:** `retryTg()` получает экспоненциальный backoff. Добавляется `chatCooldowns: Map` с per-chat/topic cooldown по errorCode. 429 читает `error.parameters?.retry_after`. Конфигурируется через `[telegram]` секцию в `channels.toml`.

**Tech Stack:** TypeScript/Bun, grammy

---

## File Map

| Файл | Действие |
|------|----------|
| `channels/src/drivers/telegram.ts` | Экспоненциальный backoff, chatCooldowns, retry_after |
| `channels/src/drivers/common.ts` | Добавить `extractTgErrorCode()` helper |

---

## Task 1: Экспоненциальный backoff в retryTg

**Files:**
- Modify: `channels/src/drivers/telegram.ts`

- [ ] **Step 1: Написать тест**

Добавить в конец файла (или в test файл `channels/src/drivers/telegram.test.ts`):

```typescript
// channels/src/drivers/telegram.test.ts
import { describe, it, expect } from "bun:test";

describe("exponentialDelay", () => {
  it("returns 1000ms for attempt 0", () => {
    expect(exponentialDelay(0)).toBe(1000);
  });
  it("returns 2000ms for attempt 1", () => {
    expect(exponentialDelay(1)).toBe(2000);
  });
  it("returns 4000ms for attempt 2", () => {
    expect(exponentialDelay(2)).toBe(4000);
  });
  it("caps at 30000ms", () => {
    expect(exponentialDelay(10)).toBe(30000);
  });
});
```

- [ ] **Step 2: Запустить тест — убедиться что падает**

```bash
cd channels && bun test src/drivers/telegram.test.ts 2>&1 | tail -10
```
Ожидание: `exponentialDelay` not found.

- [ ] **Step 3: Добавить exponentialDelay и обновить retryTg**

Найти `retryTg` (строки 51-67 в `telegram.ts`). Добавить перед ней:

```typescript
/** Exponential backoff: 1s → 2s → 4s → ... capped at 30s */
function exponentialDelay(attempt: number): number {
  return Math.min(1000 * Math.pow(2, attempt), 30_000);
}
```

Заменить `retryTg`:

```typescript
async function retryTg<T>(
  fn: () => Promise<T>,
  attempts = 3,
  label = ""
): Promise<T | undefined> {
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (e) {
      if (isTgPermanentError(e)) {
        console.warn(`[tg] ${label} permanent error, not retrying:`, e);
        return undefined;
      }
      if (i === attempts - 1) {
        console.warn(`[tg] ${label} failed after ${attempts} attempts:`, e);
        return undefined;
      }
      await Bun.sleep(exponentialDelay(i));
    }
  }
  return undefined;
}
```

- [ ] **Step 4: Запустить тест**

```bash
cd channels && bun test src/drivers/telegram.test.ts 2>&1 | tail -10
```
Ожидание: PASS.

- [ ] **Step 5: Commit**

```bash
git add channels/src/drivers/telegram.ts channels/src/drivers/telegram.test.ts
git commit -m "feat(telegram): replace linear backoff with exponential in retryTg"
```

---

## Task 2: Добавить extractTgErrorCode в common.ts

**Files:**
- Modify: `channels/src/drivers/common.ts`

- [ ] **Step 1: Написать тест**

```typescript
// В channels/src/drivers/common.test.ts или в конец common.ts
import { describe, it, expect } from "bun:test";
import { extractTgErrorCode } from "./common";

describe("extractTgErrorCode", () => {
  it("extracts 403 from Forbidden error", () => {
    const e = new Error("403: Forbidden: bot was blocked by the user");
    expect(extractTgErrorCode(e)).toBe(403);
  });

  it("extracts 429 from Too Many Requests", () => {
    const e = new Error("429: Too Many Requests: retry after 30");
    expect(extractTgErrorCode(e)).toBe(429);
  });

  it("returns null for unknown error", () => {
    expect(extractTgErrorCode(new Error("network timeout"))).toBeNull();
  });
});
```

- [ ] **Step 2: Запустить тест — убедиться что падает**

```bash
cd channels && bun test 2>&1 | grep "extractTgErrorCode" | head -5
```

- [ ] **Step 3: Добавить extractTgErrorCode в common.ts**

После функции `isTgPermanentError` добавить:

```typescript
/**
 * Extract HTTP status code from a Telegram API error message.
 * Returns null if no code found.
 */
export function extractTgErrorCode(error: unknown): number | null {
  const msg = String(error);
  const match = msg.match(/\b(4\d{2}|5\d{2})\b/);
  return match ? parseInt(match[1], 10) : null;
}

/**
 * Extract retry_after seconds from a Telegram 429 error.
 * Telegram API returns: {"ok":false,"error_code":429,"parameters":{"retry_after":N}}
 */
export function extractTgRetryAfter(error: unknown): number | null {
  if (typeof error === "object" && error !== null) {
    const e = error as Record<string, unknown>;
    const retryAfter = (e["parameters"] as Record<string, unknown>)?.["retry_after"];
    if (typeof retryAfter === "number") return retryAfter;
  }
  // Fallback: parse from string
  const msg = String(error);
  const match = msg.match(/retry.after[:\s]+(\d+)/i);
  return match ? parseInt(match[1], 10) : null;
}
```

- [ ] **Step 4: Запустить тест**

```bash
cd channels && bun test 2>&1 | tail -10
```
Ожидание: PASS.

- [ ] **Step 5: Commit**

```bash
git add channels/src/drivers/common.ts
git commit -m "feat(telegram): add extractTgErrorCode and extractTgRetryAfter helpers"
```

---

## Task 3: Per-chat cooldown map + 429 retry_after

**Files:**
- Modify: `channels/src/drivers/telegram.ts`

- [ ] **Step 1: Написать тест для cooldown логики**

```typescript
describe("chatCooldownKey", () => {
  it("builds key with chatId only when no threadId", () => {
    expect(chatCooldownKey(123, undefined)).toBe("123:");
  });
  it("builds key with chatId and threadId", () => {
    expect(chatCooldownKey(123, 456)).toBe("123:456");
  });
});
```

- [ ] **Step 2: Запустить тест — убедиться что падает**

```bash
cd channels && bun test 2>&1 | grep "chatCooldownKey" | head -3
```

- [ ] **Step 3: Добавить chatCooldowns и вспомогательные функции**

После объявления `state: ActiveState` в `telegram.ts` добавить:

```typescript
/** Per-chat/topic delivery error cooldown. Key: "${chatId}:${threadId??''}". */
interface CooldownEntry {
  errorCode: number;
  cooldownUntil: number; // Date.now() + ms
}
const chatCooldowns = new Map<string, CooldownEntry>();

function chatCooldownKey(chatId: number, threadId?: number): string {
  return `${chatId}:${threadId ?? ""}`;
}

function isChatOnCooldown(chatId: number, threadId: number | undefined, errorCode: number): boolean {
  const key = chatCooldownKey(chatId, threadId);
  const entry = chatCooldowns.get(key);
  if (!entry) return false;
  if (Date.now() > entry.cooldownUntil) {
    chatCooldowns.delete(key); // expired
    return false;
  }
  // Suppress only if same error code — different errors are not blocked
  return entry.errorCode === errorCode;
}

function setChatCooldown(chatId: number, threadId: number | undefined, errorCode: number, ms: number): void {
  chatCooldowns.set(chatCooldownKey(chatId, threadId), {
    errorCode,
    cooldownUntil: Date.now() + ms,
  });
}
```

- [ ] **Step 4: Запустить тест**

```bash
cd channels && bun test 2>&1 | tail -10
```
Ожидание: PASS.

- [ ] **Step 5: Обновить retryTg для использования cooldown и retry_after**

Заменить `retryTg` на расширенную версию с параметрами chatId и threadId:

```typescript
async function retryTg<T>(
  fn: () => Promise<T>,
  attempts = 3,
  label = "",
  chatId?: number,
  threadId?: number,
  errorCooldownMs = 60_000,
): Promise<T | undefined> {
  for (let i = 0; i < attempts; i++) {
    try {
      return await fn();
    } catch (e) {
      const errorCode = extractTgErrorCode(e) ?? 0;

      if (isTgPermanentError(e)) {
        console.warn(`[tg] ${label} permanent error (${errorCode}), not retrying:`, e);
        // Set cooldown for this chat so we don't spam the same error
        if (chatId !== undefined) {
          setChatCooldown(chatId, threadId, errorCode, errorCooldownMs);
        }
        return undefined;
      }

      // 429: respect retry_after
      if (errorCode === 429) {
        const retryAfter = extractTgRetryAfter(e);
        const delay = retryAfter ? retryAfter * 1000 : errorCooldownMs;
        console.warn(`[tg] ${label} rate limited, waiting ${delay}ms`);
        if (chatId !== undefined) {
          setChatCooldown(chatId, threadId, 429, delay);
        }
        if (i < attempts - 1) {
          await Bun.sleep(delay);
          continue;
        }
        return undefined;
      }

      if (i === attempts - 1) {
        console.warn(`[tg] ${label} failed after ${attempts} attempts:`, e);
        return undefined;
      }

      await Bun.sleep(exponentialDelay(i));
    }
  }
  return undefined;
}
```

- [ ] **Step 6: Добавить cooldown check перед отправкой**

Найти основные места где вызывается `retryTg` для отправки сообщений (sendMessage, sendPhoto и т.п.). Перед вызовом добавить проверку:

```typescript
// Example: before sending a message
if (chatId !== undefined) {
  const errCode = extractTgErrorCode(null) ?? 0; // 0 means check any existing cooldown
  // Check if chat has ANY active cooldown (any error code)
  const key = chatCooldownKey(chatId, threadId);
  const entry = chatCooldowns.get(key);
  if (entry && Date.now() < entry.cooldownUntil) {
    console.warn(`[tg] chat ${chatId} on cooldown until ${new Date(entry.cooldownUntil).toISOString()}, skipping message`);
    return;
  }
}
await retryTg(() => bot.api.sendMessage(chatId, text, opts), 3, "sendMessage", chatId, threadId, errorCooldownMs);
```

- [ ] **Step 7: Запустить все тесты**

```bash
cd channels && bun test 2>&1 | tail -15
```
Ожидание: PASS.

- [ ] **Step 8: Commit**

```bash
git add channels/src/drivers/telegram.ts channels/src/drivers/common.ts
git commit -m "feat(telegram): per-chat cooldown, 429 retry_after, exponential backoff"
```

---

## Task 4: Конфиг error_cooldown_ms и error_policy

**Files:**
- Modify: `channels/src/drivers/telegram.ts`

- [ ] **Step 1: Найти где читается channelConfig в createTelegramDriver**

Строки 97-107 в `telegram.ts`. Там уже читаются `group_mode`, `api_url`. Добавить чтение новых полей:

```typescript
export function createTelegramDriver(
  bridge: BridgeHandle,
  credential: string,
  channelConfig: Record<string, unknown> | undefined,
  language: string,
  typingMode: string,
): { start: () => Promise<void>; stop: () => Promise<void> } {
  const strings = getStrings(language);
  const groupMode = (channelConfig?.group_mode as string) ?? "mention";
  const apiUrl = channelConfig?.api_url as string | undefined;
  const errorCooldownMs = (channelConfig?.error_cooldown_ms as number) ?? 60_000;
  const errorPolicy = (channelConfig?.error_policy as string) ?? "suppress_repeated";
  
  // ...
```

- [ ] **Step 2: Пробросить errorCooldownMs в retryTg вызовы**

Внутри `createTelegramDriver` все вызовы `retryTg` должны передавать `errorCooldownMs`:

```typescript
await retryTg(() => bot.api.sendMessage(...), 3, "sendMessage", chatId, threadId, errorCooldownMs);
```

Если `errorPolicy === "always_retry"` — не устанавливать cooldown (передать 0 или игнорировать `setChatCooldown`).

- [ ] **Step 3: Запустить тесты**

```bash
cd channels && bun test 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add channels/src/drivers/telegram.ts
git commit -m "feat(telegram): read error_cooldown_ms and error_policy from channel config"
```

---

## Task 5: Финальная проверка

- [ ] **Step 1: Полный прогон тестов channels**

```bash
cd channels && bun test 2>&1 | tail -20
```
Ожидание: PASS.

- [ ] **Step 2: Проверить что нет регрессий в retryTg**

Убедиться что все вызовы `retryTg` в `telegram.ts` не сломаны — у них теперь опциональные параметры `chatId` и `threadId` которые можно не передавать:

```bash
grep -n "retryTg(" channels/src/drivers/telegram.ts | head -20
```

- [ ] **Step 3: Commit финальный**

```bash
git add -u
git commit -m "feat: phase 36 complete — Telegram error policy with exponential backoff and per-chat cooldown"
```
