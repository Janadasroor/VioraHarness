---
name: pcb
description: PCB composer
triggers: [pcb, footprint, route, compose, sync, export, cleanup, autoroute]
---
# PCB Skill
1. Preflight: `erc`/`schematic_validate` on `.flxsch`, `drc`/`pcb_validate` on `.pcb` before any compose.
2. Compose: `pcb_compose` with `--add-component/--add-trace/--add-via/--delete-item/--shrink-outline/--add-netclass/--assign-net/--add-pour`; standalone routing via `pcb_autoroute` (`ripup`, `grid`), cleanup via `pcb_cleanup`.
3. Sync: `pcb_sync` after schematic ECO; inspect with `pcb_query`/`pcb_netlist`.
4. Post: `pcb_render` base64 vision + `pcb_validate`; manufacturing via `pcb_export` (gerber|pdf|step|iges|ipc2581|odb|pos). `--auto-route` is Ask-gated.
