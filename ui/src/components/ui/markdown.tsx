"use client"

import { cn } from "@/lib/utils"
import { marked } from "marked"
import { memo, useEffect, useId, useMemo, useState } from "react"
import ReactMarkdown, { Components } from "react-markdown"
import remarkBreaks from "remark-breaks"
import remarkGfm from "remark-gfm"
import { CodeBlock, CodeBlockCode } from "./code-block"
import { MermaidBlock } from "./mermaid-block"

// ── Math Detection ─────────────────────────────────────────────────────────

// Detect math content: $...$, $$...$$, \(...\), \[...\]
const MATH_PATTERN = /\$\$[\s\S]+?\$\$|\$[^\s$].*?[^\s$]\$|\\[([]\s*[\s\S]*?\s*\\[\])]/

function hasMathContent(content: string): boolean {
  return MATH_PATTERN.test(content)
}

// ── Types & Helpers ────────────────────────────────────────────────────────

export type MarkdownProps = {
  children: string
  id?: string
  className?: string
  components?: Partial<Components>
}

function parseMarkdownIntoBlocks(markdown: string): string[] {
  const tokens = marked.lexer(markdown)
  return tokens.map((token) => token.raw)
}

function extractLanguage(className?: string): string {
  if (!className) return "plaintext"
  const match = className.match(/language-(\w+)/)
  return match ? match[1] : "plaintext"
}

const INITIAL_COMPONENTS: Partial<Components> = {
  code: function CodeComponent({ className, children, ...props }) {
    const isInline =
      !props.node?.position?.start.line ||
      props.node?.position?.start.line === props.node?.position?.end.line

    if (isInline) {
      return (
        <span
          className={cn(
            "bg-muted rounded-sm px-1 font-mono text-sm",
            className
          )}
          {...props}
        >
          {children}
        </span>
      )
    }

    const language = extractLanguage(className)

    if (language === "mermaid") {
      return <MermaidBlock code={String(children).trim()} />
    }

    const codeStr = children as string
    const lineCount = codeStr ? codeStr.split('\n').length : 0

    return (
      <CodeBlock className={className} language={language}>
        <CodeBlockCode code={codeStr} language={language} showLineNumbers={lineCount > 10} />
      </CodeBlock>
    )
  },
  pre: function PreComponent({ children }) {
    return <>{children}</>
  },
}

// ── Standard Markdown Block (no math) ──────────────────────────────────────

const MemoizedMarkdownBlock = memo(
  function MarkdownBlock({
    content,
    components = INITIAL_COMPONENTS,
  }: {
    content: string
    components?: Partial<Components>
  }) {
    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        components={components}
      >
        {content}
      </ReactMarkdown>
    )
  },
  function propsAreEqual(prevProps, nextProps) {
    return prevProps.content === nextProps.content
  }
)

MemoizedMarkdownBlock.displayName = "MemoizedMarkdownBlock"

// ── Math-aware Markdown Block (KaTeX loaded on demand) ─────────────────────

const MemoizedMathBlock = memo(
  function MathBlock({
    content,
    components = INITIAL_COMPONENTS,
  }: {
    content: string
    components?: Partial<Components>
  }) {
    const [mathPlugins, setMathPlugins] = useState<{ remarkMath: any; rehypeKatex: any } | null>(null)

    useEffect(() => {
      let cancelled = false
      ;(async () => {
        const [rm, rk] = await Promise.all([
          import("remark-math"),
          import("rehype-katex"),
        ])
        // Load KaTeX CSS on demand (webpack handles CSS dynamic imports at build time)
        // @ts-expect-error -- CSS import has no type declarations but webpack bundles it correctly
        await import("katex/dist/katex.min.css")
        if (!cancelled) setMathPlugins({ remarkMath: rm.default, rehypeKatex: rk.default })
      })()
      return () => { cancelled = true }
    }, [])

    if (!mathPlugins) {
      // Render without math until plugins load (plain text fallback)
      return (
        <ReactMarkdown
          remarkPlugins={[remarkGfm, remarkBreaks]}
          components={components}
        >
          {content}
        </ReactMarkdown>
      )
    }

    return (
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks, mathPlugins.remarkMath]}
        rehypePlugins={[mathPlugins.rehypeKatex]}
        components={components}
      >
        {content}
      </ReactMarkdown>
    )
  },
  function propsAreEqual(prevProps, nextProps) {
    return prevProps.content === nextProps.content
  }
)

MemoizedMathBlock.displayName = "MemoizedMathBlock"

// ── Main Markdown Component ────────────────────────────────────────────────

function MarkdownComponent({
  children,
  id,
  className,
  components = INITIAL_COMPONENTS,
}: MarkdownProps) {
  const generatedId = useId()
  const blockId = id ?? generatedId
  const blocks = useMemo(() => parseMarkdownIntoBlocks(children), [children])

  return (
    <div className={className}>
      {blocks.map((block, index) =>
        hasMathContent(block) ? (
          <MemoizedMathBlock
            key={`${blockId}-block-${index}`}
            content={block}
            components={components}
          />
        ) : (
          <MemoizedMarkdownBlock
            key={`${blockId}-block-${index}`}
            content={block}
            components={components}
          />
        )
      )}
    </div>
  )
}

const Markdown = memo(MarkdownComponent)
Markdown.displayName = "Markdown"

export { Markdown }
