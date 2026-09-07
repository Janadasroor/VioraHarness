use crate::provider::ToolDefForProvider;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

pub struct ToolRegistry {
    tools: Vec<ToolDef>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut r = Self { tools: Vec::new() };
        r.register_defaults();
        r
    }

    pub fn new_empty() -> Self {
        Self { tools: Vec::new() }
    }

    fn register(&mut self, name: &str, description: &str, schema: Value) {
        self.tools.push(ToolDef {
            name: name.into(),
            description: description.into(),
            schema,
        });
    }

    fn register_defaults(&mut self) {
        self.register("read", "Read a file from the project (ROOT-jailed, supports offset/limit, truncates >2000 lines). Image files (.png/.jpg/.gif/.webp) return vision instead of text — use read on the image path to SEE it. Use path \"screenshot:latest\" (or \"last screenshot\") for the newest screenshot in ~/Pictures.", json!({
            "type":"object","properties":{
                "path":{"type":"string","description":"relative path"},
                "offset":{"type":"integer","description":"line offset (0-indexed) to start reading from"},
                "limit":{"type":"integer","description":"max lines to read (e.g. 40)"}
            },"required":["path"]
        }));
        self.register("write", "Write a file (creates parent dirs, ROOT-jailed)", json!({
            "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]
        }));
        self.register("glob", "Find files by glob pattern (limit 100)", json!({
            "type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]
        }));
        self.register("grep", "Search for pattern (ripgrep, limit 100)", json!({
            "type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]
        }));
        self.register("bash", "Run bash command (offscreen QPA, timeout 120s). Pass background:true for long builds/tests/servers: detaches immediately with a task_id (output streams to its log) instead of blocking — check /tasks, then read/grep the log; never re-run to poll.", json!({
            "type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"},"workdir":{"type":"string"},"background":{"type":"boolean","description":"detach as background task with task_id instead of blocking"}},"required":["command"]
        }));

        self.register(
            "schematic_query",
            "Get component/net JSON from .flxsch",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register("schematic_render", "Render .flxsch to PNG (returns base64)", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"},"scale":{"type":"number"}},"required":["file"]
        }));
        self.register("pcb_render", "Render .pcb to PNG (base64)", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("netlist_run", "Run SPICE sim: file + analysis/step/stop -> ok/measures/rawPath", json!({
            "type":"object","properties":{"file":{"type":"string"},"analysis":{"type":"string"},"step":{"type":"string"},"stop":{"type":"string"}},"required":["file"]
        }));
        self.register(
            "netlist_validate",
            "Validate SPICE netlist/schematic",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "erc",
            "Run ERC/DRC check",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "pcb_validate",
            "Run pcb-validate DRC",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "symbol_search",
            "Search symbols",
            json!({
                "type":"object","properties":{"query":{"type":"string"}},"required":["query"]
            }),
        );
        self.register(
            "footprint_list",
            "List footprints (--query)",
            json!({
                "type":"object","properties":{"query":{"type":"string"}},"required":[]
            }),
        );

        self.register("pcb_compose", "Programmatically add/update/remove components/traces/vias on PCB (supports --auto-route, --add-component/trace/via, --route-layers)", json!({
            "type":"object","properties":{
                "file":{"type":"string","description":"input .pcb file"},
                "add_component":{"type":"string","description":"footprint=...,x=...,y=..."},
                "add_trace":{"type":"string"},
                "add_via":{"type":"string"},
                "auto_route":{"type":"boolean"},
                "route_layers":{"type":"string","enum":["top","bottom","both"]},
                "out":{"type":"string"}
            },"required":["file"]
        }));
        self.register("pcb_init", "Initialize new PCB layout (standalone or from schematic)", json!({
            "type":"object","properties":{"file":{"type":"string"},"from_schematic":{"type":"string"}},"required":["file"]
        }));
        self.register("schematic_transform", "Apply refactor transformations to schematic (--rename-net, --prefix-ref)", json!({
            "type":"object","properties":{"file":{"type":"string"},"rename_net":{"type":"string"},"prefix_ref":{"type":"string"}},"required":["file"]
        }));
        self.register(
            "schematic_validate",
            "Validate schematic and run ERC checks (distinct from erc preflight)",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register("pcb_query", "Query PCB layout file for details", json!({
            "type":"object","properties":{"file":{"type":"string"},"query":{"type":"string"}},"required":["file"]
        }));
        self.register("raw_info", "Display info about .raw simulation file (--summary)", json!({
            "type":"object","properties":{"file":{"type":"string"},"summary":{"type":"boolean"}},"required":["file"]
        }));
        self.register("raw_stats", "Compute signal metrics (min/max/avg/RMS) for .raw", json!({
            "type":"object","properties":{"file":{"type":"string"},"signal":{"type":"string"}},"required":["file"]
        }));
        self.register(
            "symbol_list",
            "List symbols in folder or .sclib",
            json!({
                "type":"object","properties":{"path":{"type":"string"}},"required":[]
            }),
        );
        self.register("symbol_render", "Render .viosym symbol to PNG", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("footprint_render", "Render .json footprint to PNG", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("flux", "Run FluxScript integration (flux eval/run/validate) — experimental", json!({
            "type":"object","properties":{"command":{"type":"string","enum":["eval","run","validate"]},"file":{"type":"string"},"script":{"type":"string"}},"required":["command"]
        }));
        self.register("raw_export", "Export .raw waveform to json/csv", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"},"format":{"type":"string"}},"required":["file"]
        }));
        self.register("netlist_to_schematic", "Convert .cir netlist to .flxsch schematic (netlist-first workflow)", json!({
            "type":"object","properties":{"file":{"type":"string","description":"input .cir path"},"out":{"type":"string","description":"output .flxsch path"}},"required":["file"]
        }));
        self.register("viora", "Generic viora wrapper: cmd string (e.g. \"schematic-diff a b --json\")", json!({
            "type":"object","properties":{"cmd":{"type":"string"},"timeout":{"type":"integer"}},"required":["cmd"]
        }));
        self.register("task", "Spawn a subagent (explore/planner/coder) with isolated context and filtered tools. Use for parallel exploration or delegated work.", json!({
            "type":"object",
            "properties":{
                "prompt":{"type":"string","description":"Task prompt for subagent"},
                "kind":{"type":"string","enum":["explore","planner","coder","reviewer"],"description":"Subagent kind — explore=read-only fast, planner=draft plan, coder=full"},
                "model":{"type":"string","description":"Optional model override"}
            },
            "required":["prompt"]
        }));
        self.register("skill", "Load a skill's SKILL.md on demand (progressive disclosure). Returns skill content for domain guidance.", json!({
            "type":"object","properties":{"name":{"type":"string","description":"skill name: sim|pcb|flux|erc or path"}},"required":["name"]
        }));

        self.register("question", "Ask the user clarifying questions with fixed options (interactive in TUI; headless runs must use best judgment). Use when ambiguous instead of guessing.", json!({
            "type":"object",
            "properties":{
                "questions":{
                    "type":"array",
                    "description":"1-4 questions",
                    "items":{
                        "type":"object",
                        "properties":{
                            "question":{"type":"string"},
                            "header":{"type":"string"},
                            "multiSelect":{"type":"boolean"},
                            "options":{
                                "type":"array",
                                "items":{
                                    "type":"object",
                                    "properties":{
                                        "label":{"type":"string"},
                                        "description":{"type":"string"}
                                    },
                                    "required":["label"]
                                }
                            }
                        },
                        "required":["question","options"]
                    }
                }
            },
            "required":["questions"]
        }));
        self.register("todowrite", "Replace the session task list. Each call sets the full list; use merge:true to upsert. Mark exactly one in_progress.", json!({
            "type":"object",
            "properties":{
                "todos":{
                    "type":"array",
                    "description":"Full task list",
                    "items":{
                        "type":"object",
                        "properties":{
                            "content":{"type":"string"},
                            "status":{"type":"string","enum":["pending","in_progress","completed"]},
                            "priority":{"type":"string","enum":["high","medium","low"]}
                        },
                        "required":["content","status"]
                    }
                },
                "merge":{"type":"boolean","description":"Upsert by content instead of replacing"}
            },
            "required":["todos"]
        }));
        self.register("edit", "Exact-string file edit. Replaces ONE occurrence of old_string; fails closed on zero/multi matches unless replace_all. Prefer over write for small changes.", json!({
            "type":"object",
            "properties":{
                "path":{"type":"string"},
                "old_string":{"type":"string","description":"Exact text to replace (whitespace included)"},
                "new_string":{"type":"string"},
                "replace_all":{"type":"boolean"}
            },
            "required":["path","old_string","new_string"]
        }));
        self.register("apply_patch", "Apply a patch text (*** Begin Patch … *** End Patch with *** Add/Delete/Update File, *** Move to:, @@ hunks). Exact matching, no fuzz.", json!({
            "type":"object",
            "properties":{
                "patch":{"type":"string","description":"Full patch text"}
            },
            "required":["patch"]
        }));
        self.register("webfetch", "Fetch a URL as text (docs, datasheets, references). HTML is converted to readable text; JSON/text pass through truncated.", json!({
            "type":"object",
            "properties":{
                "url":{"type":"string"},
                "max_chars":{"type":"integer","minimum":100,"maximum":100000}
            },
            "required":["url"]
        }));
        self.register("websearch", "Keyless web search (docs, parts, references). Returns [{title, url, snippet}]; follow up with webfetch.", json!({
            "type":"object",
            "properties":{
                "query":{"type":"string"},
                "count":{"type":"integer","minimum":1,"maximum":10}
            },
            "required":["query"]
        }));
    }

    pub fn all(&self) -> &[ToolDef] {
        &self.tools
    }

    pub fn visible_tools(&self, allowed_names: Option<&[String]>) -> Vec<&ToolDef> {
        match allowed_names {
            None => self.tools.iter().collect(),
            Some(allow) => self
                .tools
                .iter()
                .filter(|t| allow.contains(&t.name))
                .collect(),
        }
    }

    pub fn to_provider_tools(&self) -> Vec<ToolDefForProvider> {
        self.tools
            .iter()
            .map(|t| ToolDefForProvider {
                call_type: "function".into(),
                function: crate::provider::ToolFunction {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.schema.clone(),
                },
            })
            .collect()
    }

    pub fn add_tool(&mut self, def: ToolDef) {
        self.tools.push(def);
    }
}
