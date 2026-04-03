---
name: code-review
description: Review code for bugs, security issues, and best practices
triggers:
  - review code
  - check the code
  - code review
  - find bugs
  - проверь код
  - ревью кода
  - найди баги
  - посмотри код
---

# Code Review Skill

## Process

1. Read the code carefully (workspace_read or from message)
2. Check for:
   - **Security**: SQL injection, XSS, command injection, hardcoded secrets
   - **Bugs**: null/undefined handling, off-by-one, race conditions
   - **Performance**: N+1 queries, unnecessary allocations, missing indexes
   - **Style**: naming conventions, dead code, code duplication
3. Rate severity: CRITICAL / HIGH / MEDIUM / LOW
4. Provide specific fix suggestions with code examples

## Output Format

### Summary
One sentence: overall quality assessment

### Issues Found
For each issue:
- **[SEVERITY]** Brief description
- Location: file:line
- Fix: specific code change

### What's Good
2-3 positive observations (reinforcement)
