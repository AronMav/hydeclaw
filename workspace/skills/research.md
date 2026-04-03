---
name: research
description: Deep research on a topic using multiple sources, cross-referencing, and synthesis
triggers:
  - research
  - analyze in depth
  - deep research
  - find and analyze
  - исследование
  - глубокий анализ
  - глубокое исследование
  - найди и проанализируй
---

# Research Skill

## Strategy

1. **Initial search**: Use search_web for broad overview (3-5 queries with different angles)
2. **Source evaluation**: Pick top 3-5 most relevant results
3. **Deep read**: Use web_fetch to read full articles from top sources
4. **Cross-reference**: Compare information across sources, note contradictions
5. **Memory check**: memory(action="search") for any prior knowledge on the topic
6. **Synthesis**: Combine findings into a structured analysis

## Output Format

- Executive summary (2-3 sentences)
- Key findings (bulleted list)
- Sources (numbered, with URLs)
- Confidence assessment (high/medium/low per finding)
- Open questions (what remains unclear)

## Rules

- Never present single-source information as fact
- Always note when sources disagree
- Prefer recent sources over old ones
- Use search_web_fresh for time-sensitive topics
- Save key findings to memory(action="index") for future reference
