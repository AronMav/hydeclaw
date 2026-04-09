---
name: canvas-usage
description: Display rich visual content in the Canvas panel — HTML dashboards, charts, markdown, JSON tables, embedded URLs
triggers:
  - визуализ
  - график
  - диаграмм
  - dashboard
  - canvas
  - покажи визуально
  - нарисуй
  - отобрази
  - chart
  - graph
  - таблиц
  - выведи в canvas
  - покажи на canvas
  - интерактивн
  - дашборд
tools_required:
  - canvas
---

# Canvas — визуальное отображение контента

Canvas — выделенная панель в UI HydeClaw для показа визуального контента: HTML-страниц, графиков, дашбордов, markdown-документов, таблиц, встроенных URL.

## Когда использовать Canvas

- Визуализация данных (графики, диаграммы, charts)
- Dashboard или аналитика
- Таблица с форматированием или стилизацией
- Интерактивный контент (формы, фильтры, анимации)
- HTML-страница или виджет
- Встроенный URL (iframe)
- Пользователь говорит "покажи в canvas", "нарисуй", "визуализируй"

## Когда НЕ использовать Canvas

- Простой текст — пиши прямо в чат
- Короткие списки — markdown в чате достаточно
- Изображения из инструментов — показываются inline автоматически
- Табличные данные без стилизации — используй `rich_card` (card_type="table") для inline-таблиц в чате

## Действия (actions)

### present — показать контент

Основное действие. Открывает Canvas-панель и отображает контент.

```json
{
  "action": "present",
  "content_type": "html",
  "title": "Заголовок панели",
  "content": "<полный HTML с inline CSS и JS>"
}
```

### push_data — отправить JSON-данные

Показывает структурированные данные. Автоматически ставит `content_type: "json"`.

```json
{
  "action": "push_data",
  "content": "{\"columns\": [\"Name\", \"Value\"], \"rows\": [[\"CPU\", \"42%\"], [\"RAM\", \"8GB\"]]}",
  "title": "System Metrics"
}
```

JSON со структурой `{columns, rows}` отображается как таблица (TableCard).

### clear — очистить canvas

```json
{
  "action": "clear"
}
```

### run_js — выполнить JS в текущем canvas

Выполняет JavaScript-код в контексте текущего canvas-содержимого через browser-renderer. Требует предварительный `present` с контентом.

```json
{
  "action": "run_js",
  "code": "document.querySelector('#counter').textContent = '42'"
}
```

### snapshot — скриншот canvas

Делает PNG-скриншот текущего canvas через browser-renderer. Требует предварительный `present` с контентом.

```json
{
  "action": "snapshot"
}
```

## Типы контента (content_type)

| Тип        | Описание                             | Рендеринг                                              |
| ---------- | ------------------------------------ | ------------------------------------------------------ |
| `html`     | Полная HTML-страница c inline CSS/JS | Sandboxed iframe (allow-scripts)                       |
| `markdown` | Markdown-текст                       | Компонент `<Markdown>` с prose-стилями                 |
| `url`      | URL для встраивания                  | iframe с `sanitizeUrl()`                               |
| `json`     | JSON-строка                          | Форматированный JSON или TableCard (`{columns, rows}`) |

По умолчанию: `markdown`.

## Правила дизайна для HTML

При создании HTML-контента соблюдай строгие правила дизайна:

### Обязательно

- Полная самодостаточная HTML-страница с **inline** CSS и JS
- Тёмная тема: глубокие цвета (`#1a1a2e`, `#0a192f`, `#2d1b33`), не плоский чёрный
- SVG-иконки или CSS-формы вместо emoji
- Тёплые тона, teals, ambers или monochrome — никаких purple/indigo/violet градиентов
- Асимметричные layout'ы: разные размеры элементов, left-aligned текст
- Контрастные font-weight (200 vs 800), mix serif + sans-serif
- Глубина: layered shadows, subtle borders, glassmorphism
- Жизнь: CSS transitions on hover, staggered @keyframe fade-ins, subtle transforms

### Запрещено

- Emoji как иконки (🌤️☁️🌡️💧💨🚀📊✨ и т.д.)
- Purple/indigo/violet градиенты
- 3 одинаковые карточки в ряд — используй asymmetric grid
- Центрирование всего подряд — left-align text, varied whitespace

### Библиотеки через CDN

- **Chart.js**: `<script src="https://cdn.jsdelivr.net/npm/chart.js"></script>`
- **D3.js**: `<script src="https://cdn.jsdelivr.net/npm/d3"></script>`
- **Mermaid**: `<script src="https://cdn.jsdelivr.net/npm/mermaid/dist/mermaid.min.js"></script>`
- Любая CDN-библиотека для визуализации

## Ограничения

- **Максимальный размер контента**: 5 MB
- HTML рендерится в `<iframe sandbox="allow-scripts">` — нет доступа к parent window
- `run_js` и `snapshot` требуют запущенный browser-renderer
- URL-контент фильтруется через `sanitizeUrl()` для безопасности

## API

| Endpoint               | Метод  | Описание                        |
| ---------------------- | ------ | ------------------------------- |
| `/api/canvas/{agent}`  | GET    | Текущее состояние canvas агента |
| `/api/canvas/{agent}`  | DELETE | Очистить canvas агента          |

WebSocket-событие `canvas_update` — real-time обновление canvas в UI.

## Важно

**ОБЯЗАТЕЛЬНО** после вызова canvas напиши текстовое резюме в чат. Пользователь может не видеть Canvas-панель (мобильное устройство, канал без UI). Пустой ответ = провал.
