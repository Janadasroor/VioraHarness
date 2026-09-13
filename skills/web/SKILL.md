---
name: web
description: Web development loop (serve, screenshot, verify)
triggers: [web, website, html, css, react, vite, frontend, localhost, dev server, screenshot]
---
# Web Skill
Loop: `dev_serve` the folder (port 0 picks a free one) → `browser_screenshot` the URL for vision → `browser_dom` to assert text → edit → re-screenshot. After every web edit, re-screenshot and describe what you see.

- Local targets with default `out` auto-save `./screenshot-latest.<mode>.png`; pass `out:` to override.
- Headless Chrome needs no X server. Visible Chrome needs `browser_open` (Ask-gated, runs outside the sandbox); never `xdotool windowclose`.
- Kill dev servers via `/tasks` when done. Docs via `webfetch`/`websearch`.
