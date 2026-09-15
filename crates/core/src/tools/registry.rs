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
        self.register("read", "Read a file from the project (ROOT-jailed). Paginate big files with offset/limit (0-indexed lines). Caps: 2000 lines per call; the model-visible result is further capped (~8k chars, full JSON saved to /tmp for `cat`). Image files (.png/.jpg/.gif/.webp) return vision instead of text — use read on the image path to SEE it. Use path \"screenshot:latest\" (or \"last screenshot\") for the newest screenshot in ~/Pictures.", json!({
            "type":"object","properties":{
                "path":{"type":"string","description":"relative path"},
                "offset":{"type":"integer","description":"line offset (0-indexed) to start reading from"},
                "limit":{"type":"integer","description":"max lines to read (e.g. 40)"}
            },"required":["path"]
        }));
        self.register("write", "Write a file (creates parent dirs, ROOT-jailed)", json!({
            "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]
        }));
        self.register("glob", "Find files by glob pattern (limit 100). `path` is the base directory to search under (default project root). `*` spans within a segment, `?` one char, `**` spans directories — e.g. `**/*.cir` finds nested files.", json!({
            "type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"]
        }));
        self.register("grep", "Search file contents (ripgrep if present, else system grep -R; limit 100). `path` is the base directory. Returns ok:false only when no search backend exists.", json!({
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
        self.register("schematic_render", "Render .flxsch to PNG (returns base64 vision). Netlist-first: write .cir -> netlist-run -> raw_export -> netlist-to-schematic -> schematic-render. Re-render after every schematic mutation.", json!({
            "type":"object","properties":{"file":{"type":"string","description":".flxsch (or .cir, auto-converted via netlist-to-schematic)"},"out":{"type":"string","description":"output PNG path (default /tmp/viora_render.png)"},"scale":{"type":"number","description":"render scale (default 4.0)"},"transparent":{"type":"boolean","description":"transparent PNG background"}},"required":["file"]
        }));
        self.register("pcb_render", "Render .pcb to PNG (base64 vision). Re-render after every pcb-compose for visual feedback.", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("netlist_run", "Run SPICE sim on .cir/.flxsch. Netlist-first entry point: prefer --measure/--assert/--export-raw json, then raw_export. Use --compat/--robust for convergence.", json!({
            "type":"object","properties":{
                "file":{"type":"string","description":".cir or .flxsch input"},
                "analysis":{"type":"string","enum":["op","tran","ac"],"description":"analysis type"},
                "step":{"type":"string","description":"transient step (e.g. 1u)"},
                "stop":{"type":"string","description":"transient stop (e.g. 1m)"},
                "compat":{"type":"boolean","description":"backward-compat transforms for .cir"},
                "robust":{"type":"boolean","description":"robust solver options for convergence"},
                "stats":{"type":"boolean","description":"statistical summary of signals"},
                "measure":{"type":["string","array"],"description":"measurement expr(s), repeatable (e.g. V(out)_avg > 0.5)","items":{"type":"string"}},
                "assert":{"type":["string","array"],"description":"pass/fail assertion expr(s), repeatable","items":{"type":"string"}},
                "measure_format":{"type":"string","enum":["text","json"]},
                "range":{"type":"string","description":"time slice t0:t1 for stats/measure/export"},
                "signal":{"type":["string","array"],"description":"signal(s) to export","items":{"type":"string"}},
                "max_points":{"type":"integer","minimum":1},
                "base_signal":{"type":"string"},
                "export_raw":{"type":"string","description":"raw export: format csv|json|parquet OR legacy output path (/tmp/x.raw|.json|.csv — format inferred, file copied there)"},
                "timeout":{"type":"string","description":"sim timeout (e.g. 60s)"}
            },"required":["file"]
        }));
        self.register(
            "netlist_validate",
            "Validate SPICE netlist syntax (.cir). Run before netlist_run; also auto-runs after write/edit of .cir/.sp/.flxsch.",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "erc",
            "Run electrical rules check (ERC) on a .flxsch schematic. Run before pcb-compose.",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "drc",
            "Run design rules check (DRC) on a .pcb layout. Alias of pcb-validate/pcb-drc.",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "pcb_validate",
            "Validate PCB layout and run DRC (pcb-validate). Run before pcb-compose --auto-route.",
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

        self.register("pcb_compose", "Programmatically add/update/remove components/traces/vias on PCB. Validate erc/drc first; after compose run pcb_render (vision) + pcb_validate.", json!({
            "type":"object","properties":{
                "file":{"type":"string","description":"input .pcb file"},
                "add_component":{"type":"string","description":"footprint=...,x=...,y=...,rotation=...,layer=...,name=...,value=..."},
                "add_trace":{"type":"string","description":"x1=...,y1=...,x2=...,y2=...,width=...,layer=...,net=..."},
                "add_via":{"type":"string","description":"x=...,y=...,diameter=...,drill=...,net=..."},
                "delete_item":{"type":"string","description":"id=... OR name=..."},
                "shrink_outline":{"type":"string","description":"margin=<val_in_mm>"},
                "add_netclass":{"type":"string","description":"name=...,width=...,clearance=..."},
                "assign_net":{"type":"string","description":"net=...,class=..."},
                "add_pour":{"type":"string","description":"layer=...,net=...,clearance=..."},
                "auto_route":{"type":"boolean","description":"auto-route after compose (Ask-gated)"},
                "allow_diagonals":{"type":"boolean","description":"allow 45-degree diagonals in auto-router"},
                "route_layers":{"type":"string","enum":["top","bottom","both"]},
                "out":{"type":"string","description":"output .pcb path"}
            },"required":["file"]
        }));
        self.register("pcb_sync", "Synchronize a .pcb layout with a .flxsch schematic (incremental netlist/footprint import).", json!({
            "type":"object","properties":{"file":{"type":"string","description":".pcb to update"},"schematic":{"type":"string","description":"source .flxsch"},"out":{"type":"string"}},"required":["file","schematic"]
        }));
        self.register("pcb_export", "Export PCB to manufacturing formats (gerber|pdf|step|iges|ipc2581|odb|pos).", json!({
            "type":"object","properties":{"file":{"type":"string"},"format":{"type":"string","enum":["gerber","pdf","step","iges","ipc2581","odb","pos"]},"output":{"type":"string","description":"output dir or file (default ./output)"}},"required":["file"]
        }));
        self.register("pcb_autoroute", "Run multi-layer auto-router on a .pcb file (standalone; pcb-compose --auto-route composes+routes).", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"},"ripup":{"type":"boolean","description":"rip up existing traces/vias first"},"grid":{"type":"number","description":"grid mm (default 0.5)"}},"required":["file"]
        }));
        self.register("pcb_cleanup", "Board cleanup: purge dangling tracks/vias, zero-length tracks, duplicate vias, merge collinear segments.", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register(
            "pcb_netlist",
            "Dump detailed netlist/connectivity report of a .pcb file.",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
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
        self.register("schematic_netlist", "Generate SPICE or JSON netlist from a .flxsch schematic.", json!({
            "type":"object","properties":{"file":{"type":"string"},"format":{"type":"string","enum":["spice","json"]},"analysis":{"type":"string","enum":["op","tran","ac"]},"step":{"type":"string"},"stop":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("schematic_bom", "Generate Bill of Materials (BOM) from a .flxsch schematic.", json!({
            "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("netlist_compare", "Compare schematic-generated netlist against an external netlist.", json!({
            "type":"object","properties":{"schematic":{"type":"string","description":".flxsch source"},"netlist":{"type":"string","description":"external .cir to compare"},"analysis":{"type":"string"},"step":{"type":"string"},"stop":{"type":"string"}},"required":["schematic","netlist"]
        }));
        self.register("autofix", "Attempt automatic fix of common schematic/PCB ERC/DRC violations.", json!({
            "type":"object","properties":{"file":{"type":"string","description":".flxsch or .pcb file"},"out":{"type":"string"}},"required":["file"]
        }));
        self.register("raw_info", "Display info about .raw simulation file (--summary)", json!({
            "type":"object","properties":{"file":{"type":"string"},"summary":{"type":"boolean"}},"required":["file"]
        }));
        self.register("raw_stats", "Compute signal metrics (min/max/avg/RMS) for .raw. Pair with raw_export for slices.", json!({
            "type":"object","properties":{"file":{"type":"string"},"signal":{"type":"string","description":"repeatable in viora; comma-join or repeat via array"},"range":{"type":"string","description":"t0:t1 slice"}},"required":["file"]
        }));
        self.register(
            "symbol_list",
            "List symbols in folder or .sclib",
            json!({
                "type":"object","properties":{"path":{"type":"string"}},"required":[]
            }),
        );
        self.register(
            "symbol_validate",
            "Validate .viosym symbol file compliance.",
            json!({
                "type":"object","properties":{"file":{"type":"string"}},"required":["file"]
            }),
        );
        self.register(
            "footprint_import",
            "Import KiCad footprint (.kicad_mod) to VioraEDA (.json).",
            json!({
                "type":"object","properties":{"file":{"type":"string"},"out":{"type":"string"},"render":{"type":"boolean"},"limit":{"type":"integer","minimum":1}},"required":["file"]
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
        self.register("raw_export", "Export .raw waveform to json/csv/parquet. Follow netlist_run --export-raw; preview capped, full file at out.", json!({
            "type":"object","properties":{
                "file":{"type":"string"},
                "out":{"type":"string"},
                "format":{"type":"string","enum":["json","csv","parquet"]},
                "signal":{"type":["string","array"],"items":{"type":"string"}},
                "signal_regex":{"type":"string"},
                "max_points":{"type":"integer","minimum":1},
                "base_signal":{"type":"string"},
                "range":{"type":"string","description":"t0:t1 slice"}
            },"required":["file"]
        }));
        self.register("netlist_to_schematic", "Convert .cir netlist to .flxsch schematic (netlist-first workflow)", json!({
            "type":"object","properties":{"file":{"type":"string","description":"input .cir path"},"out":{"type":"string","description":"output .flxsch path"}},"required":["file"]
        }));
        self.register("viora", "Generic viora wrapper: cmd string (e.g. \"schematic-diff a b --json\")", json!({
            "type":"object","properties":{"cmd":{"type":"string"},"timeout":{"type":"integer"}},"required":["cmd"]
        }));
        self.register("task", "Spawn a subagent (explore/planner/coder/reviewer) with isolated context and filtered tools. Use for parallel exploration or delegated work. background:true detaches it (run id returned at once, result arrives as a follow-up turn) so the turn continues; default false blocks until done.", json!({
            "type":"object",
            "properties":{
                "prompt":{"type":"string","description":"Task prompt for subagent"},
                "kind":{"type":"string","enum":["explore","planner","coder","reviewer"],"description":"Subagent kind — explore=read-only fast, planner=draft plan, coder=full"},
                "model":{"type":"string","description":"Optional model override"},
                "background":{"type":"boolean","description":"Detach: return the run id at once, result follows later"}
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
        self.register(            "browser_screenshot",
            "Screenshot a page with headless Chrome CLI (local .html or http(s), incl. localhost dev servers). Returns PNG base64 vision — use after web edits for visual feedback, like schematic-render. Local targets with default out also auto-save ./screenshot-latest.png (pass out: to override).", json!({
            "type":"object",
            "properties":{
                "target":{"type":"string","description":"http(s) URL or local .html path"},
                "out":{"type":"string","description":"output PNG path (default /tmp)"},
                "width":{"type":"integer","minimum":320,"maximum":3840},
                "height":{"type":"integer","minimum":240,"maximum":2160},
                "delay_ms":{"type":"integer","minimum":0,"maximum":15000,"description":"JS settle time (virtual-time-budget)"}
            },
            "required":["target"]
        }));
        self.register("browser_dom", "Dump rendered DOM text with headless Chrome CLI (--dump-dom + virtual-time-budget so JS runs). Use to verify web edits without images.", json!({
            "type":"object",
            "properties":{
                "target":{"type":"string","description":"http(s) URL or local .html path"},
                "max_chars":{"type":"integer","minimum":100,"maximum":100000},
                "wait_ms":{"type":"integer","minimum":0,"maximum":15000}
            },
            "required":["target"]
        }));
        self.register(
            "browser_pdf",
            "Export a page to PDF with headless Chrome CLI (--print-to-pdf, no headers/footers).",
            json!({
                "type":"object",
                "properties":{
                    "target":{"type":"string","description":"http(s) URL or local .html path"},
                    "out":{"type":"string","description":"output PDF path (default /tmp)"}
                },
                "required":["target"]
            }),
        );
        self.register(
            "browser_open",
            "Open a page in the user's VISIBLE host Chrome (new window, outside the sandbox). Ask-gated: use when the user should see/click the page themselves; verify with browser_screenshot, never xdotool.",
            json!({
                "type":"object",
                "properties":{
                    "target":{"type":"string","description":"http(s) URL or local .html path"}
                },
                "required":["target"]
            }),
        );
        self.register("dev_serve", "Serve a directory over localhost (python3 http.server) as a detached background task. Returns {url, task_id, log} — screenshot/fetch the url, then /tasks kill.", json!({
            "type":"object",
            "properties":{
                "dir":{"type":"string","description":"directory to serve (default .)"},
                "port":{"type":"integer","minimum":0,"maximum":65535,"description":"0 = pick a free port"}
            },
            "required":[]
        }));
        self.register("adb_devices", "List attached Android devices (adb devices -l). Returns [{serial, state, details}]. Confirm the target before any device action.", json!({
            "type":"object","properties":{"timeout":{"type":"integer"}},"required":[]
        }));
        self.register("adb_shell", "Run a shell command on an Android device. Serial auto-selects when exactly one device is attached (else pass serial or set ANDROID_SERIAL).", json!({
            "type":"object","properties":{
                "command":{"type":"string","description":"shell command (e.g. getprop ro.build.version.sdk)"},
                "serial":{"type":"string"},
                "timeout":{"type":"integer"}
            },"required":["command"]
        }));
        self.register("adb_install", "Install an APK on a device (Ask-gated). Device errors (e.g. signature conflicts) surface in stdout.", json!({
            "type":"object","properties":{
                "apk":{"type":"string","description":"in-root .apk path"},
                "serial":{"type":"string"},
                "reinstall":{"type":"boolean","description":"-r keep data"},
                "downgrade":{"type":"boolean","description":"-d allow downgrade"},
                "grant":{"type":"boolean","description":"-g grant runtime permissions"},
                "test":{"type":"boolean","description":"-t allow test packages"},
                "timeout":{"type":"integer"}
            },"required":["apk"]
        }));
        self.register("adb_logcat", "Dump device logs (adb logcat -d). Dump mode only — for a live tail use bash background:true and follow /tasks.", json!({
            "type":"object","properties":{
                "serial":{"type":"string"},
                "lines":{"type":"integer","description":"-t last N lines"},
                "format":{"type":"string","description":"-v format (e.g. brief, time, color)"},
                "filter":{"type":["string","array"],"description":"logcat filter specs (e.g. ActivityManager:I)","items":{"type":"string"}},
                "timeout":{"type":"integer"}
            },"required":[]
        }));
        self.register(
            "adb_screenshot",
            "Capture the device screen to PNG (returns base64 vision). Use after UI actions instead of running blind.",
            json!({
                "type":"object","properties":{
                    "serial":{"type":"string"},
                    "out":{"type":"string","description":"output PNG path (default /tmp/adb_screenshot.png)"},
                    "timeout":{"type":"integer"}
                },"required":[]
            }),
        );
        self.register(
            "adb_push",
            "Push a host file to a device (src must be in-root).",
            json!({
                "type":"object","properties":{
                    "src":{"type":"string","description":"in-root host path"},
                    "dst":{"type":"string","description":"device path (e.g. /data/local/tmp/x)"},
                    "serial":{"type":"string"},
                    "timeout":{"type":"integer"}
                },"required":["src","dst"]
            }),
        );
        self.register(
            "adb_pull",
            "Pull a device file to the host (dst parent created, jailed).",
            json!({
                "type":"object","properties":{
                    "src":{"type":"string","description":"device path"},
                    "dst":{"type":"string","description":"in-root host path"},
                    "serial":{"type":"string"},
                    "timeout":{"type":"integer"}
                },"required":["src","dst"]
            }),
        );
        self.register("emulator", "Emulator control: list AVDs, or boot one as a detached background task (kill via /tasks). Boot runs outside bwrap (the emulator is itself a KVM VM).", json!({
            "type":"object","properties":{
                "action":{"type":"string","enum":["list","boot"]},
                "avd":{"type":"string","description":"AVD name for boot (see list)"},
                "wipe":{"type":"boolean","description":"-wipe-data"},
                "no_snapshot":{"type":"boolean","description":"-no-snapshot (default true)"},
                "wait":{"type":"boolean","description":"block until a new device appears + boot_completed"},
                "timeout":{"type":"integer"}
            },"required":[]
        }));
        self.register("gradle", "Run Gradle (project gradlew preferred, else gradle on PATH). Long builds: background:true detaches with task_id. Runs outside bwrap (needs SDK + ~/.gradle); project dir stays jailed.", json!({
            "type":"object","properties":{
                "dir":{"type":"string","description":"project dir with gradlew (default .)"},
                "tasks":{"type":["string","array"],"description":"gradle tasks (default [assembleDebug])","items":{"type":"string"}},
                "offline":{"type":"boolean","description":"--offline"},
                "args":{"type":"array","description":"extra raw args","items":{"type":"string"}},
                "background":{"type":"boolean"},
                "timeout":{"type":"integer"}
            },"required":[]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(r: &ToolRegistry) -> Vec<&str> {
        r.all().iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn defaults_cover_core_and_viora_tools() {
        let r = ToolRegistry::new();
        let n = names(&r);
        for must in [
            "read",
            "write",
            "glob",
            "grep",
            "bash",
            "edit",
            "apply_patch",
            "todowrite",
            "question",
            "task",
            "skill",
            "webfetch",
            "websearch",
            "browser_screenshot",
            "browser_dom",
            "browser_pdf",
            "browser_open",
            "dev_serve",
            "viora",
            "adb_devices",
            "adb_shell",
            "adb_install",
            "adb_logcat",
            "adb_push",
            "adb_pull",
            "adb_screenshot",
            "emulator",
            "gradle",
            "netlist_run",
            "netlist_validate",
            "netlist_to_schematic",
            "netlist_compare",
            "schematic_render",
            "schematic_query",
            "schematic_validate",
            "schematic_netlist",
            "schematic_bom",
            "erc",
            "drc",
            "pcb_validate",
            "pcb_compose",
            "pcb_sync",
            "pcb_export",
            "pcb_autoroute",
            "pcb_cleanup",
            "pcb_netlist",
            "pcb_query",
            "autofix",
            "symbol_validate",
            "footprint_import",
            "raw_export",
            "raw_stats",
        ] {
            assert!(n.contains(&must), "missing tool: {must}");
        }
        let mut sorted = n.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), n.len(), "duplicate tool names");
    }

    #[test]
    fn schemas_are_well_formed() {
        let r = ToolRegistry::new();
        for t in r.all() {
            assert_eq!(
                t.schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{}",
                t.name
            );
            assert!(
                t.schema
                    .get("required")
                    .and_then(|v| v.as_array())
                    .is_some(),
                "{}",
                t.name
            );
            assert!(!t.description.trim().is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn visible_tools_filters() {
        let r = ToolRegistry::new();
        assert_eq!(r.visible_tools(None).len(), r.all().len());
        let sub = r.visible_tools(Some(&["read".to_string(), "bash".to_string()]));
        assert_eq!(sub.len(), 2);
        let none = r.visible_tools(Some(&["nope".to_string()]));
        assert!(none.is_empty());
    }

    #[test]
    fn provider_shape_and_add() {
        let mut r = ToolRegistry::new_empty();
        assert!(r.all().is_empty());
        r.add_tool(ToolDef {
            name: "x".into(),
            description: "d".into(),
            schema: json!({"type": "object"}),
        });
        let pt = r.to_provider_tools();
        assert_eq!(pt.len(), 1);
        assert_eq!(pt[0].call_type, "function");
        assert_eq!(pt[0].function.name, "x");
        let full = ToolRegistry::new().to_provider_tools();
        assert_eq!(full.len(), ToolRegistry::new().all().len());
    }
}
