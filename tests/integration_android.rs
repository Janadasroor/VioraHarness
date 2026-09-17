// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use vioraharness_core::tools;

// Skip-gate: true when an `adb` binary is reachable via ADB_BIN,
// ANDROID_HOME/ANDROID_SDK_ROOT, or PATH (same resolution as the tools).
fn adb_available() -> bool {
    if let Ok(bin) = std::env::var("ADB_BIN") {
        if !bin.trim().is_empty() && std::path::Path::new(&bin).exists() {
            return true;
        }
    }
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(root) = std::env::var(var) {
            if !root.trim().is_empty()
                && std::path::Path::new(&format!(
                    "{}/platform-tools/adb",
                    root.trim_end_matches('/')
                ))
                .exists()
            {
                return true;
            }
        }
    }
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            if std::path::Path::new(&format!("{dir}/adb")).exists() {
                return true;
            }
        }
    }
    false
}

fn ok(res: &serde_json::Value) -> bool {
    res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
}

async fn need_device() -> Option<String> {
    if !adb_available() {
        eprintln!("skip: no adb");
        return None;
    }
    let res = tools::execute_tool("adb_devices", serde_json::json!({})).await;
    if !ok(&res) {
        eprintln!("skip: adb_devices failed: {res}");
        return None;
    }
    let devs = res
        .get("devices")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if devs.is_empty() {
        eprintln!("skip: no devices attached");
        return None;
    }
    devs[0]
        .get("serial")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[tokio::test]
async fn test_adb_devices_lists_attached() {
    if !adb_available() {
        eprintln!("skip: no adb");
        return;
    }
    let res = tools::execute_tool("adb_devices", serde_json::json!({})).await;
    assert!(ok(&res), "adb_devices failed: {res}");
    let devs = res
        .get("devices")
        .and_then(|v| v.as_array())
        .expect("devices array");
    assert!(!devs.is_empty(), "expected ≥1 device: {res}");
    for d in devs {
        assert!(!d
            .get("serial")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty());
        assert!(!d
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty());
    }
}

#[tokio::test]
async fn test_adb_shell_roundtrip() {
    let Some(serial) = need_device().await else {
        return;
    };
    let tag = std::process::id();
    let probe = format!("vh-shell-{tag}");
    let res = tools::execute_tool(
        "adb_shell",
        serde_json::json!({"serial": serial, "command": format!("echo {probe}")}),
    )
    .await;
    assert!(ok(&res), "adb_shell echo failed: {res}");
    assert!(res
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains(&probe));
    assert_eq!(
        res.get("serial").and_then(|v| v.as_str()),
        Some(serial.as_str())
    );

    let res = tools::execute_tool(
        "adb_shell",
        serde_json::json!({"serial": serial, "command": "getprop ro.build.version.sdk"}),
    )
    .await;
    assert!(ok(&res), "getprop failed: {res}");
    let sdk: u32 = res
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .parse()
        .unwrap_or(0);
    assert!(sdk >= 21, "sane SDK level: {res}");
}

#[tokio::test]
async fn test_adb_shell_auto_serial() {
    if need_device().await.is_none() {
        return;
    }
    // No serial passed: single-device auto-select covers it.
    let res =
        tools::execute_tool("adb_shell", serde_json::json!({"command": "echo auto-ok"})).await;
    assert!(ok(&res), "auto-serial shell failed: {res}");
    assert!(res.get("serial").and_then(|v| v.as_str()).is_some());
}

#[tokio::test]
async fn test_adb_logcat_dump() {
    let Some(serial) = need_device().await else {
        return;
    };
    let res = tools::execute_tool(
        "adb_logcat",
        serde_json::json!({"serial": serial, "lines": 5}),
    )
    .await;
    assert!(ok(&res), "logcat dump failed: {res}");
    assert!(!res
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty());
}

#[tokio::test]
async fn test_adb_push_pull_roundtrip() {
    let Some(serial) = need_device().await else {
        return;
    };
    let tag = std::process::id();
    let content = format!("vh-probe-{tag}");
    let host = format!("/tmp/vh_adb_probe_{tag}.txt");
    let remote = format!("/data/local/tmp/vh_adb_probe_{tag}.txt");
    let back = format!("/tmp/vh_adb_back_{tag}.txt");
    std::fs::write(&host, &content).unwrap();

    let p = tools::execute_tool(
        "adb_push",
        serde_json::json!({"serial": serial, "src": host, "dst": remote}),
    )
    .await;
    assert!(ok(&p), "push failed: {p}");

    let c = tools::execute_tool(
        "adb_shell",
        serde_json::json!({"serial": serial, "command": format!("cat {remote}")}),
    )
    .await;
    assert!(c
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains(&content));

    let q = tools::execute_tool(
        "adb_pull",
        serde_json::json!({"serial": serial, "src": remote, "dst": back}),
    )
    .await;
    assert!(ok(&q), "pull failed: {q}");
    assert_eq!(std::fs::read_to_string(&back).unwrap(), content);

    let _ = tools::execute_tool(
        "adb_shell",
        serde_json::json!({"serial": serial, "command": format!("rm {remote}")}),
    )
    .await;
    let _ = std::fs::remove_file(&host);
    let _ = std::fs::remove_file(&back);
}

#[tokio::test]
async fn test_adb_screenshot_is_real_png() {
    let Some(serial) = need_device().await else {
        return;
    };
    let tag = std::process::id();
    let out = format!("/tmp/vh_shot_{tag}.png");
    let res = tools::execute_tool(
        "adb_screenshot",
        serde_json::json!({"serial": serial, "out": out}),
    )
    .await;
    assert!(ok(&res), "adb_screenshot failed: {res}");
    assert_eq!(
        res.get("serial").and_then(|v| v.as_str()),
        Some(serial.as_str())
    );
    let bytes = std::fs::read(&out).expect("screenshot file exists");
    assert!(
        bytes.len() > 10_000,
        "png has substance: {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG magic");
    assert!(
        res.get("base64")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .len()
            > 10_000
    );
    let _ = std::fs::remove_file(&out);
}

#[tokio::test]
async fn test_emulator_list() {
    if !adb_available() {
        // emulator lives next to adb; without any SDK hint there is nothing to list with.
        let res = tools::execute_tool("emulator", serde_json::json!({"action": "list"})).await;
        eprintln!("no SDK; emulator list -> {res}");
        return;
    }
    let res = tools::execute_tool("emulator", serde_json::json!({"action": "list"})).await;
    assert!(ok(&res), "emulator list failed: {res}");
    assert!(
        res.get("avds").and_then(|v| v.as_array()).is_some(),
        "{res}"
    );
}

#[tokio::test]
async fn test_gradle_version_runs_anywhere() {
    // `gradle --version` needs no project: proves runner resolution +
    // foreground execution end to end.
    let res = tools::execute_tool(
        "gradle",
        serde_json::json!({"dir": "/tmp", "tasks": ["--version"]}),
    )
    .await;
    if !ok(&res) {
        eprintln!(
            "skip: no gradle runner ({})",
            res.get("error").unwrap_or(&serde_json::Value::Null)
        );
        return;
    }
    assert!(res
        .get("stdout")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .contains("Gradle"));
}

#[tokio::test]
async fn test_android_inputs_fail_closed() {
    let r = tools::execute_tool("adb_shell", serde_json::json!({})).await;
    assert!(!ok(&r));
    let r = tools::execute_tool("adb_install", serde_json::json!({})).await;
    assert!(!ok(&r));
    let r = tools::execute_tool("adb_push", serde_json::json!({"src": "/tmp/x", "dst": ""})).await;
    assert!(!ok(&r));
    let r = tools::execute_tool("gradle", serde_json::json!({"dir": "/nonexistent"})).await;
    assert!(!ok(&r));
}
