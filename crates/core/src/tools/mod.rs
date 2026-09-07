pub mod bash;
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
            let out = viora::run_viora_command(
                &[
                    "schematic-render".into(),
                    render_file.clone(),
                    out_path.into(),
                    "--scale".into(),
                    scale.to_string(),
                ],
                Some(60),
            )
            .await;

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
            let file = args.get("file").and_then(|v| v.as_str()).unwrap_or("");
            let mut vargs = vec![
                "netlist-run".to_string(),
                file.to_string(),
                "--json".to_string(),
            ];
            if let Some(a) = args.get("analysis").and_then(|v| v.as_str()) {
                vargs.extend(["--analysis".into(), a.into()]);
            }
            if let Some(s) = args.get("step").and_then(|v| v.as_str()) {
                vargs.extend(["--step".into(), s.into()]);
            }
            if let Some(s) = args.get("stop").and_then(|v| v.as_str()) {
                vargs.extend(["--stop".into(), s.into()]);
            }

            if let Some(v) = args.get("measure").and_then(|v| v.as_bool()) {
                if v {
                    vargs.push("--measure".into());
                }
            }
            if let Some(v) = args.get("assert").and_then(|v| v.as_bool()) {
                if v {
                    vargs.push("--assert".into());
                }
            }
            if let Some(s) = args.get("export_raw").and_then(|v| v.as_str()) {
                vargs.extend(["--export-raw".into(), s.into()]);
            }
            if let Some(s) = args.get("range").and_then(|v| v.as_str()) {
                vargs.extend(["--range".into(), s.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(120)).await;
            json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data})
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
            let out = viora::run_viora_command(
                &[
                    "raw-export".into(),
                    file.into(),
                    "--format".into(),
                    format.into(),
                    "--json".into(),
                    "--out".into(),
                    out_path.into(),
                ],
                Some(60),
            )
            .await;

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
            let mut vargs = vec!["pcb-compose".into(), file.into()];
            if let Some(s) = args.get("add_component").and_then(|v| v.as_str()) {
                vargs.extend(["--add-component".into(), s.into()]);
            }
            if let Some(s) = args.get("add_trace").and_then(|v| v.as_str()) {
                vargs.extend(["--add-trace".into(), s.into()]);
            }
            if let Some(s) = args.get("add_via").and_then(|v| v.as_str()) {
                vargs.extend(["--add-via".into(), s.into()]);
            }
            if args
                .get("auto_route")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                vargs.push("--auto-route".into());
            }
            if let Some(s) = args.get("route_layers").and_then(|v| v.as_str()) {
                vargs.extend(["--route-layers".into(), s.into()]);
            }
            if let Some(s) = args.get("out").and_then(|v| v.as_str()) {
                vargs.extend(["--out".into(), s.into()]);
            }
            vargs.push("--json".into());

            let out = viora::run_viora_command(&vargs, Some(120)).await;
            let mut res =
                json!({"ok": out.ok, "stdout": out.stdout, "stderr": out.stderr, "data": out.data});
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
            if let Some(s) = args.get("signal").and_then(|v| v.as_str()) {
                vargs.extend(["--signal".into(), s.into()]);
            }
            let out = viora::run_viora_command(&vargs, Some(30)).await;
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
            if prompt.is_empty() {
                return json!({"ok": false, "error": "missing prompt for task"});
            }
            let kind = match kind_str {
                "explore" => crate::subagent::SubagentKind::Explore,
                "planner" => crate::subagent::SubagentKind::Planner,
                "coder" => crate::subagent::SubagentKind::Coder,
                "reviewer" => crate::subagent::SubagentKind::Reviewer,
                _ => crate::subagent::SubagentKind::Explore,
            };
            let pool = crate::subagent::SubagentPool::new(4);
            match pool.spawn_one(kind, prompt, model).await {
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
}
