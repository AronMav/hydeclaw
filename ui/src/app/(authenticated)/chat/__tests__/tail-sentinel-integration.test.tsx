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
import React, { useRef, useState, useEffect } from "react";
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

  it("re-attaches IO when the sentinel DOM node is replaced (Footer remount)", () => {
    // Reproduce the Virtuoso-remount hazard: if the sentinel element
    // identity changes, the effect must tear down the old observer
    // and attach a new one to the fresh node.
    function RemountHarness() {
      const scrollerRef = useRef<HTMLDivElement>(null);
      const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
      const [sentinelKey, setSentinelKey] = useState(0);

      useEffect(() => {
        const scroller = scrollerRef.current;
        if (!scroller || !sentinelEl) return;
        const detach = attachTailSentinel(scroller, sentinelEl, () => {});
        return detach;
      }, [sentinelEl]);

      return (
        <div ref={scrollerRef}>
          <div
            key={sentinelKey}
            ref={setSentinelEl}
            data-testid={`sentinel-${sentinelKey}`}
          />
          <button onClick={() => setSentinelKey((k) => k + 1)}>remount</button>
        </div>
      );
    }

    const { getByText } = render(<RemountHarness />);
    const firstInstanceCount = MockIntersectionObserver.instances.length;

    act(() => { getByText("remount").click(); });

    expect(MockIntersectionObserver.instances.length).toBe(
      firstInstanceCount + 1,
    );
    // Previous observer must be disconnected on teardown
    expect(MockIntersectionObserver.instances[firstInstanceCount - 1].disconnected).toBe(true);
  });

  it("missed-token counter resets when isAtTail transitions false → true", () => {
    // Prove the `useEffect([isAtTail])` reset path: simulate a token
    // accrual while the user is away from the tail, then re-enter and
    // confirm the counter is cleared.
    function MissedTokensHarness({
      bumpRef,
      onBadge,
    }: {
      bumpRef: React.MutableRefObject<(() => void) | null>;
      onBadge: (value: number) => void;
    }) {
      const scrollerRef = useRef<HTMLDivElement>(null);
      const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
      const [isAtTail, setIsAtTail] = useState(true);
      const [missed, setMissed] = useState(0);

      useEffect(() => {
        const scroller = scrollerRef.current;
        if (!scroller || !sentinelEl) return;
        const detach = attachTailSentinel(scroller, sentinelEl, (atTail) => {
          setIsAtTail((prev) => (prev === atTail ? prev : atTail));
        });
        return detach;
      }, [sentinelEl]);

      useEffect(() => {
        if (isAtTail) {
          setMissed(0);
        }
      }, [isAtTail]);

      // Expose a bump function the test can call while `isAtTail` is false.
      bumpRef.current = () => setMissed((m) => m + 1);

      onBadge(missed);
      return (
        <div ref={scrollerRef}>
          <div ref={setSentinelEl} />
        </div>
      );
    }

    const bumpRef: React.MutableRefObject<(() => void) | null> = { current: null };
    let lastBadge = -1;
    render(
      <MissedTokensHarness
        bumpRef={bumpRef}
        onBadge={(v) => { lastBadge = v; }}
      />,
    );
    // Initial mount reset: badge starts at 0 (isAtTail=true triggers reset effect).
    expect(lastBadge).toBe(0);

    // User leaves tail.
    act(() => { MockIntersectionObserver.last().fire(false); });
    expect(lastBadge).toBe(0);

    // Three tokens arrive while away — badge accrues.
    act(() => { bumpRef.current!(); });
    act(() => { bumpRef.current!(); });
    act(() => { bumpRef.current!(); });
    expect(lastBadge).toBe(3);

    // User re-enters tail — reset effect must fire and zero the badge.
    // Without the `useEffect([isAtTail])` reset, this would stay at 3.
    act(() => { MockIntersectionObserver.last().fire(true); });
    expect(lastBadge).toBe(0);
  });

  it("shouldFollow stays true during brief sentinel flicker (<1500ms)", async () => {
    // Reproduce the Virtuoso-catchup-lag scenario: sentinel briefly
    // leaves the tail zone (IO false), then re-enters (IO true)
    // within the debounce window. shouldFollow must NOT flip to
    // false during this flicker.
    vi.useFakeTimers();
    function DebounceHarness({ onState }: { onState: (s: { shouldFollow: boolean }) => void }) {
      const scrollerRef = useRef<HTMLDivElement>(null);
      const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
      const [isAtTail, setIsAtTail] = useState(true);
      const [shouldFollow, setShouldFollow] = useState(true);
      const isAtTailRef = useRef(true);
      const shouldFollowRef = useRef(true);

      useEffect(() => {
        const scroller = scrollerRef.current;
        if (!scroller || !sentinelEl) return;
        const detach = attachTailSentinel(scroller, sentinelEl, (atTail) => {
          isAtTailRef.current = atTail;
          setIsAtTail((prev) => (prev === atTail ? prev : atTail));
        });
        return detach;
      }, [sentinelEl]);

      useEffect(() => {
        if (isAtTail) {
          if (!shouldFollowRef.current) {
            shouldFollowRef.current = true;
            setShouldFollow(true);
          }
          return;
        }
        if (!shouldFollowRef.current) return;
        const timer = window.setTimeout(() => {
          if (!isAtTailRef.current) {
            shouldFollowRef.current = false;
            setShouldFollow(false);
          }
        }, 1500);
        return () => clearTimeout(timer);
      }, [isAtTail]);

      onState({ shouldFollow });
      return (
        <div ref={scrollerRef}>
          <div ref={setSentinelEl} />
        </div>
      );
    }

    let last: { shouldFollow: boolean } = { shouldFollow: true };
    render(<DebounceHarness onState={(s) => { last = s; }} />);
    expect(last.shouldFollow).toBe(true);

    // Sentinel leaves tail (Virtuoso lag).
    act(() => { MockIntersectionObserver.last().fire(false); });
    expect(last.shouldFollow).toBe(true);

    // 500ms elapses — still within debounce.
    act(() => { vi.advanceTimersByTime(500); });
    expect(last.shouldFollow).toBe(true);

    // Sentinel returns before 1500ms — debounce canceled.
    act(() => { MockIntersectionObserver.last().fire(true); });
    expect(last.shouldFollow).toBe(true);

    // Run full timer queue — confirm nothing pending.
    act(() => { vi.advanceTimersByTime(2000); });
    expect(last.shouldFollow).toBe(true);

    vi.useRealTimers();
  });

  it("shouldFollow flips false after sentinel absent for ≥1500ms", () => {
    // User-intent confirmation: sustained absence from tail flips
    // shouldFollow to false. This is what drives followOutput off
    // and stops the ResizeObserver pin.
    vi.useFakeTimers();
    function DebounceHarness({ onState }: { onState: (s: { shouldFollow: boolean }) => void }) {
      const scrollerRef = useRef<HTMLDivElement>(null);
      const [sentinelEl, setSentinelEl] = useState<HTMLDivElement | null>(null);
      const [isAtTail, setIsAtTail] = useState(true);
      const [shouldFollow, setShouldFollow] = useState(true);
      const isAtTailRef = useRef(true);
      const shouldFollowRef = useRef(true);

      useEffect(() => {
        const scroller = scrollerRef.current;
        if (!scroller || !sentinelEl) return;
        const detach = attachTailSentinel(scroller, sentinelEl, (atTail) => {
          isAtTailRef.current = atTail;
          setIsAtTail((prev) => (prev === atTail ? prev : atTail));
        });
        return detach;
      }, [sentinelEl]);

      useEffect(() => {
        if (isAtTail) {
          if (!shouldFollowRef.current) {
            shouldFollowRef.current = true;
            setShouldFollow(true);
          }
          return;
        }
        if (!shouldFollowRef.current) return;
        const timer = window.setTimeout(() => {
          if (!isAtTailRef.current) {
            shouldFollowRef.current = false;
            setShouldFollow(false);
          }
        }, 1500);
        return () => clearTimeout(timer);
      }, [isAtTail]);

      onState({ shouldFollow });
      return (
        <div ref={scrollerRef}>
          <div ref={setSentinelEl} />
        </div>
      );
    }

    let last: { shouldFollow: boolean } = { shouldFollow: true };
    render(<DebounceHarness onState={(s) => { last = s; }} />);

    act(() => { MockIntersectionObserver.last().fire(false); });
    act(() => { vi.advanceTimersByTime(1600); });
    expect(last.shouldFollow).toBe(false);

    // Re-entering tail restores shouldFollow.
    act(() => { MockIntersectionObserver.last().fire(true); });
    expect(last.shouldFollow).toBe(true);

    vi.useRealTimers();
  });
});
