---
name: sim
description: Netlist-first simulation loop
triggers: [netlist, simulate, raw, tran, ac]
---
# Sim Skill
1. write .cir -> netlist_run --measure --assert --export-raw json -> raw_export -> schematic_render
2. After mutation, render PNG as image_url vision.
3. Use --compat --robust for convergence, --range t0:t1 for slices.
