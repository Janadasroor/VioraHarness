# VioraHarness — Architecture
> Extracted from `roadmap.md §2`. This file is the stable architecture reference; `roadmap.md` remains the SSOT for sequencing.

## Diagram

```
[ TUI (Ratatui) ] ──JSONL stdio──►┌─────────────────────────┐
[ exec / serve ] ──HTTP SSE :4096►│   VioraHarness Server   │
[  web / IDE  ] ───HTTP SSE─────►│   (Rust, Tokio, Axum)   │
                                  │  ThreadManager          │
                                  │  AgentLoop (while true)│
                                  │  Provider (OR+Gemini)  │
                                  │  ToolRegistry           │
                                  │  Permissions            │
                                  │  Context Assembler      │
                                  │  Sandbox                │
                                  │  SessionStore (SQLite) │
                                  │  SubagentPool           │
                                  │  MCP Bridge             │
                                  └──────────┬──────────────┘
                                             │ --json + PNG base64
                                             ▼
                                   ┌────────────────────────┐
                                   │ Workspace viora daemon │
                                   │ QLocalSocket p2        │
                                   │ VioMATRIXC + Flux JIT  │
                                   └────────────────────────┘
```

Pattern: long-lived harness server — `stdio reader + message processor + thread manager + core threads`.

## Primitives

| # | Name | Crate Path | Key Invariant |
|---|------|------------|---------------|
| 1 | Agent Loop | `crates/core/src/loop/mod.rs` | `Receive→Assemble→CallLLM→Dispatch→Observe→Compact?→Repeat`; explicit recovery branches. |
| 2 | Context | `crates/core/src/context/` | L1 sys + L2 git/QPA + L3 AGENTS.md + L4 memory (parallel prefetch) + L5 retriever. |
| 3 | Tools/Skills | `crates/core/src/tools/` | `defineTool!` + `.txt` descriptions; skills `skills/SKILL.md` progressive disclosure. |
| 4 | Permissions | `crates/core/src/permissions/` | Flat triples last-match wins, default ask, `visibleTools()` filter. |
| 5 | Sandbox | `crates/core/src/sandbox/` | bwrap/Landlock/seccomp Linux, Seatbelt macOS, Docker fallback. |
| 6 | Session | `crates/core/src/session/` | SQLite event journal + projector, snapshot for `/undo`. |
| 7 | Memory | `crates/core/src/memory/` | `AGENTS.md` always-loaded + `MEMORY.md` learned, atomic fs. |
| 8 | Subagents | `crates/core/src/subagent/` | `Task` spawn isolated context, filtered tools, structured return. |
| 9 | Observability | `crates/core/src/observe/` | `tracing` spans + CostTracker + lifecycle hooks. |

## Contracts

- Tool call validated via JSON Schema `strict` before exec; `invalid-call` handler; stack traces stripped.
- Steering queue injects only before next model call, never mid-tool.
- `ask → Deferred` parked; TUI resolves.
- Compaction threshold 80% → summary subagent with tools denied.
- Image feedback via `schematic-render`/`pcb-render` base64 `image_url` parts.

See `roadmap.md §2` for data flow and §5 for tool tiers.
