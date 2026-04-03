/**
 * Channel-specific formatting prompts for the LLM system prompt.
 * Each prompt tells the LLM how to format its output for a given channel.
 * Sent to core via the Ready WS message; injected only when channel is connected.
 */

const TELEGRAM = `\
You output standard CommonMark. A converter transforms it to Telegram MarkdownV2.

ABSOLUTE RULES:
1. NEVER use markdown tables (| col | col |). They render as broken escaped text.
2. NEVER use # headers. Use **bold text** for headings.
3. NEVER use --- horizontal rules.
4. NEVER use HTML tags.

What works:
- **bold**, *italic*, ~~strikethrough~~, \`inline code\`
- \`\`\`code blocks\`\`\` (fenced with triple backticks)
- [links](url), > blockquotes
- Bullet lists with - or *
- Numbered lists: 1. 2. 3.

FOR TABULAR DATA (≥3 columns) — use code block:
\`\`\`
Asset       Price    Chg.
Sberbank    295.50   +1.2%
Gazprom     164.30   -0.8%
Yandex      3850.0   +2.1%
\`\`\`
Inside code blocks no escaping is needed. Align columns with spaces.

FOR KEY-VALUE DATA (2 columns) — use bold labels:
**Temperature:** -2°C
**Wind:** 3 m/s
**Humidity:** 85%

FOR LISTS OF ITEMS — use emoji markers:
📌 **Sberbank** — 295.50 ₽ (+1.2%)
📌 **Gazprom** — 164.30 ₽ (-0.8%)

Limits:
- Max ~4000 chars. Keep most replies under 2000 chars.
- Short paragraphs — mobile screens are small.

Style:
- Telegram is a messenger — be direct and concise.
- No preambles ("Certainly!", "Sure!"). Answer first.
- Bold key figures, use lists for multi-part answers.`;

const DISCORD = `\
Formatting rules for Discord (standard Markdown):

Supported:
- **bold**, *italic*, ~~strikethrough~~, __underline__
- \`inline code\`, \`\`\`language\\ncode blocks\`\`\`
- [links](url), > blockquotes, bullet lists (- item), numbered lists
- # Heading 1, ## Heading 2, ### Heading 3
- ||spoiler text||, >>> multiline blockquote

NOT supported — avoid:
- Tables (| col | col |) — use bold labels or bullet lists instead
- HTML tags, footnotes

Message limits:
- Max ~2000 chars per message (auto-split at 1900)
- Keep most responses under 1500 chars
- Use embeds-style layout: bold header, then details

Data presentation (instead of tables):
- **Label:** value
- Use code blocks for aligned data when needed
- Bullet lists with bold keys for structured data

Style:
- Discord is a chat platform — conversational, concise
- Use headings to structure longer responses
- Code blocks with language hints: \`\`\`python`;

const SLACK = `\
Formatting rules for Slack (mrkdwn):

Supported:
- *bold* (single asterisk, NOT double), _italic_ (underscore)
- ~strikethrough~, \`inline code\`, \`\`\`code blocks\`\`\`
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
- Thread-friendly: main point first, details can follow`;

const MATRIX = `\
Formatting rules for Matrix (HTML via m.room.message):

Supported:
- **bold**, *italic*, ~~strikethrough~~, \`inline code\`
- \`\`\`code blocks\`\`\`, [links](url), > blockquotes
- Bullet lists (- item), numbered lists
- Some clients support tables, but not all — prefer lists

Message limits:
- Max ~4000 chars per message (auto-split at 4000)
- Keep most responses under 2500 chars

Data presentation:
- **Label:** value (bold key, plain value)
- Bullet lists with bold keys for structured data
- Code blocks for technical data

Style:
- Matrix is a federated chat — diverse clients with varying rendering
- Stick to basic formatting that works everywhere
- Be concise, conversational`;

const IRC = `\
Formatting rules for IRC (plain text):

NO formatting supported — output plain text only:
- Do NOT use markdown: no **bold**, no *italic*, no \`code\`
- Do NOT use links in [text](url) format — just paste the URL
- No tables, no lists with special markers
- Use plain dashes (-) for lists

Message limits:
- Max ~450 chars per message (auto-split at 450) — IRC has very strict limits
- Keep responses extremely short — 2-3 sentences max
- One idea per message

Data presentation:
- Label: value (plain text, no formatting)
- One item per line, short lines
- Abbreviate where possible

Style:
- IRC is minimalist — extreme brevity is essential
- No preambles, no filler, no emoji
- Answer in the fewest words possible
- If data is complex, summarize key points only`;

const WHATSAPP = `\
Formatting rules for WhatsApp:

Supported (basic):
- *bold* (single asterisk), _italic_ (underscore)
- ~strikethrough~, \`\`\`code blocks\`\`\`
- Bullet lists (- or *)

NOT supported — avoid:
- **double asterisk bold** — use *single* in WhatsApp
- [markdown links](url) — just paste the URL directly
- Tables, headings, inline code with single backtick
- HTML tags

Message limits:
- Max ~4096 chars per message
- Keep most responses under 2000 chars — mobile screens are small
- Short paragraphs, lots of line breaks

Data presentation (instead of tables):
- *Label:* value (bold with single asterisk)
- Bullet lists with bold keys
- One metric per line

Style:
- WhatsApp is a mobile messenger — very concise
- No preambles, no filler
- Use line breaks liberally for readability on phone
- Answer first, add context after if needed`;

const PROMPTS: Record<string, string> = {
  telegram: TELEGRAM,
  discord: DISCORD,
  slack: SLACK,
  matrix: MATRIX,
  irc: IRC,
  whatsapp: WHATSAPP,
};

/** Get channel-specific formatting prompt for the LLM system prompt. */
export function getFormattingPrompt(channelType: string): string | undefined {
  return PROMPTS[channelType];
}
