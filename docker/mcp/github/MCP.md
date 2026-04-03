# GitHub MCP

Operations with GitHub repositories via gh CLI.

## Tools

### list_repos
List user's repositories.

### list_issues
- **repo** (required): Repository (owner/repo)
- **state** (optional): open/closed/all

### list_prs
- **repo** (required): Repository (owner/repo)
- **state** (optional): open/closed/merged/all

### create_issue
- **repo** (required): Repository
- **title** (required): Issue title
- **body** (optional): Description (markdown)

### get_issue
- **repo** (required): Repository
- **number** (required): Issue or PR number
