---
name: sim
description: Netlist-first simulation loop
triggers: [netlist, simulate, raw, tran, ac, measure, assert, compare, bom]
---
# Sim Skill
1. Netlist-first: write .cir -> `netlist_validate` -> `netlist_run` with `--measure/--assert/--export-raw json` -> `raw_export`/`raw_stats` -> `netlist_to_schematic` -> `schematic_render` PNG vision.
2. Convergence: retry with `compat:true` + `robust:true`; slice with `range:"t0:t1"`.
3. From schematic: `schematic_netlist` (spice|json) then `netlist_run`; cross-check with `netlist_compare` (schematic vs .cir) and `schematic_bom` for parts.
4. After every mutation, re-render PNG (`schematic_render`) and re-run `netlist_run`; describe waveforms, don't dump pixels.
