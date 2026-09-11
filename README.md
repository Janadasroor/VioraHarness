# VioraHarness
Custom coding agent harness for **VioraEDA** — Rust + Ratatui TUI + OpenRouter/Gemini/gateway providers, full-control harness pattern (long-lived server + streaming clients).

See `roadmap.md` for architecture SSOT, `RESEARCH.md` for deep harness survey, `ARCHITECTURE.md` for diagram, `AGENTS.md` for agent working memory.

## Quick Start
```bash
export OPENROUTER_API_KEY=sk-or-...  # or GEMINI_API_KEY; gateway free tier needs no key
cargo run -- doctor                       # check viora + keys + config + db
cargo run -- run "hello" --model provider/model-id
cargo run -- run "next step" -c -y        # continue latest chat here, auto-allow
cargo run -- tui                          # interactive Ratatui (Enter send, /help)
cargo run -- sessions                     # list persisted sessions
cargo run -- undo                         # restore last file snapshot
cargo run -- serve --port 4096            # HTTP SSE (loopback; bearer w/ VIORAHARNESS_API_TOKEN)
cargo run -- shim --port 11435           # free-tier gateway shim for URL+key-only clients (-> /v1)
```

## Tools (typed JSON Schema, validated before exec)
- `read`/`write`/`glob`/`grep`/`bash` (ROOT-jailed, bwrap sandbox)
- `edit`/`apply_patch` (snapshotted for undo), `todowrite`, `question`, `task` (subagents), `skill`, `webfetch`/`websearch`
- Viora domain: `schematic_query`/`schematic_render`/`pcb_render`/`netlist_run`/`netlist_validate`/`erc`/`pcb_validate`/`symbol_search`/`footprint_list`/`raw_export`/`viora` generic

## Permissions
`bash` default allow; dangerous commands ask (`rm`, `dd`, `shutdown`…, compound/wrapped/smuggled forms detected); `sudo` denied; `rm -rf /`, fork bombs, raw-disk writes never run. `write`/`edit` outside project roots ask; shell redirects to external files count as writes. Explicit approval (dialog/`-y`) lifts jail per call. See `AGENTS.md`.

## Skills
Bundled `skills/{flux,sim,pcb,erc}/`, project `./skills/*/`, global `~/.config/vioraharness/skills/*/` — auto-loaded on intent. TUI: `/skills` browser, `/skill-new [--local] <description>` (AI names + writes SKILL.md).

## Session
SQLite `~/.local/share/vioraharness/sessions.db` — event-sourced, survives restart (`migrations/`). CLI: `resume`/`fork`/`rename`/`archive`/`delete`/`export`/`history`; TUI `/resume`; server REST + SSE.

## Workflow Invariant
Netlist-first: `.cir` → `netlist_run --measure --assert` → `raw_export` → `schematic_render` PNG (vision feedback). Validate `erc/drc` before `pcb-compose --auto-route`.
