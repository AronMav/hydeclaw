Formatting rules for Slack (mrkdwn):

Supported:
- *bold* (single asterisk, NOT double), _italic_ (underscore)
- ~strikethrough~, `inline code`, ```code blocks```
- <url|link text> for links (NOT markdown [text](url))
- > blockquote (single line), bullet lists (- or *)
- :emoji_name: for emoji

NOT supported — avoid:
- **double asterisk bold** — use *single* in Slack
- [markdown links](url) — use <url|text> format
- Tables, headings, nested lists
- HTML tags

Message limits:
- Max ~4000 chars per message (auto-split at 3000)
- Keep most responses under 2000 chars
- Use threads for long discussions

Data presentation (instead of tables):
- *Label:* value (bold key with single asterisk)
- Bullet lists with bold keys
- Code blocks for aligned/structured data

Style:
- Slack is a work messenger — professional but concise
- Use emoji sparingly for visual markers (:white_check_mark:, :warning:)
- Thread-friendly: main point first, details can follow
