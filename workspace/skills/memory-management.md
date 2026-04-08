---
name: memory-management
description: Best practices for using memory with categories and topics
triggers:
  - запомни
  - remember
  - сохрани в память
  - save to memory
  - memory write
---

# Управление памятью

## Категоризация воспоминаний

При сохранении информации в память ВСЕГДА указывай category и topic.

### Категории (category)
- **decision** — принятые решения, выбор между альтернативами
- **preference** — предпочтения пользователя, вкусы, стиль
- **event** — произошедшие события, встречи, инциденты
- **discovery** — найденные факты, инсайты, новые знания
- **advice** — рекомендации, советы, best practices
- **general** — всё остальное

### Тема (topic)
Свободный текст, описывающий область: название проекта, технология, имя человека и т.д.

## Примеры

Пользователь: "Запомни что я предпочитаю Python для скриптов"
→ memory(action="index", content="Пользователь предпочитает Python для скриптов", category="preference", topic="programming", pinned=true)

Пользователь: "Мы решили использовать PostgreSQL"
→ memory(action="index", content="Решение: использовать PostgreSQL в проекте", category="decision", topic="database", pinned=true)

## Закрепление (pinned)

Устанавливай pinned=true для:
- Ключевых фактов о пользователе
- Важных решений по проектам
- Устойчивых предпочтений

pinned=false (по умолчанию) для:
- Временной информации
- Деталей разговоров
- Контекстных заметок

## Поиск с фильтрацией

При поиске используй category и topic для точности:
→ memory(action="search", query="база данных", category="decision") — только решения
→ memory(action="search", query="Python", topic="programming") — только по теме
