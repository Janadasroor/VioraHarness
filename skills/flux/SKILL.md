---
name: flux
description: FluxScript synthesis — generate/validate .flux via viora flux run/eval
triggers: [flux, script, JIT, smart signal, symbol, footprint]
---
# Flux Skill
Use `flux eval` for inline, `flux run` for files, validate via `flux validate`.
Prefer `viora flux eval "code" --json` for quick checks.
Symbols: `symbol_search`/`symbol_list` -> `symbol_validate` -> `symbol_render`; footprints: `footprint_list` -> `footprint_import` (kicad_mod -> json) -> `footprint_render`.
