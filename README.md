# VioraHarness

VioraHarness is a custom coding-agent harness for **VioraEDA**, the C++20/Qt6 SPICE toolchain. It implements the full-control harness pattern: the harness owns the agent loop, tool registry, permission policy, OS sandbox, session journal, and terminal UI, while the language model (via OpenRouter, Gemini, or the managed gateway) provides reasoning only.

The stack is Rust throughout, with a Ratatui terminal interface, a Tokio-based HTTP server with SSE streaming, and an SQLite-backed event-sourced session store.

## Features

- **Agent loop** with a fixed turn budget, loop-guard heuristics, automatic compaction, and durable per-turn persistence.
- **Typed tool registry**: filesystem, shell, patch, web, and VioraEDA domain tools (`schematic`, `netlist`, `erc`, `pcb`, `raw_export`), each validated against a JSON Schema before execution (fail-closed).
- **Permission policy**: flat allow/ask/deny rules with last-match-wins semantics. Destructive operations require explicit approval; annihilation patterns (filesystem wipes, fork bombs, raw-disk writes) are unconditionally blocked.
- **Sandboxing**: strict `bwrap` confinement on Linux with a read-only host bind, scoped writable directories, and offscreen Qt platform for headless simulation.
- **Session persistence**: event-sourced SQLite journal. Sessions survive restarts and support resume, fork, rename, archive, export, and file-level undo.
- **Subagents**: single-shot `explore`, `planner`, `coder`, and `reviewer` runs with per-kind tool allowlists, per-chat scoping, and read-only transcripts.
- **Skills**: bundled and user-defined `SKILL.md` packs, loaded on intent detection (progressive disclosure).
- **Interfaces**: interactive TUI, one-shot CLI execution, HTTP SSE server, and a gateway shim for URL-plus-key-only clients.

## Requirements

- Rust toolchain 1.86 or later (`cargo`, `rustc`).
- Linux (sandboxed execution uses `bwrap`); degraded operation with `VIORAHARNESS_SANDBOX=off`.
- A `viora` binary on `PATH` (or `VIORA_BIN`) for EDA simulation and rendering workflows.
- At least one provider credential: `OPENROUTER_API_KEY`, `GEMINI_API_KEY`, or `OPENCODE_API_KEY` / `ZEN_API_KEY` for gateway models.

## Installation

```bash
git clone https://github.com/Janadasroor/VioraHarness
cd VioraHarness
cargo install --path . --force   # refreshes ~/.cargo/bin and ~/.local/bin
```

Verify the installation:

```bash
vioraharness doctor              # checks viora binary, keys, config, and database
```

## Quick Start

```bash
export OPENROUTER_API_KEY=sk-or-...   # or GEMINI_API_KEY, or OPENCODE_API_KEY
vioraharness doctor                   # check viora + keys + config + db
vioraharness run "hello" --model provider/model-id
vioraharness run "next step" -c -y    # continue latest chat here, auto-allow Asks
vioraharness tui                      # interactive terminal UI (Enter sends, /help lists commands)
vioraharness sessions                 # list persisted sessions
vioraharness undo                     # restore newest file snapshot
vioraharness serve --port 4096        # HTTP SSE API (loopback; bearer token when configured)
vioraharness shim --port 11435        # gateway shim for URL+key-only clients (-> /v1)
```

Run `vioraharness --help` for the full command reference.

## Configuration

Configuration is resolved hierarchically: `~/.config/vioraharness/vioraharness.json`, then `./vioraharness.json`, then `./.vioraharness/vioraharness.jsonc`, with `VIORAHARNESS_*` environment variables taking precedence. Key settings cover providers, permission triples, sandbox strictness, compaction threshold, agent mode (`eda` / `web` / `android`), theme, and notification preferences. See `AGENTS.md` for the environment variable reference.

## Providers

| Provider | Models | Credential |
|----------|--------|------------|
| OpenRouter | `provider/model-id` (live discovery via `/v1/models`) | `OPENROUTER_API_KEY` |
| Gemini | `google/*`, native thinking-config streaming | `GEMINI_API_KEY` |
| Managed gateway | `opencode/*`, `zen/*`, `go/*` families | `OPENCODE_API_KEY` / `ZEN_API_KEY` (`public` for eligible free-tier ids) |

Routing is selected per model id family: Responses-style models use the Responses endpoint, Claude/Qwen use Messages, all others use OpenAI-compatible chat. Context limits are resolved live per model for header accounting and compaction; 128k tokens is assumed when unknown.

## Permissions and Safety

- `bash` is allowed by default; dangerous forms (recursive deletes, disk writes, shutdown, process-mass-kill patterns, interpreter payloads, privilege escalation) require approval or are denied outright.
- `write`, `edit`, and `apply_patch` are automatic inside project roots and require approval outside them. Shell redirections to external files count as writes.
- Jail and sandbox decisions are made on resolved paths. An explicit approval (dialog Allow, or `-y` / `VIORAHARNESS_AUTO_ALLOW=1` for the run) lifts the jail for that call only; annihilation patterns remain blocked in all cases.

## Sessions

Sessions are stored in `~/.local/share/vioraharness/sessions.db` as an append-only event journal with SQL projections. The CLI provides `resume`, `fork`, `rename`, `archive`, `delete`, `export`, and `history`; the TUI exposes the same operations through `/resume` and `/sessions`, and the server through its REST and SSE surface.

## EDA Workflow

For circuit work the harness enforces a netlist-first order: write `.cir`, run `netlist-run --measure --assert --export-raw json`, export raw data, convert `.cir` to `.flxsch`, then render the schematic PNG for vision feedback. `erc`/`drc` validation precedes any `pcb-compose --auto-route` step.

## Documentation

- `roadmap.md` — architecture single source of truth and phased build plan.
- `ARCHITECTURE.md` — stable architecture reference with component diagram.
- `RESEARCH.md` — vendor-neutral survey of harness designs (2025-2026).
- `AGENTS.md` — agent working memory: commands, flags, modes, permissions, environment.

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for the full text.
