import { fireEvent, render, screen } from "@testing-library/react"
import { beforeAll, describe, expect, it } from "vitest"
import {
  CitationRef,
  CitationSection,
  FootnoteProvider,
  extractFootnotes,
} from "@/components/ui/citation-tooltip"

// Radix Tooltip uses ResizeObserver internally
beforeAll(() => {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as any
})

describe("extractFootnotes", () => {
  it("extracts footnote definitions from markdown", () => {
    const md = `Some text with [^1] and [^2].

[^1]: First footnote definition
[^2]: Second footnote definition`
    const map = extractFootnotes(md)
    expect(map.get("1")).toBe("First footnote definition")
    expect(map.get("2")).toBe("Second footnote definition")
  })

  it("returns empty map for markdown without footnotes", () => {
    const map = extractFootnotes("Just plain markdown text.")
    expect(map.size).toBe(0)
  })

  it("handles multi-word footnote ids", () => {
    const md = `[^my-note]: A longer id definition`
    const map = extractFootnotes(md)
    expect(map.get("my-note")).toBe("A longer id definition")
  })
})

describe("CitationRef", () => {
  it("renders a superscript element with the footnote number", () => {
    render(
      <FootnoteProvider footnotes={new Map([["1", "Source text"]])}>
        <CitationRef href="#user-content-fn-1" data-footnote-ref>
          1
        </CitationRef>
      </FootnoteProvider>
    )
    const sup = screen.getByText("1").closest("sup")
    expect(sup).toBeTruthy()
  })

  it("renders with tooltip trigger when footnote text is available", () => {
    const { container } = render(
      <FootnoteProvider footnotes={new Map([["1", "Cited source text"]])}>
        <CitationRef href="#user-content-fn-1" data-footnote-ref>
          1
        </CitationRef>
      </FootnoteProvider>
    )
    // When footnote text exists, the component wraps in tooltip with a trigger
    const tooltipTrigger = container.querySelector("[data-slot='tooltip-trigger']")
    expect(tooltipTrigger).toBeTruthy()
    // The trigger (superscript) is rendered inside the tooltip structure
    const sup = screen.getByText("1").closest("sup")
    expect(sup).toBeTruthy()
  })

  it("renders superscript without tooltip when no footnote text available", () => {
    render(
      <FootnoteProvider footnotes={new Map()}>
        <CitationRef href="#user-content-fn-unknown" data-footnote-ref>
          99
        </CitationRef>
      </FootnoteProvider>
    )
    const sup = screen.getByText("99").closest("sup")
    expect(sup).toBeTruthy()
  })
})

describe("CitationSection", () => {
  it("renders with sr-only class (visually hidden)", () => {
    render(
      <CitationSection data-footnotes>
        <ol>
          <li>Footnote content</li>
        </ol>
      </CitationSection>
    )
    const section = screen.getByLabelText("Footnotes")
    expect(section).toBeTruthy()
    expect(section.className).toContain("sr-only")
  })
})
