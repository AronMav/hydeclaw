---
name: mcp-docker-pattern
description: Use when deploying new MCP servers to HydeClaw via Docker containers and mcp-deploy.sh script
triggers:
  - mcp сервер
  - подключи mcp
  - deploy mcp
  - mcp server
tools_required:
  - code_exec
priority: 10
---

# MCP Server Deployment

Как подключать новые MCP серверы к HydeClaw.

## КРИТИЧЕСКИЕ ПРАВИЛА

1. **`process(action="start")`** — для ВСЕХ docker-команд и скриптов. Работает НА ХОСТЕ.
2. **НЕ используй `code_exec`** для docker-операций — он работает в sandbox БЕЗ Docker.
3. **ВСЕГДА используй скрипт `mcp-deploy.sh`** — НЕ пиши Dockerfile вручную. Скрипт сам определяет тип (Python/Node.js), собирает на базе bridge image, верифицирует.
4. **Деплой ВСЕГДА**, даже если сервер требует API-токен. Контейнер создаётся, токен добавляется позже через `secret_set`. Не отказывайся от деплоя из-за отсутствия токена.
5. После `process(action="start")` вызови `process(action="status", process_id=...)` и дождись завершения. Если скрипт вернул FAIL — проверь логи через `process(action="start", command="docker logs mcp-NAME")`.

## Автоматический деплой через скрипт

На Pi есть скрипт `~/hydeclaw/scripts/mcp-deploy.sh`, который делает ВСЁ автоматически:
build, container create, workspace YAML, verify.

### Тип 1: Node.js stdio MCP (официальные mcp/* образы)

```
process(action="start", command="~/hydeclaw/scripts/mcp-deploy.sh stdio-node mcp/fetch:latest fetch 9011")
```

Скрипт автоматически:
- Пуллит образ (с --platform linux/amd64 для ARM совместимости)
- Определяет entrypoint из образа
- Создаёт 3-строчный Dockerfile на базе hydeclaw-mcp-bridge
- Билдит, создаёт контейнер, YAML, верифицирует

### Тип 2: Python stdio MCP (pip-пакет)

```
process(action="start", command="~/hydeclaw/scripts/mcp-deploy.sh stdio-python mcp-server-git git 9012 mcp-server-git")
```

Формат: `stdio-python <pip-package> <name> <port> [command-name]`

### Тип 3: Внешний HTTP MCP (без Docker)

```
process(action="start", command="~/hydeclaw/scripts/mcp-deploy.sh url https://context7.com/mcp context7")
```

Только создаёт workspace/mcp/name.yaml с url.

### Удаление

```
process(action="start", command="~/hydeclaw/scripts/mcp-deploy.sh remove fetch")
```

## Проверка результата

После `process(action="start")` вызови `process(action="status", process_id=...)` чтобы проверить результат.
Скрипт выводит OK/FAIL и количество tools.

## Занятые порты

| Порт | Сервис |
|------|--------|
| 9002 | summarize |
| 9003 | stock-analysis |
| 9004 | weather |
| 9005 | obsidian |
| 9006 | github |
| 9007 | postgres |
| 9011 | toolgate (managed process, НЕ MCP) |
| 9020 | browser-renderer |
| 9030 | browser-cdp |

Для новых MCP используй порты начиная с 9040, 9041, 9042...
Пропускай уже занятые.

## Известные MCP серверы

### Node.js stdio (mcp/* на Docker Hub):
- `mcp/fetch:latest` — HTTP fetch, загрузка веб-страниц
- `mcp/memory:latest` — key-value memory store
- `mcp/sequentialthinking:latest` — structured reasoning
- `mcp/filesystem:latest` — файловая система (нужен volume mount)
- `mcp/puppeteer:latest` — Chromium browser automation
- `mcp/everart:latest` — image generation
- `mcp/time:latest` — timezone/datetime operations

### Python pip:
- `mcp-server-git` — git operations (command: `mcp-server-git`)

### External HTTP (без Docker):
- `https://context7.com/mcp` — документация библиотек
- `https://mcp.deepwiki.com/mcp` — wiki/knowledge base

## MCP серверы с env переменными (токены)

Для серверов требующих API-токены, передай env vars 5-м аргументом:
```
process(action="start", command="~/hydeclaw/scripts/mcp-deploy.sh stdio-node mcp/slack:latest slack 9047 'SLACK_BOT_TOKEN: ${SLACK_BOT_TOKEN}'")
```

Токен добавляется потом через `secret_set`. Сервер задеплоится, но вернёт ошибку при вызове без токена — это нормально.

## Troubleshooting

Если скрипт вернул FAIL:
1. Проверь логи контейнера: `process(action="start", command="docker logs mcp-NAME")`
2. Частая причина: образ не существует для ARM64 — скрипт пробует --platform linux/amd64
3. Для Python MCP: проверь что pip-пакет существует
4. Для external URL: проверь доступность через `process(action="start", command="curl -X POST URL ...")`
