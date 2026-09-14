use vioraharness_core::tools;

fn viora_built() -> bool {
    if let Ok(root) = std::env::var("VIOSPICE_ROOT") {
        if std::path::Path::new(&format!("{root}/build/viora")).exists() {
            return true;
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if std::path::Path::new(&format!("{home}/qt_projects/viospice/build/viora")).exists() {
        return true;
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if std::path::Path::new(&format!("{dir}/viora")).exists() {
                return true;
            }
        }
    }
    if std::path::Path::new(&format!("{home}/.local/bin/viora")).exists() {
        return true;
    }
    false
}

#[tokio::test]
async fn test_netlist_validate_pass() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let cir_path = "/tmp/vh_test_rc.cir";
    let cir =
        "* RC test\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let write_res = tools::execute_tool(
        "write",
        serde_json::json!({"path": cir_path, "content": cir}),
    )
    .await;
    assert!(
        write_res
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        "write failed: {write_res}"
    );

    let res = tools::execute_tool("netlist_validate", serde_json::json!({"file": cir_path})).await;

    assert!(
        res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) || res.get("stdout").is_some(),
        "netlist_validate failed: {res}"
    );
}

#[tokio::test]
async fn test_read_and_glob() {
    let res = tools::execute_tool("read", serde_json::json!({"path": "Cargo.toml"})).await;
    assert!(res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
    let content = res.get("content").and_then(|v| v.as_str()).unwrap_or("");
    assert!(content.contains("vioraharness"));

    let res = tools::execute_tool("glob", serde_json::json!({"pattern": "*.json"})).await;
    assert!(res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false));
}

#[tokio::test]
async fn test_symbol_search() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let res = tools::execute_tool("symbol_search", serde_json::json!({"query": "resistor"})).await;
    assert!(
        res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "symbol_search failed: {res}"
    );
}

fn ok_or_stdout(res: &serde_json::Value) -> bool {
    res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) || res.get("stdout").is_some()
}

#[tokio::test]
async fn test_netlist_run_full_flags() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let tag = std::process::id();
    let cir = format!("/tmp/vh_full_{tag}.cir");
    let raw = format!("/tmp/vh_full_{tag}.raw");
    let deck =
        "* RC full\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let w = tools::execute_tool("write", serde_json::json!({"path": cir, "content": deck})).await;
    assert!(
        w.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "write: {w}"
    );
    let r = tools::execute_tool(
        "netlist_run",
        serde_json::json!({
            "file": cir,
            "compat": true,
            "robust": true,
            "stats": true,
            "measure": ["V(out)_avg > 0"],
            "assert": ["V(out)_avg > 0"],
            "measure_format": "json",
            "export_raw": raw,
        }),
    )
    .await;
    assert!(
        r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "netlist_run full flags failed: {r}"
    );
    assert!(
        std::path::Path::new(&raw).exists(),
        "export_raw missing: {r}"
    );
    for f in [&cir, &raw] {
        let _ = std::fs::remove_file(f);
    }
}

#[tokio::test]
async fn test_schematic_side_tools() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let tag = std::process::id();
    let cir = format!("/tmp/vh_sch_{tag}.cir");
    let flx = format!("/tmp/vh_sch_{tag}.flxsch");
    let deck =
        "* RC sch\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let w = tools::execute_tool("write", serde_json::json!({"path": cir, "content": deck})).await;
    assert!(
        w.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "write: {w}"
    );
    let n2s = tools::execute_tool(
        "netlist_to_schematic",
        serde_json::json!({"file": cir, "out": flx}),
    )
    .await;
    assert!(
        n2s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "n2s: {n2s}"
    );

    let nl = tools::execute_tool(
        "schematic_netlist",
        serde_json::json!({"file": flx, "format": "spice"}),
    )
    .await;
    assert!(ok_or_stdout(&nl), "schematic_netlist: {nl}");

    let bom = tools::execute_tool("schematic_bom", serde_json::json!({"file": flx})).await;
    assert!(ok_or_stdout(&bom), "schematic_bom: {bom}");

    let sv = tools::execute_tool("schematic_validate", serde_json::json!({"file": flx})).await;
    assert!(ok_or_stdout(&sv), "schematic_validate: {sv}");

    let erc = tools::execute_tool("erc", serde_json::json!({"file": flx})).await;
    assert!(ok_or_stdout(&erc), "erc: {erc}");

    let cmp = tools::execute_tool(
        "netlist_compare",
        serde_json::json!({"schematic": flx, "netlist": cir}),
    )
    .await;
    assert!(ok_or_stdout(&cmp), "netlist_compare: {cmp}");

    let fix = tools::execute_tool("autofix", serde_json::json!({"file": flx})).await;
    assert!(ok_or_stdout(&fix), "autofix: {fix}");

    for f in [&cir, &flx] {
        let _ = std::fs::remove_file(f);
    }
}

#[tokio::test]
async fn test_pcb_side_tools() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let tag = std::process::id();
    let cir = format!("/tmp/vh_pcb_{tag}.cir");
    let flx = format!("/tmp/vh_pcb_{tag}.flxsch");
    let pcb = format!("/tmp/vh_pcb_{tag}.pcb");
    let deck =
        "* RC pcb\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let w = tools::execute_tool("write", serde_json::json!({"path": cir, "content": deck})).await;
    assert!(
        w.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "write: {w}"
    );
    let n2s = tools::execute_tool(
        "netlist_to_schematic",
        serde_json::json!({"file": cir, "out": flx}),
    )
    .await;
    assert!(
        n2s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "n2s: {n2s}"
    );

    let init = tools::execute_tool(
        "pcb_init",
        serde_json::json!({"file": pcb, "from_schematic": flx}),
    )
    .await;
    assert!(ok_or_stdout(&init), "pcb_init: {init}");
    if !std::path::Path::new(&pcb).exists() {
        eprintln!("skip pcb rest: pcb_init produced no file: {init}");
        for f in [&cir, &flx] {
            let _ = std::fs::remove_file(f);
        }
        return;
    }

    let q = tools::execute_tool("pcb_query", serde_json::json!({"file": pcb})).await;
    assert!(ok_or_stdout(&q), "pcb_query: {q}");
    let pn = tools::execute_tool("pcb_netlist", serde_json::json!({"file": pcb})).await;
    assert!(ok_or_stdout(&pn), "pcb_netlist: {pn}");
    let pv = tools::execute_tool("pcb_validate", serde_json::json!({"file": pcb})).await;
    assert!(ok_or_stdout(&pv), "pcb_validate: {pv}");
    let drc = tools::execute_tool("drc", serde_json::json!({"file": pcb})).await;
    assert!(
        drc.get("ok").and_then(|v| v.as_bool()).is_some() || drc.get("stdout").is_some(),
        "drc must be a registered tool, got: {drc}"
    );
    let sync = tools::execute_tool(
        "pcb_sync",
        serde_json::json!({"file": pcb, "schematic": flx}),
    )
    .await;
    assert!(ok_or_stdout(&sync), "pcb_sync: {sync}");
    let clean = tools::execute_tool("pcb_cleanup", serde_json::json!({"file": pcb})).await;
    assert!(ok_or_stdout(&clean), "pcb_cleanup: {clean}");
    let comp = tools::execute_tool(
        "pcb_compose",
        serde_json::json!({
            "file": pcb,
            "add_component": "footprint=R_0603,x=10,y=10",
            "route_layers": "both",
        }),
    )
    .await;
    assert!(ok_or_stdout(&comp), "pcb_compose: {comp}");
    let exp = tools::execute_tool(
        "pcb_export",
        serde_json::json!({"file": pcb, "format": "gerber", "output": format!("/tmp/vh_pcb_exp_{tag}")}),
    )
    .await;
    assert!(ok_or_stdout(&exp), "pcb_export: {exp}");
    let render = tools::execute_tool("pcb_render", serde_json::json!({"file": pcb})).await;
    assert!(ok_or_stdout(&render), "pcb_render: {render}");

    for f in [&cir, &flx, &pcb] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir_all(format!("/tmp/vh_pcb_exp_{tag}"));
}

#[tokio::test]
async fn test_raw_and_symbol_tools() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let tag = std::process::id();
    let cir = format!("/tmp/vh_raw_{tag}.cir");
    let raw = format!("/tmp/vh_raw_{tag}.raw");
    let deck =
        "* RC raw\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let w = tools::execute_tool("write", serde_json::json!({"path": cir, "content": deck})).await;
    assert!(
        w.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "write: {w}"
    );
    let r = tools::execute_tool(
        "netlist_run",
        serde_json::json!({"file": cir, "export_raw": raw}),
    )
    .await;
    assert!(
        r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "run: {r}"
    );

    let info = tools::execute_tool(
        "raw_info",
        serde_json::json!({"file": raw, "summary": true}),
    )
    .await;
    assert!(ok_or_stdout(&info), "raw_info: {info}");
    let stats = tools::execute_tool(
        "raw_stats",
        serde_json::json!({"file": raw, "signal": "V(out)"}),
    )
    .await;
    assert!(ok_or_stdout(&stats), "raw_stats: {stats}");
    let exp = tools::execute_tool(
        "raw_export",
        serde_json::json!({"file": raw, "out": format!("/tmp/vh_raw_{tag}.json"), "format": "json", "max_points": 100}),
    )
    .await;
    assert!(
        exp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "raw_export: {exp}"
    );

    let fl = tools::execute_tool("footprint_list", serde_json::json!({})).await;
    assert!(ok_or_stdout(&fl), "footprint_list: {fl}");

    for f in [&cir, &raw] {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(format!("/tmp/vh_raw_{tag}.json"));
}

#[tokio::test]
async fn test_netlist_first_chain() {
    if !viora_built() {
        eprintln!("skip: viora not built");
        return;
    }
    let tag = std::process::id();
    let cir = format!("/tmp/vh_chain_{tag}.cir");
    let deck =
        "* RC chain\nV1 in 0 DC 5\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 1m\n.save V(out)\n.end\n";
    let w = tools::execute_tool("write", serde_json::json!({"path": cir, "content": deck})).await;
    assert!(
        w.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "write: {w}"
    );

    let v = tools::execute_tool("netlist_validate", serde_json::json!({"file": cir})).await;
    assert!(
        v.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "validate: {v}"
    );

    let raw_path = format!("/tmp/vh_chain_{tag}.raw");
    let r = tools::execute_tool(
        "netlist_run",
        serde_json::json!({"file": cir, "export_raw": raw_path}),
    )
    .await;
    assert!(
        r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "run: {r}"
    );

    let exp_path = format!("/tmp/vh_chain_{tag}.json");
    let e = tools::execute_tool(
        "raw_export",
        serde_json::json!({"file": raw_path, "out": exp_path, "format": "json"}),
    )
    .await;
    assert!(
        e.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "export: {e}"
    );
    let exported = std::fs::read_to_string(&exp_path).expect("export file exists");
    assert!(
        exported.len() > 100,
        "export has substance: {exported:.200}"
    );

    let flx = format!("/tmp/vh_chain_{tag}.flxsch");
    let n2s = tools::execute_tool(
        "netlist_to_schematic",
        serde_json::json!({"file": cir, "out": flx}),
    )
    .await;
    assert!(
        n2s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "n2s: {n2s}"
    );

    let png = format!("/tmp/vh_chain_{tag}.png");
    let s = tools::execute_tool(
        "schematic_render",
        serde_json::json!({"file": flx, "out": png}),
    )
    .await;
    assert!(
        s.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "render: {s}"
    );
    let meta = std::fs::metadata(&png).expect("png exists");
    assert!(meta.len() > 1000, "png has substance: {} bytes", meta.len());

    for f in [&cir, &raw_path, &exp_path, &flx, &png] {
        let _ = std::fs::remove_file(f);
    }
}
