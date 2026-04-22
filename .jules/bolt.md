## 2024-04-22 - Expensive JSON Serialization in Render Loop
**Learning:** Frequent React renders in components processing large `args` or `result` payloads (like `ToolCallPartView` which handles chat stream chunks) can severely block the main thread if stringification like `JSON.stringify(data, null, 2)` happens on every render.
**Action:** Always wrap heavy synchronous operations, especially `JSON.stringify` on unbounded data, in `useMemo` when calculating derived values inside frequently re-rendering components.
