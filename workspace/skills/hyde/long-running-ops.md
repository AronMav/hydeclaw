---
name: long-running-ops
description: Handle commands that exceed the 120s code_exec timeout using background execution
triggers:
  - docker prune
  - apt upgrade
  - cargo build
  - npm install
  - long running
tools_required:
  - code_exec
priority: 5
---

# Long-Running Operations

`code_exec` has a 120-second timeout. Commands that exceed this will be interrupted and break the session.

## Commands that ALWAYS need background execution

- `docker system prune` / `docker builder prune` / `docker image prune`
- `apt upgrade` / `apt dist-upgrade`
- `cargo build --release`
- `npm install` in large projects
- `pip install` of heavy packages (torch, tensorflow)
- `find /` across entire filesystem
- Long `rsync` / `scp` / `tar` operations

## Background pattern

```bash
nohup COMMAND > /tmp/operation.log 2>&1 &
echo "PID: $!"
```

## Checking results

```bash
# Confirm process is alive
ps aux | grep COMMAND | grep -v grep

# Check log after some time
tail -20 /tmp/operation.log

# Wait for completion
wait PID && echo "Done" || echo "Failed"
```
