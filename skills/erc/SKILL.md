---
name: erc
description: ERC/DRC autofix
triggers: [erc, drc, validate, autofix, cleanup]
---
# ERC Skill
1. Run `schematic_validate`/`erc` before any `pcb-compose`; `drc`/`pcb_validate` before `--auto-route`.
2. Try `autofix` on the failing `.flxsch`/`.pcb` first, then re-run checks + `schematic_render`/`pcb_render` to verify.
3. Dedup wires, snap to 0.1mm; use `pcb_cleanup` for dangling tracks/vias before final DRC.
