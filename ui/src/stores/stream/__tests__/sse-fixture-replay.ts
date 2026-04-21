// ui/src/stores/stream/__tests__/sse-fixture-replay.ts
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * Reads an SSE fixture file and returns a ReadableStream that emits
 * the fixture's bytes in configurable chunks, with a configurable
 * delay between chunks. This lets tests exercise both "all-at-once"
 * (chunk=Infinity, delay=0) and "realistic streaming"
 * (chunk=64 bytes, delay=10ms) modes.
 */
export function createFixtureStream(
  fixtureName: string,
  opts: { chunkBytes?: number; delayMs?: number } = {},
): ReadableStream<Uint8Array> {
  const fixturePath = path.join(__dirname, "fixtures", fixtureName);
  const raw = fs.readFileSync(fixturePath);
  const chunkBytes = opts.chunkBytes ?? raw.length;
  const delayMs = opts.delayMs ?? 0;

  let offset = 0;
  return new ReadableStream({
    async pull(controller) {
      if (offset >= raw.length) {
        controller.close();
        return;
      }
      const end = Math.min(offset + chunkBytes, raw.length);
      const chunk = raw.subarray(offset, end);
      controller.enqueue(new Uint8Array(chunk));
      offset = end;
      if (delayMs > 0) {
        await new Promise((r) => setTimeout(r, delayMs));
      }
    },
  });
}

/** Count lines starting with `data:` in the fixture — for smoke asserts. */
export function countDataLines(fixtureName: string): number {
  const fixturePath = path.join(__dirname, "fixtures", fixtureName);
  const raw = fs.readFileSync(fixturePath, "utf8");
  return raw.split("\n").filter((l) => l.startsWith("data:")).length;
}
