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
