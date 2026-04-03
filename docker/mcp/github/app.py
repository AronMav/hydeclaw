"""GitHub — MCP server for GitHub operations via gh CLI and API."""

import os
import asyncio
import json
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse

GITHUB_TOKEN = os.environ.get("GITHUB_TOKEN", "")

app = FastAPI()

MCP_TOOLS = [
    {
        "name": "list_repos",
        "description": "List user's GitHub repositories.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "default": 20, "description": "Max repos to list"},
            },
        },
    },
    {
        "name": "list_issues",
        "description": "List open issues for a repository.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository in 'owner/repo' format"},
                "state": {"type": "string", "default": "open", "description": "Issue state: open, closed, all"},
                "limit": {"type": "integer", "default": 20},
            },
            "required": ["repo"],
        },
    },
    {
        "name": "list_prs",
        "description": "List pull requests for a repository.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository in 'owner/repo' format"},
                "state": {"type": "string", "default": "open", "description": "PR state: open, closed, merged, all"},
                "limit": {"type": "integer", "default": 20},
            },
            "required": ["repo"],
        },
    },
    {
        "name": "create_issue",
        "description": "Create a new GitHub issue.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository in 'owner/repo' format"},
                "title": {"type": "string", "description": "Issue title"},
                "body": {"type": "string", "description": "Issue body (markdown)"},
            },
            "required": ["repo", "title"],
        },
    },
    {
        "name": "get_issue",
        "description": "Get details of a specific issue or PR.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "Repository in 'owner/repo' format"},
                "number": {"type": "integer", "description": "Issue or PR number"},
            },
            "required": ["repo", "number"],
        },
    },
]


async def run_gh(args: list[str]) -> str:
    """Run gh CLI command and return stdout. Uses execFile-style (no shell)."""
    env = os.environ.copy()
    if GITHUB_TOKEN:
        env["GH_TOKEN"] = GITHUB_TOKEN
    proc = await asyncio.create_subprocess_exec(
        "gh", *args,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        env=env,
    )
    stdout, stderr = await proc.communicate()
    if proc.returncode != 0:
        err = stderr.decode().strip()
        raise RuntimeError(f"gh {' '.join(args)} failed: {err}")
    return stdout.decode().strip()


async def list_repos(limit: int = 20) -> str:
    out = await run_gh(["repo", "list", "--limit", str(limit), "--json", "name,description,updatedAt,isPrivate"])
    repos = json.loads(out)
    if not repos:
        return "No repositories found."
    lines = []
    for r in repos:
        vis = "private" if r.get("isPrivate") else "public"
        desc = r.get("description") or ""
        lines.append(f"  {r['name']} ({vis}) — {desc}")
    return "Repositories:\n" + "\n".join(lines)


async def list_issues(repo: str, state: str = "open", limit: int = 20) -> str:
    out = await run_gh(["issue", "list", "-R", repo, "--state", state, "--limit", str(limit),
                        "--json", "number,title,state,author,createdAt"])
    issues = json.loads(out)
    if not issues:
        return f"No issues ({state}) in {repo}."
    lines = [f"Issues ({state}) in {repo}:"]
    for i in issues:
        author = i.get("author", {}).get("login", "?")
        lines.append(f"  #{i['number']} {i['title']} (by {author})")
    return "\n".join(lines)


async def list_prs(repo: str, state: str = "open", limit: int = 20) -> str:
    out = await run_gh(["pr", "list", "-R", repo, "--state", state, "--limit", str(limit),
                        "--json", "number,title,state,author,createdAt"])
    prs = json.loads(out)
    if not prs:
        return f"No PRs ({state}) in {repo}."
    lines = [f"Pull Requests ({state}) in {repo}:"]
    for p in prs:
        author = p.get("author", {}).get("login", "?")
        lines.append(f"  #{p['number']} {p['title']} (by {author})")
    return "\n".join(lines)


async def create_issue(repo: str, title: str, body: str = "") -> str:
    args = ["issue", "create", "-R", repo, "--title", title]
    if body:
        args.extend(["--body", body])
    out = await run_gh(args)
    return f"Issue created: {out}"


async def get_issue(repo: str, number: int) -> str:
    out = await run_gh(["issue", "view", "-R", repo, str(number), "--json",
                        "number,title,state,body,author,createdAt,comments"])
    data = json.loads(out)
    author = data.get("author", {}).get("login", "?")
    body = data.get("body", "")[:2000]
    comments = data.get("comments", [])
    lines = [
        f"#{data['number']} {data['title']} [{data['state']}]",
        f"Author: {author}",
        f"\n{body}" if body else "",
    ]
    if comments:
        lines.append(f"\nComments: {len(comments)}")
        for c in comments[:5]:
            c_author = c.get("author", {}).get("login", "?")
            c_body = c.get("body", "")[:500]
            lines.append(f"  {c_author}: {c_body}")
    return "\n".join(lines)


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.post("/mcp")
async def mcp_endpoint(request: Request):
    body = await request.json()
    method = body.get("method", "")
    req_id = body.get("id", 1)
    params = body.get("params", {})

    if method == "tools/list":
        return JSONResponse({"jsonrpc": "2.0", "result": {"tools": MCP_TOOLS}, "id": req_id})

    if method == "tools/call":
        tool_name = params.get("name", "")
        args = params.get("arguments", {})

        try:
            if tool_name == "list_repos":
                result = await list_repos(args.get("limit", 20))
            elif tool_name == "list_issues":
                result = await list_issues(args["repo"], args.get("state", "open"), args.get("limit", 20))
            elif tool_name == "list_prs":
                result = await list_prs(args["repo"], args.get("state", "open"), args.get("limit", 20))
            elif tool_name == "create_issue":
                result = await create_issue(args["repo"], args["title"], args.get("body", ""))
            elif tool_name == "get_issue":
                result = await get_issue(args["repo"], args["number"])
            else:
                return JSONResponse({
                    "jsonrpc": "2.0",
                    "error": {"code": -32601, "message": f"Unknown tool: {tool_name}"},
                    "id": req_id,
                })

            return JSONResponse({
                "jsonrpc": "2.0",
                "result": {"content": [{"type": "text", "text": result}]},
                "id": req_id,
            })
        except Exception as e:
            return JSONResponse({
                "jsonrpc": "2.0",
                "error": {"code": -32000, "message": str(e)},
                "id": req_id,
            })

    return JSONResponse({
        "jsonrpc": "2.0",
        "error": {"code": -32601, "message": f"Unknown method: {method}"},
        "id": req_id,
    })
