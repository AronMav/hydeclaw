---
name: orchestrator
description: Task orchestration with structured JSON plans — decompose multi-agent tasks, write plans to workspace, track step progress
triggers:
  - orchestrate
  - coordinate agents
  - run in parallel
  - spawn agents
  - task plan
  - multi-step task
  - orchestrate
  - orchestrator
  - координируй агентов
  - параллельная задача
  - план задачи
priority: 9
tools_required:
  - workspace_write
  - workspace_read
  - agent
---

## When to Create a Task Plan

Create a task plan file when:
- Task requires **2+ agents** or **sequential steps with dependencies**
- Progress must be visible to the user or other agents
- Task runs long and you want to checkpoint partial progress

Single-agent tasks do not need a plan file — just spawn and monitor.

---

## JSON Plan Format

Plan files live at `tasks/task_YYYYMMDD_XXXXXX.json` in the workspace.

```json
{
  "task_id": "task_20260408_a1b2c3",
  "agent": "AgentName",
  "title": "Brief description of the overall task",
  "created_at": "2026-04-08T10:00:00Z",
  "updated_at": "2026-04-08T10:02:30Z",
  "status": "planning",
  "steps": [
    {
      "id": "step_1",
      "title": "Research competitor pricing",
      "status": "pending",
      "started_at": null,
      "finished_at": null,
      "error": null
    },
    {
      "id": "step_2",
      "title": "Write pricing comparison report",
      "status": "pending",
      "started_at": null,
      "finished_at": null,
      "error": null
    }
  ]
}
```

### Field Definitions

| Field | Type | Values |
|-------|------|--------|
| `task_id` | string | `task_YYYYMMDD_6alphanum` — use date + 6 random chars |
| `agent` | string | Your own agent name (from `IDENTITY.md` or `agents_list`) |
| `title` | string | Human-readable task summary |
| `created_at` | ISO 8601 | Set once at creation, never change |
| `updated_at` | ISO 8601 | Update on every rewrite |
| `status` | enum | `planning` → `in_progress` → `done` \| `error` |
| `steps[].id` | string | `step_1`, `step_2`, ... (sequential integers) |
| `steps[].status` | enum | `pending` → `in_progress` → `done` \| `error` |
| `steps[].started_at` | ISO 8601 \| null | Set when step begins |
| `steps[].finished_at` | ISO 8601 \| null | Set when step completes or fails |
| `steps[].error` | string \| null | Error message if status is `error` |

---

## Write Protocol (Before Spawning Subagents)

**Before** spawning any agent, write the full plan file using `workspace_write`:

```
workspace_write(
  path="tasks/task_20260408_a1b2c3.json",
  content='{ "task_id": "task_20260408_a1b2c3", "agent": "Hyde", "title": "Analyse Q1 data", "created_at": "2026-04-08T10:00:00Z", "updated_at": "2026-04-08T10:00:00Z", "status": "planning", "steps": [...] }'
)
```

Start with `"status": "planning"`. Change to `"in_progress"` immediately before launching the first agent.

---

## Update Protocol (After Each Subagent Completes)

After each agent completes, **rewrite the ENTIRE file** via `workspace_write` (NOT `workspace_edit` — avoids whitespace sensitivity).

**Steps:**
1. Read current plan: `workspace_read(path="tasks/task_20260408_a1b2c3.json")`
2. Update the step's `status`, `started_at`, `finished_at`, `error`
3. Update top-level `status` and `updated_at`
4. Write the whole file back: `workspace_write(path="tasks/task_20260408_a1b2c3.json", content=<full JSON>)`

**Status transitions:**
- Step starts → set `status: "in_progress"`, `started_at: "<now>"`
- Step succeeds → set `status: "done"`, `finished_at: "<now>"`
- Step fails → set `status: "error"`, `finished_at: "<now>"`, `error: "<message>"`
- All steps done → top-level `status: "done"`
- Any step errored and cannot retry → top-level `status: "error"`

---

## Parallel Steps

When spawning parallel agents, set ALL their steps to `"in_progress"` in the plan **before** launching:

```
// 1. Update plan: steps 1 and 2 both → in_progress
workspace_write(path="tasks/...", content=<plan with step_1 and step_2 as in_progress>)

// 2. Spawn both agents
id1 = agent(action="run", target="Alma", task="Task A...")
id2 = agent(action="run", target="Hyde", task="Task B...")

// 3. Monitor loop until both complete
// 4. As each completes, rewrite plan with updated status
```

---

## Example Workflow (3 Steps)

```
// Step 1: Create the plan
workspace_write(
  path="tasks/task_20260408_x9y8z7.json",
  content='{"task_id":"task_20260408_x9y8z7","agent":"Hyde","title":"Q1 analysis","created_at":"2026-04-08T09:00:00Z","updated_at":"2026-04-08T09:00:00Z","status":"planning","steps":[{"id":"step_1","title":"Fetch Q1 data","status":"pending","started_at":null,"finished_at":null,"error":null},{"id":"step_2","title":"Analyse trends","status":"pending","started_at":null,"finished_at":null,"error":null},{"id":"step_3","title":"Write report","status":"pending","started_at":null,"finished_at":null,"error":null}]}'
)

// Step 2: Mark step_1 in_progress, update plan status → in_progress
workspace_write(path="tasks/task_20260408_x9y8z7.json", content=<step_1 in_progress, status in_progress>)

// Step 3: Spawn agent for step_1
agent(action="run", target="Alma", task="Fetch Q1 revenue data from reports/q1.csv and return totals by region")

// Step 4: Monitor
agent(action="status", target="Alma")  // repeat until idle
// Read last_result from status response

// Step 5: Mark step_1 done, step_2 in_progress, write plan
workspace_write(path="tasks/task_20260408_x9y8z7.json", content=<step_1 done, step_2 in_progress>)

// ... repeat for step_2, step_3 ...

// Final: Mark all done, status → done
workspace_write(path="tasks/task_20260408_x9y8z7.json", content=<all done, status done>)
```

---

## Anti-Patterns

- DO NOT use `workspace_edit` to update plan files — always full rewrite via `workspace_write`
- DO NOT forget to update `updated_at` on every rewrite
- DO NOT set top-level `status: "done"` until ALL steps are done
- DO NOT create a plan for single-agent tasks — overhead not worth it
