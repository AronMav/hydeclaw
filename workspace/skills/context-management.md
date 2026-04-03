---
name: context-management
description: Conversation context management — when to summarize, what to remember, how to stay on track
triggers:
  - forgot
  - what were we talking about
  - lost context
  - remind me
  - conversation too long
  - забыл
  - о чём мы говорили
  - потерял контекст
  - напомни
  - разговор слишком длинный
priority: 3
---

## Conversation Context Management

### The Problem

Long conversations lose focus. Important information from early in the conversation gets displaced by new content. The LLM starts "forgetting" earlier instructions.

### Context Preservation Strategy

#### 1. Anchor points
At the start of a complex task, establish:
- The goal of the conversation
- Key constraints
- Decisions already made

#### 2. Progressive summarization
When the conversation is long:
- Summarize intermediate results
- Save conclusions to memory (`memory(action="index")`)
- Reference the saved version instead of repeating

#### 3. What to save to long-term memory
- ✅ Conclusions and decisions (not the discussion process)
- ✅ User preferences
- ✅ Facts that are hard to find again
- ❌ Intermediate reasoning
- ❌ Raw search results
- ❌ File contents (they are already in workspace)

#### 4. Topic switching
When the user changes topics:
- Wrap up the current one (brief summary if needed)
- Don't carry context from the previous topic into the new one
- If topics are related — state the connection explicitly

### Signs of Context Loss

- Repeating what was already said
- Contradicting yourself
- User says "we discussed this" / "I already said"
- Confusing details from different topics

When detected → `memory(action="search")` for relevant context.

### Principles

- Better to ask than to guess
- A brief summary at the end of a complex discussion is free insurance
- If unsure about context — acknowledge it and clarify
