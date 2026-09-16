pub mod android;
pub mod bash;
pub mod browser;
pub mod fs;
pub mod patch;
pub mod question;
pub mod registry;
pub mod tasks;
pub mod todo;
pub mod viora;
pub mod web;

use serde_json::{json, Value};

pub use registry::{ToolDef, ToolRegistry};

fn validate_against_schema(name: &str, args: &Value) -> Option<Value> {
    let reg = ToolRegistry::new();
    let def = reg.all().iter().find(|t| t.name == name)?;
    let schema = &def.schema;

    if let Some(required) = schema.get("required").and_then(|v| v.as_array()) {
        for req in required {
            if let Some(key) = req.as_str() {
                if args.get(key).is_none() {
                    return Some(
                        json!({"ok": false, "error": format!("schema validation failed for {name}: missing required field '{key}'")}),
                    );
                }
            }
        }
    }

    let is_valid = std::panic::catch_unwind(|| jsonschema::is_valid(schema, args)).unwrap_or(true);
    if !is_valid {
        return Some(
            json!({"ok": false, "error": format!("schema validation failed for {name}: args do not match schema")}),
        );
    }
    None
}

async fn after_write_hook(path: &str) -> Option<Value> {
    if path.ends_with(".cir") || path.ends_with(".sp") || path.ends_with(".flxsch") {
        let out = viora::run_viora_command(
            &["netlist-validate".into(), path.into(), "--json".into()],
            Some(30),
        )
        .await;
        if !out.ok {
            return Some(serde_json::json!({"lsp_diagnostics": out.stdout, "stderr": out.stderr}));
        }
    }
    None
}

/// Collect a `string | string[]` arg into a Vec (single string, array, or absent).
fn str_list(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn push_repeatable(vargs: &mut Vec<String>, flag: &str, values: Vec<String>) {
    for v in values {
        vargs.push(flag.into());
        vargs.push(v);
    }
}

fn push_flag(vargs: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if args.get(key).and_then(|v| v.as_bool()).unwrap_or(false) {
        vargs.push(flag.into());
    }
}

fn push_opt_str(vargs: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
        if !s.is_empty() {
            vargs.push(flag.into());
            vargs.push(s.into());
        }
    }
}

/// Build `viora netlist-run` argv from tool args (pure, unit-testable).
pub fn build_netlist_run_args(args: &Value) -> Vec<String> {
    let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
    let mut vargs = vec![
        "netlist-run".to_string(),
        file.to_string(),
        "--json".to_string(),
    ];
    push_opt_str(&mut vargs, args, "analysis", "--analysis");
    push_opt_str(&mut vargs, args, "step", "--step");
    push_opt_str(&mut vargs, args, "stop", "--stop");
    push_opt_str(&mut vargs, args, "timeout", "--timeout");
    push_opt_str(&mut vargs, args, "range", "--range");
    push_opt_str(&mut vargs, args, "measure_format", "--measure-format");
    push_opt_str(&mut vargs, args, "base_signal", "--base-signal");
    if let Some(s) = args.get("export_raw").and_then(|v| v.as_str()) {
        if !s.is_empty() {
            vargs.push("--export-raw".into());
            vargs.push(normalize_export_raw_format(s).into());
        }
    }
    push_flag(&mut vargs, args, "compat", "--compat");
    push_flag(&mut vargs, args, "robust", "--robust");
    push_flag(&mut vargs, args, "stats", "--stats");
    push_repeatable(&mut vargs, "--measure", str_list(args, "measure"));
    push_repeatable(&mut vargs, "--assert", str_list(args, "assert"));
    push_repeatable(&mut vargs, "--signal", str_list(args, "signal"));
    if let Some(n) = args.get("max_points").and_then(|v| v.as_u64()) {
        vargs.push("--max-points".into());
        vargs.push(n.to_string());
    }
    vargs
}

/// Normalize `export_raw`: accepts a format (`csv|json|parquet`) or a legacy
/// output path (`/tmp/x.raw`, `/tmp/x.json`, ...). Paths infer their format
/// from the extension (`.csv`→csv, `.parquet`→parquet, else json).
pub fn normalize_export_raw_format(s: &str) -> &str {
    match s.trim().to_lowercase().as_str() {
        "csv" => "csv",
        "parquet" => "parquet",
        "json" => "json",
        _ => {
            let lower = s.to_lowercase();
            if lower.ends_with(".csv") {
                "csv"
            } else if lower.ends_with(".parquet") || lower.ends_with(".pq") {
                "parquet"
            } else {
                "json"
            }
        }
    }
}

/// True when `export_raw` is a legacy output path rather than a bare format.
pub fn export_raw_is_path(s: &str) -> bool {
    !matches!(s.trim().to_lowercase().as_str(), "csv" | "json" | "parquet")
        && (s.contains('/') || s.contains('.'))
}

/// Build `viora pcb-compose` argv from tool args (pure, unit-testable).
pub fn build_pcb_compose_args(args: &Value) -> Vec<String> {
    let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
    let mut vargs = vec!["pcb-compose".to_string(), file.to_string()];
    for (key, flag) in [
        ("add_component", "--add-component"),
        ("add_trace", "--add-trace"),
        ("add_via", "--add-via"),
        ("delete_item", "--delete-item"),
        ("shrink_outline", "--shrink-outline"),
        ("add_netclass", "--add-netclass"),
        ("assign_net", "--assign-net"),
        ("add_pour", "--add-pour"),
        ("route_layers", "--route-layers"),
        ("out", "--out"),
    ] {
        push_opt_str(&mut vargs, args, key, flag);
    }
    push_flag(&mut vargs, args, "auto_route", "--auto-route");
    push_flag(&mut vargs, args, "allow_diagonals", "--allow-diagonals");
    vargs.push("--json".into());
    vargs
}

pub async fn execute_tool(name: &str, args: Value) -> Value {
    if let Some(err) = validate_against_schema(name, &args) {
        return err;
    }
    match name {
        "read" => fs::read_file(args).await,
        "write" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let res = fs::write_file(args.clone()).await;
            if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(diag) = after_write_hook(&path).await {
                    let mut merged = res;
                    merged["diagnostics"] = diag;
                    return merged;
                }
            }
            res
        }
        "glob" => fs::glob_files(args).await,
        "grep" => fs::grep(args).await,
        "bash" => bash::bash(args).await,
        "viora" => {
            let cmd_str = args.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
            let timeout = args.get("timeout").and_then(|v| v.as_u64());
            if cmd_str.is_empty() {
                return json!({"ok": false, "error": "missing cmd"});
            }
            let parts: Vec<String> = shell_words::split(cmd_str)
                .unwrap_or_else(|_| cmd_str.split_whitespace().map(|s| s.to_string()).collect());
            let out = viora::run_viora_command(&parts, timeout).await;
            json!({
                "ok": out.ok,
                "code": out.code,
                "stdout": out.stdout,
                "stderr": out.stderr,
                "data": out.data
            })
        }

        "schematic_query" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["schematic-query".into(), file.into(), "--json".into()],
                Some(60),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "schematic_render" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out_path = args
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("/tmp/viora_render.png");
            let scale = args.get("scale").and_then(|v| v.as_f64()).unwrap_or(4.0);

            let render_file = if file.to_lowercase().ends_with(".cir") {
                let flxsch = file.trim_end_matches(".cir").to_string() + ".flxsch";

                let flxsch_path = if file.starts_with("/tmp/") {
                    file.replace(".cir", ".flxsch")
                } else {
                    flxsch
                };
                let conv = viora::run_viora_command(
                    &[
                        "netlist-to-schematic".into(),
                        file.into(),
                        "--out".into(),
                        flxsch_path.clone(),
                    ],
                    Some(30),
                )
                .await;
                if conv.ok {
                    flxsch_path
                } else {
                    file.to_string()
                }
            } else {
                file.to_string()
            };
            let mut render_args = vec![
                "schematic-render".into(),
                render_file.clone(),
                out_path.into(),
                "--scale".into(),
                scale.to_string(),
            ];
            if args
                .get("transparent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                render_args.push("--transparent".into());
            }
            let out = viora::run_viora_command(&render_args, Some(60)).await;

            let mut res = json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "out": out_path, "render_file": render_file});
            if out.ok {
                if let Ok(b) = tokio::fs::read(out_path).await {
                    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                    let b64 = BASE64.encode(&b);
                    res["base64"] = json!(b64);
                    res["base64_len"] = json!(b64.len());
                }
            }
            res
        }
        "pcb_render" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out_path = args
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("/tmp/viora_pcb.png");
            let out = viora::run_viora_command(
                &["pcb-render".into(), file.into(), out_path.into()],
                Some(60),
            )
            .await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "out": out_path});
            if out.ok {
                if let Ok(b) = tokio::fs::read(out_path).await {
                    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                    res["base64"] = json!(BASE64.encode(&b));
                }
            }
            res
        }
        "netlist_run" => {
            let vargs = build_netlist_run_args(&args);
            let out = viora::run_viora_command(&vargs, Some(120)).await;
            let mut res = json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data, "argv": vargs});
            // Back-compat: `export_raw` historically accepted an output path
            // (`/tmp/x.raw`). viora's `--export-raw` takes a format, so copy
            // the reported rawPath to the requested path on success.
            if out.ok {
                if let Some(dest) = args.get("export_raw").and_then(|v| v.as_str()) {
                    if export_raw_is_path(dest) {
                        let src = out
                            .data
                            .as_ref()
                            .and_then(|d| d.get("rawPath"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if let Some(src) = src {
                            if src != dest {
                                match std::fs::copy(&src, dest) {
                                    Ok(_) => {
                                        res["rawPath"] = json!(dest);
                                    }
                                    Err(e) => {
                                        res["copy_warning"] = json!(format!(
                                            "sim ok but failed to copy {src} -> {dest}: {e}"
                                        ));
                                    }
                                }
                            } else {
                                res["rawPath"] = json!(dest);
                            }
                        }
                    }
                }
            }
            res
        }
        "netlist_validate" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["netlist-validate".into(), file.into(), "--json".into()],
                Some(60),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "erc" | "drc" | "pcb_validate" => {
            let cmd = match name {
                "erc" => "erc",
                "pcb_validate" => "pcb-validate",
                "drc" => "drc",
                _ => "pcb-validate",
            };
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out =
                viora::run_viora_command(&[cmd.into(), file.into(), "--json".into()], Some(60))
                    .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "symbol_search" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["symbol-search".into(), query.into(), "--json".into()],
                Some(30),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "footprint_list" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["footprint-list".into(), "--json".into()];
            if !query.is_empty() {
                vargs.extend(["--query".into(), query.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "netlist_to_schematic" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = args
                .get("out")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| file.replace(".cir", ".flxsch"));
            let out2 = viora::run_viora_command(
                &[
                    "netlist-to-schematic".into(),
                    file.into(),
                    "--out".into(),
                    out.clone(),
                ],
                Some(30),
            )
            .await;
            json!({"ok": out2.ok, "stdout": out2.stdout, "stderr": out2.stderr, "out": out, "data": out2.data})
        }
        "raw_export" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out_path = args
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("/tmp/viora_raw.json");
            let format = args
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("json");
            let mut vargs = vec![
                "raw-export".into(),
                file.into(),
                "--format".into(),
                format.into(),
                "--json".into(),
                "--out".into(),
                out_path.into(),
            ];
            push_repeatable(&mut vargs, "--signal", str_list(&args, "signal"));
            push_opt_str(&mut vargs, &args, "signal_regex", "--signal-regex");
            push_opt_str(&mut vargs, &args, "range", "--range");
            push_opt_str(&mut vargs, &args, "base_signal", "--base-signal");
            if let Some(n) = args.get("max_points").and_then(|v| v.as_u64()) {
                vargs.push("--max-points".into());
                vargs.push(n.to_string());
            }
            let out = viora::run_viora_command(&vargs, Some(60)).await;

            let mut res = json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data, "out": out_path});
            if out.ok {
                if let Ok(content) = tokio::fs::read_to_string(out_path).await {
                    let preview = if content.len() > 2000 {
                        format!("{}...(truncated {} bytes)", &content[..2000], content.len())
                    } else {
                        content.clone()
                    };
                    res["preview"] = json!(preview);
                    res["size"] = json!(content.len());
                }
            }
            res
        }

        "pcb_compose" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let vargs = build_pcb_compose_args(&args);

            let out = viora::run_viora_command(&vargs, Some(120)).await;
            let mut res = json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data, "argv": vargs});
            // After compose, optionally render for vision feedback (netlist-first workflow invariant)
            if out.ok {
                res["hint"] =
                    json!("run pcb_render for PNG vision feedback and pcb_validate for DRC");
            }
            res
        }
        "pcb_init" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["pcb-init".into(), file.into(), "--json".into()];
            if let Some(s) = args.get("from_schematic").and_then(|v| v.as_str()) {
                vargs.extend(["--from-schematic".into(), s.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "schematic_transform" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["schematic-transform".into(), file.into(), "--json".into()];
            if let Some(s) = args.get("rename_net").and_then(|v| v.as_str()) {
                vargs.extend(["--rename-net".into(), s.into()]);
            }
            if let Some(s) = args.get("prefix_ref").and_then(|v| v.as_str()) {
                vargs.extend(["--prefix-ref".into(), s.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "schematic_validate" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["schematic-validate".into(), file.into(), "--json".into()],
                Some(30),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "pcb_query" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["pcb-query".into(), file.into(), "--json".into()];
            if let Some(q) = args.get("query").and_then(|v| v.as_str()) {
                vargs.extend(["--query".into(), q.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "raw_info" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["raw-info".into(), file.into(), "--json".into()];
            if args
                .get("summary")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                vargs.push("--summary".into());
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "raw_stats" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["raw-stats".into(), file.into(), "--json".into()];
            push_repeatable(&mut vargs, "--signal", str_list(&args, "signal"));
            push_opt_str(&mut vargs, &args, "range", "--range");
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "schematic_netlist" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["schematic-netlist".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "format", "--format");
            push_opt_str(&mut vargs, &args, "analysis", "--analysis");
            push_opt_str(&mut vargs, &args, "step", "--step");
            push_opt_str(&mut vargs, &args, "stop", "--stop");
            push_opt_str(&mut vargs, &args, "out", "--out");
            // Accept both `format` and legacy `f` naming from callers.
            if args.get("format").is_none() {
                push_opt_str(&mut vargs, &args, "f", "--format");
            }
            let out = viora::run_viora_command(&vargs, Some(60)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "schematic_bom" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec!["schematic-bom".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "out", "--out");
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "netlist_compare" => {
            let schematic = args.get("schematic").and_then(|v| v.as_str()).unwrap_or("");
            let netlist = args.get("netlist").and_then(|v| v.as_str()).unwrap_or("");
            if schematic.is_empty() || netlist.is_empty() {
                return json!({"ok": false, "error": "netlist_compare needs schematic + netlist"});
            }
            let mut vargs = vec![
                "netlist-compare".into(),
                schematic.into(),
                netlist.into(),
                "--json".into(),
            ];
            push_opt_str(&mut vargs, &args, "analysis", "--analysis");
            push_opt_str(&mut vargs, &args, "step", "--step");
            push_opt_str(&mut vargs, &args, "stop", "--stop");
            let out = viora::run_viora_command(&vargs, Some(60)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "autofix" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let mut vargs = vec!["autofix".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "out", "--out");
            let out = viora::run_viora_command(&vargs, Some(120)).await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data});
            if out.ok {
                res["hint"] =
                    json!("re-run erc/drc + schematic_render/pcb_render to verify the fix");
            }
            res
        }
        "pcb_sync" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let schematic = args.get("schematic").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() || schematic.is_empty() {
                return json!({"ok": false, "error": "pcb_sync needs file + schematic"});
            }
            let mut vargs = vec![
                "pcb-sync".into(),
                file.into(),
                "--schematic".into(),
                schematic.into(),
                "--json".into(),
            ];
            push_opt_str(&mut vargs, &args, "out", "--out");
            let out = viora::run_viora_command(&vargs, Some(60)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "pcb_export" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let mut vargs = vec!["pcb-export".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "format", "--format");
            if args.get("format").is_none() {
                push_opt_str(&mut vargs, &args, "f", "--format");
            }
            if args
                .get("output")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .is_some()
            {
                push_opt_str(&mut vargs, &args, "output", "--output");
            } else {
                push_opt_str(&mut vargs, &args, "out", "--output");
            }
            let out = viora::run_viora_command(&vargs, Some(120)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "pcb_autoroute" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let mut vargs = vec!["pcb-autoroute".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "out", "--out");
            push_flag(&mut vargs, &args, "ripup", "--ripup");
            if let Some(g) = args.get("grid").and_then(|v| v.as_f64()) {
                vargs.push("--grid".into());
                vargs.push(g.to_string());
            } else if let Some(g) = args.get("grid").and_then(|v| v.as_str()) {
                if !g.is_empty() {
                    vargs.push("--grid".into());
                    vargs.push(g.into());
                }
            }
            let out = viora::run_viora_command(&vargs, Some(180)).await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data});
            if out.ok {
                res["hint"] = json!("run pcb_render for vision feedback and pcb_validate for DRC");
            }
            res
        }
        "pcb_cleanup" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let mut vargs = vec!["pcb-cleanup".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "out", "--out");
            let out = viora::run_viora_command(&vargs, Some(60)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "pcb_netlist" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["pcb-netlist".into(), file.into(), "--json".into()],
                Some(30),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "symbol_validate" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out = viora::run_viora_command(
                &["symbol-validate".into(), file.into(), "--json".into()],
                Some(30),
            )
            .await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "footprint_import" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            if file.is_empty() {
                return json!({"ok": false, "error": "missing file"});
            }
            let mut vargs = vec!["footprint-import".into(), file.into(), "--json".into()];
            push_opt_str(&mut vargs, &args, "out", "--out");
            push_flag(&mut vargs, &args, "render", "--render");
            if let Some(n) = args.get("limit").and_then(|v| v.as_u64()) {
                vargs.push("--limit".into());
                vargs.push(n.to_string());
            }
            let out = viora::run_viora_command(&vargs, Some(60)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "symbol_list" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            let vargs = vec!["symbol-list".into(), path.into(), "--json".into()];
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "symbol_render" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out_path = args
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("/tmp/viora_symbol.png");
            let out = viora::run_viora_command(
                &["symbol-render".into(), file.into(), out_path.into()],
                Some(30),
            )
            .await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "out": out_path});
            if out.ok {
                if let Ok(b) = tokio::fs::read(out_path).await {
                    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                    res["base64"] = json!(BASE64.encode(&b));
                }
            }
            res
        }
        "footprint_render" => {
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let out_path = args
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("/tmp/viora_fp.png");
            let out = viora::run_viora_command(
                &["footprint-render".into(), file.into(), out_path.into()],
                Some(30),
            )
            .await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "out": out_path});
            if out.ok {
                if let Ok(b) = tokio::fs::read(out_path).await {
                    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                    res["base64"] = json!(BASE64.encode(&b));
                }
            }
            res
        }
        "flux" => {
            let cmd = args
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("eval");
            let mut vargs = vec!["flux".into(), cmd.into(), "--json".into()];
            if let Some(f) = args.get("file").and_then(|v| v.as_str()) {
                vargs.extend(["--file".into(), f.into()]);
            }
            if let Some(s) = args.get("script").and_then(|v| v.as_str()) {
                vargs.extend(["--script".into(), s.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
        }
        "task" => {
            let prompt = args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let kind_str = args
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("explore");
            let model = args
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // Parent context injected by the loop (see loop/mod.rs): depth
            // for tracker labels + recursion guard, model as offline
            // fallback behind explicit `model:` and SUBAGENT_MODEL env,
            // session_id tagging the spawning chat for per-chat /agents.
            let parent_depth = args
                .get("parent_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let parent_model = args
                .get("parent_model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let parent_session = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if prompt.is_empty() {
                return json!({"ok": false, "error": "missing prompt for task"});
            }
            let kind = match crate::subagent::SubagentKind::parse(kind_str) {
                Ok(k) => k,
                Err(e) => return json!({"ok": false, "error": format!("{e:#}")}),
            };
            let parent = crate::subagent::pool::ParentCtx {
                depth: parent_depth,
                model: parent_model,
                session: parent_session,
            };
            if args
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                // Detached: the run id returns at once so the turn
                // continues; completion surfaces via take_completions
                // (TUI notice + follow-up turn, like bg bash tasks).
                // Needs a live session to collect the result (TUI/serve).
                let run_id = crate::subagent::SubagentPool::spawn_background_full(
                    kind, prompt, model, parent,
                );
                return json!({"ok": true, "background": true, "run_id": run_id, "kind": kind_str});
            }
            let pool = crate::subagent::SubagentPool::new(4);
            match pool.spawn_one_full(kind, prompt, model, None, parent).await {
                Ok(result) => json!({"ok": true, "result": result, "kind": kind_str}),
                Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
            }
        }
        "skill" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                let skills = crate::skills::list_skills();
                let list: Vec<Value> = skills
                    .iter()
                    .map(|s| {
                        json!({
                            "name": s.name,
                            "description": s.description,
                            "triggers": s.triggers,
                            "path": s.path.to_string_lossy()
                        })
                    })
                    .collect();
                return json!({"ok": true, "skills": list});
            }
            if let Some(skill) = crate::skills::get_skill(&name) {
                json!({
                    "ok": true,
                    "name": skill.name,
                    "description": skill.description,
                    "triggers": skill.triggers,
                    "path": skill.path.to_string_lossy(),
                    "content": skill.content
                })
            } else {
                let p = std::path::Path::new(&name);
                if p.exists() {
                    if let Ok(content) = std::fs::read_to_string(p) {
                        return json!({"ok": true, "path": name, "content": content});
                    }
                }
                json!({"ok": false, "error": format!("skill not found: {}", name)})
            }
        }

        "question" => {
            let id = args
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("question")
                .to_string();
            question::ask_question(&id, args).await
        }
        "todowrite" => todo::run_todos(args).await,
        "edit" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let res = fs::edit_file(args.clone()).await;
            if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Some(diag) = after_write_hook(&path).await {
                    let mut merged = res;
                    merged["diagnostics"] = diag;
                    return merged;
                }
            }
            res
        }
        "apply_patch" => {
            let patch_text = args.get("patch").and_then(|v| v.as_str()).unwrap_or("");
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            patch::apply_patch_text(patch_text, session_id).await
        }
        "webfetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let max_chars = args
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            web::webfetch(url, max_chars).await
        }
        "websearch" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let count = args
                .get("count")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            web::websearch(query, count).await
        }
        "browser_screenshot" => browser::browser_screenshot(args).await,
        "browser_dom" => browser::browser_dom(args).await,
        "browser_pdf" => browser::browser_pdf(args).await,
        "browser_open" => browser::browser_open(args).await,
        "dev_serve" => browser::dev_serve(args).await,
        "adb_devices" => android::adb_devices(args).await,
        "adb_shell" => android::adb_shell(args).await,
        "adb_install" => android::adb_install(args).await,
        "adb_logcat" => android::adb_logcat(args).await,
        "adb_push" => android::adb_push(args).await,
        "adb_pull" => android::adb_pull(args).await,
        "adb_screenshot" => android::adb_screenshot(args).await,
        "emulator" => android::emulator_ctl(args).await,
        "gradle" => android::gradle(args).await,
        _ => json!({"ok": false, "error": format!("unknown tool: {}", name)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn schemas_accept_valid_calls() {
        let r = execute_tool(
            "question",
            json!({"questions": [{"question": "Q?", "options": [{"label": "A"}]}]}),
        )
        .await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("best judgment"),
            "{r}"
        );

        let r = execute_tool("websearch", json!({})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("missing required field 'query'"),
            "{r}"
        );

        let r = execute_tool("webfetch", json!({"url": "ftp://x/y"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("refused non-http"),
            "{r}"
        );

        let r = execute_tool("edit", json!({"path": "x"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("missing required field"),
            "{r}"
        );

        let r = execute_tool(
            "todowrite",
            json!({"todos": [{"content": "x", "status": "done"}]}),
        )
        .await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("schema validation"),
            "{r}"
        );

        let r = execute_tool("apply_patch", json!({})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("missing required field 'patch'"),
            "{r}"
        );
    }

    #[tokio::test]
    async fn edit_and_patch_end_to_end() {
        let dir = std::env::temp_dir().join(format!("vh-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("f.txt").to_string_lossy().to_string();
        std::fs::write(dir.join("f.txt"), "hello world\n").unwrap();

        let r = execute_tool(
            "edit",
            json!({"path": f, "old_string": "world", "new_string": "there"}),
        )
        .await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "hello there\n"
        );

        let patch = format!(
            "*** Begin Patch\n*** Update File: {f}\n@@\n-hello there\n+hi there\n*** End Patch"
        );
        let r = execute_tool("apply_patch", json!({"patch": patch})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(
            std::fs::read_to_string(dir.join("f.txt")).unwrap(),
            "hi there\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn dispatch_offline_arms() {
        let r = execute_tool("no_such_tool", json!({})).await;
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("unknown tool"), "{r}");

        let r = execute_tool("task", json!({"prompt": "x", "kind": "explor"})).await;
        assert_eq!(r["ok"], false, "typo'd kind must fail closed, {r}");
        let err = r["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("kind") || err.contains("schema"),
            "error names the kind problem: {r}"
        );

        let r = execute_tool("bash", json!({})).await;
        assert_eq!(r["ok"], false, "schema requires command");

        let r = execute_tool("bash", json!({"command": ""})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("missing command"),
            "{r}"
        );

        let r = execute_tool("bash", json!({"command": "rm -rf /"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("denied by policy"),
            "{r}"
        );

        let r = execute_tool("viora", json!({"cmd": ""})).await;
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("missing cmd"), "{r}");

        let r = execute_tool("read", json!({})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("missing required field 'path'"),
            "{r}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn task_background_returns_id_then_drains() {
        use crate::subagent::tracker;
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_SUBAGENT_MODEL").ok();
        std::env::set_var("VIORAHARNESS_SUBAGENT_MODEL", "test/bogus-model-xyz");
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let prompt = format!("bg-probe-{n}-{}", std::process::id());

        let r = execute_tool(
            "task",
            json!({"prompt": prompt, "kind": "explore", "background": true}),
        )
        .await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["background"], true, "{r}");
        let run_id = r["run_id"].as_str().unwrap_or_default().to_string();
        assert!(!run_id.is_empty(), "{r}");
        assert!(
            tracker::active_runs().iter().any(|x| x.id == run_id),
            "detached run is live at once"
        );
        // Finish deterministically: cancel wins the race against the
        // bogus-model failure; either order must drain exactly once.
        let _ = tracker::cancel_run(&run_id);
        let start = std::time::Instant::now();
        let mut drained = false;
        while start.elapsed().as_secs() < 15 {
            if tracker::take_completions().iter().any(|x| x.id == run_id) {
                drained = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(drained, "completion surfaced");
        assert!(
            tracker::take_completions().iter().all(|x| x.id != run_id),
            "drains once"
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_MODEL", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_MODEL"),
        }
    }

    #[test]
    fn netlist_run_arg_builder_covers_full_flags() {
        let args = json!({
            "file": "a.cir",
            "analysis": "tran",
            "step": "1u",
            "stop": "1m",
            "compat": true,
            "robust": true,
            "stats": true,
            "measure": ["V(out)_avg > 0.5", "V(out)_rms < 6"],
            "assert": "V(out)_avg > 0.5",
            "measure_format": "json",
            "range": "0:0.001",
            "signal": "V(out)",
            "max_points": 1000,
            "base_signal": "V(out)",
            "export_raw": "/tmp/a.json",
            "timeout": "60s"
        });
        let v = build_netlist_run_args(&args);
        for flag in [
            "--analysis",
            "--step",
            "--stop",
            "--compat",
            "--robust",
            "--stats",
            "--measure",
            "--assert",
            "--measure-format",
            "--range",
            "--signal",
            "--max-points",
            "--base-signal",
            "--export-raw",
            "--timeout",
        ] {
            assert!(v.contains(&flag.to_string()), "missing {flag}: {v:?}");
        }
        assert_eq!(v.iter().filter(|s| s.as_str() == "--measure").count(), 2);
        assert_eq!(v.iter().filter(|s| s.as_str() == "--signal").count(), 1);
        // Legacy path normalizes to its inferred format.
        let pos = v
            .iter()
            .position(|s| s == "--export-raw")
            .expect("--export-raw");
        assert_eq!(v[pos + 1], "json", "path /tmp/a.json infers json: {v:?}");
        assert_eq!(normalize_export_raw_format("csv"), "csv");
        assert_eq!(normalize_export_raw_format("/tmp/x.csv"), "csv");
        assert_eq!(normalize_export_raw_format("/tmp/x.parquet"), "parquet");
        assert!(export_raw_is_path("/tmp/a.json"));
        assert!(!export_raw_is_path("json"));
        // Back-compat: old boolean measure/assert must not emit bare flags.
        let legacy = build_netlist_run_args(&json!({"file": "a.cir"}));
        assert!(!legacy.contains(&"--measure".to_string()));
        assert!(!legacy.contains(&"--assert".to_string()));
    }

    #[test]
    fn pcb_compose_arg_builder_covers_all_injects() {
        let args = json!({
            "file": "b.pcb",
            "add_component": "footprint=R_0603,x=10,y=10",
            "add_trace": "x1=0,y1=0,x2=1,y2=1,width=0.2,layer=top,net=GND",
            "add_via": "x=1,y=1,diameter=0.6,drill=0.3,net=GND",
            "delete_item": "id=3",
            "shrink_outline": "margin=1",
            "add_netclass": "name=PWR,width=0.5,clearance=0.2",
            "assign_net": "net=GND,class=PWR",
            "add_pour": "layer=top,net=GND,clearance=0.3",
            "auto_route": true,
            "allow_diagonals": true,
            "route_layers": "both",
            "out": "/tmp/b.pcb"
        });
        let v = build_pcb_compose_args(&args);
        for flag in [
            "--add-component",
            "--add-trace",
            "--add-via",
            "--delete-item",
            "--shrink-outline",
            "--add-netclass",
            "--assign-net",
            "--add-pour",
            "--auto-route",
            "--allow-diagonals",
            "--route-layers",
            "--out",
            "--json",
        ] {
            assert!(v.contains(&flag.to_string()), "missing {flag}: {v:?}");
        }
    }

    #[test]
    fn str_list_accepts_string_array_and_absent() {
        assert_eq!(str_list(&json!({"m": "a"}), "m"), vec!["a".to_string()]);
        assert_eq!(
            str_list(&json!({"m": ["a", "b"]}), "m"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(str_list(&json!({}), "m").is_empty());
        assert!(str_list(&json!({"m": ""}), "m").is_empty());
    }

    #[tokio::test]
    async fn eda_schemas_reject_missing_required() {
        for (tool, args) in [
            ("netlist_run", json!({})),
            ("drc", json!({})),
            ("pcb_sync", json!({"file": "a.pcb"})),
            ("netlist_compare", json!({"schematic": "a.flxsch"})),
            ("pcb_export", json!({})),
            ("autofix", json!({})),
            ("schematic_netlist", json!({})),
            ("footprint_import", json!({})),
        ] {
            let r = execute_tool(tool, args).await;
            assert_eq!(r["ok"], false, "{tool} should fail closed");
            assert!(
                r["error"]
                    .as_str()
                    .unwrap()
                    .contains("missing required field"),
                "{tool}: {r}"
            );
        }
        // New schemas accept their happy-path shapes (dispatch may still fail
        // without viora, but must not fail schema validation).
        let r = execute_tool(
            "netlist_run",
            json!({"file": "a.cir", "measure": ["V(out)_avg > 1"], "compat": true}),
        )
        .await;
        assert!(
            !r.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .contains("schema validation"),
            "netlist_run happy shape rejected: {r}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn edit_snapshot_uses_db_seq_not_zero() {
        // Snapshots must carry the session's DB seq so rewind targets line up.
        use crate::session::SessionStore;
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_DB").ok();
        let dir = std::env::temp_dir().join(format!("vh-seq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.db");
        std::env::set_var("VIORAHARNESS_DB", &db);
        let f = dir.join("f.txt").to_string_lossy().to_string();
        std::fs::write(dir.join("f.txt"), "hello world\n").unwrap();
        {
            let store = SessionStore::new(db.to_str().unwrap()).unwrap();
            store.create_session("sess-seq", "m", None).unwrap();
            store.append_message("sess-seq", "user", "fix it").unwrap();
            store
                .append_message("sess-seq", "assistant", "on it")
                .unwrap();
        }
        let r = execute_tool(
            "edit",
            json!({"path": f, "old_string": "world", "new_string": "there", "session_id": "sess-seq"}),
        )
        .await;
        assert_eq!(r["ok"], true, "{r}");
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        let snaps = store.get_snapshots("sess-seq").unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].0, 2, "snapshot tagged with DB seq, not 0");
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
