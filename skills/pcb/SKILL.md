---
name: pcb
description: PCB composer
triggers: [pcb, footprint, route, compose]
---
# PCB Skill
Use pcb_compose --add-component/--add-trace/--add-via + --auto-route.
Validate erc/drc before compose, then pcb_render base64.
