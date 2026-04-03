---
name: cross-channel
description: Cross-channel context awareness — unified memory across Telegram, UI, Discord
triggers:
  - we discussed
  - in telegram
  - in the chat
  - remember when
  - мы обсуждали
  - в телеграме
  - в чате
  - помнишь когда
priority: 3
---

## Cross-Channel Context

Sessions are shared across channels. If the user wrote in Telegram and then continued in the UI — you see the full history.

### When to apply
- User references a previous conversation ("we discussed", "I mentioned")
- Context from another channel is needed
- `memory(action="search")` for long-term context, `session(action="history")` for recent

### Limitations
- Different channels have different formats (Telegram: MarkdownV2, UI: markdown)
- Files/images are bound to a channel — do not forward between channels
- Do not mention technical channel details to the user
