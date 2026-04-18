/**
 * Harness-based integration test for the sentinel → React state flow
 * used inside MessageList. Avoids mocking Virtuoso + chat store +
 * session router (which would dwarf the code under test).
 *
 * The Harness reproduces the same useEffect pattern MessageList uses:
 *   - lookup scroller + sentinel from the DOM
 *   - attachTailSentinel
 *   - forward callback to setState + ref
 *   - teardown on unmount
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, act } from "@testing-library/react";
import { useRef, useState, useEffect } from "react";
import { attachTailSentinel } from "../tail-sentinel";

/** Reuse the same IO mock shape from the unit tests. */
class MockIntersectionObserver {
  static instances: MockIntersectionObserver[] = [];
  readonly callback: IntersectionObserverCallback;
  readonly options: IntersectionObserverInit | undefined;
  readonly observed: Element[] = [];
  disconnected = false;

  constructor(cb: IntersectionObserverCallback, options?: IntersectionObserverInit) {
    this.callback = cb;
    this.options = options;
    MockIntersectionObserver.instances.push(this);
  }

  observe(el: Element) { this.observed.push(el); }
  unobserve() {}
  disconnect() { this.disconnected = true; }
  takeRecords() { return []; }

  fire(isIntersecting: boolean) {
    this.callback(
      [{ isIntersecting, target: this.observed[0] } as unknown as IntersectionObserverEntry],
      this as unknown as IntersectionObserver,
    );
  }

  static last() { return this.instances.at(-1)!; }
}

beforeEach(() => {
  MockIntersectionObserver.instances = [];
  vi.stubGlobal("IntersectionObserver", MockIntersectionObserver);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

/** Minimal reproduction of the MessageList attach pattern. */
function Harness({ onState }: { onState: (s: { isAtTail: boolean; ref: boolean }) => void }) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const isAtTailRef = useRef<boolean>(true);
  const [isAtTail, setIsAtTail] = useState<boolean>(true);

  useEffect(() => {
    const scroller = scrollerRef.current;
    const sentinel = sentinelRef.current;
    if (!scroller || !sentinel) return;

    const detach = attachTailSentinel(scroller, sentinel, (atTail) => {
      isAtTailRef.current = atTail;
      setIsAtTail((prev) => (prev === atTail ? prev : atTail));
    });
    return detach;
  }, []);

  onState({ isAtTail, ref: isAtTailRef.current });

  return (
    <div ref={scrollerRef}>
      <div ref={sentinelRef} data-testid="sentinel" />
    </div>
  );
}

describe("tail-sentinel integration — Harness", () => {
  it("initial state is isAtTail=true", () => {
    let last: { isAtTail: boolean; ref: boolean } | null = null;
    render(<Harness onState={(s) => { last = s; }} />);
    expect(last!.isAtTail).toBe(true);
    expect(last!.ref).toBe(true);
  });

  it("attaches IO with the scroller as root and observes the sentinel", () => {
    render(<Harness onState={() => {}} />);
    const io = MockIntersectionObserver.last();
    expect(io.options?.root).toBeInstanceOf(HTMLDivElement);
    expect(io.observed).toHaveLength(1);
    expect((io.observed[0] as HTMLElement).dataset.testid).toBe("sentinel");
  });

  it("IO false transition flips isAtTail → false (both state and ref)", () => {
    let last: { isAtTail: boolean; ref: boolean } | null = null;
    render(<Harness onState={(s) => { last = s; }} />);

    act(() => {
      MockIntersectionObserver.last().fire(false);
    });

    expect(last!.isAtTail).toBe(false);
    expect(last!.ref).toBe(false);
  });

  it("IO true transition after false restores isAtTail → true", () => {
    let last: { isAtTail: boolean; ref: boolean } | null = null;
    render(<Harness onState={(s) => { last = s; }} />);

    act(() => { MockIntersectionObserver.last().fire(false); });
    act(() => { MockIntersectionObserver.last().fire(true); });

    expect(last!.isAtTail).toBe(true);
    expect(last!.ref).toBe(true);
  });

  it("unmount disconnects the observer", () => {
    const { unmount } = render(<Harness onState={() => {}} />);
    const io = MockIntersectionObserver.last();
    expect(io.disconnected).toBe(false);
    unmount();
    expect(io.disconnected).toBe(true);
  });

  it("duplicate true callbacks do not cause extra re-renders (state dedupes)", () => {
    let renderCount = 0;
    render(
      <Harness
        onState={() => {
          renderCount += 1;
        }}
      />,
    );
    const before = renderCount;

    act(() => { MockIntersectionObserver.last().fire(true); });
    act(() => { MockIntersectionObserver.last().fire(true); });

    // The state setter returns prev unchanged; React skips the re-render.
    expect(renderCount - before).toBeLessThanOrEqual(0);
  });
});
