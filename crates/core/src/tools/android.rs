//! Android device tools: `adb` subcommands, emulator control, Gradle builds.
//!
//! Probed live 2026-09-14 against adb 1.0.41 + emulator-5554 (Pixel Tablet,
//! SDK 35, Android 15) before implementation:
//! - `devices -l`, `shell`, `logcat -d -t`, `push`/`pull` roundtrip,
//!   `pm list`, `dumpsys`, `emulator -list-avds`, `gradlew --version` all OK.
//! - `install` surfaces device errors (signature conflict on re-install).
//! - Emulator adb needs no auth key (`ADB_KEYS_PATH`/redirected `HOME`
//!   still show `device`). Physical devices with adb auth keep working
//!   only outside the strict sandbox (`~/.android` is not bound) — use
//!   explicit approval or `VIORAHARNESS_SANDBOX=off` for those.
//!
//! No hardcoded paths: `ADB_BIN`/`ANDROID_HOME`/`ANDROID_SDK_ROOT` env,
//! then `PATH`. The SDK here lives at `~/android-sdk` with *no*
//! `ANDROID_HOME` set — resolution must not assume that location.

use serde_json::{json, Value};
use std::path::Path;
use tokio::process::Command as TokioCommand;

/// Optional SDK root: explicit `ANDROID_HOME` wins, then `ANDROID_SDK_ROOT`.
pub fn sdk_root() -> Option<String> {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn exists(p: &str) -> bool {
    Path::new(p).exists()
}

fn scan_path(exe: &str) -> Option<String> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let cand = Path::new(dir).join(exe);
            if cand.exists() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// Resolve `adb`: `ADB_BIN` > `$SDK/platform-tools/adb` > `PATH` > `"adb"`.
pub fn resolve_adb() -> String {
    if let Ok(p) = std::env::var("ADB_BIN") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    if let Some(root) = sdk_root() {
        let cand = format!("{}/platform-tools/adb", root.trim_end_matches('/'));
        if exists(&cand) {
            return cand;
        }
    }
    scan_path("adb").unwrap_or_else(|| "adb".into())
}

/// Resolve the emulator binary the same way (`$SDK/emulator/emulator`).
pub fn resolve_emulator() -> String {
    if let Ok(p) = std::env::var("EMULATOR_BIN") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    if let Some(root) = sdk_root() {
        let cand = format!("{}/emulator/emulator", root.trim_end_matches('/'));
        if exists(&cand) {
            return cand;
        }
    }
    scan_path("emulator").unwrap_or_else(|| "emulator".into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub details: String,
}

/// Parse `adb devices -l` output (header line + `<serial> <state> [details]`).
pub fn parse_devices(text: &str) -> Vec<Device> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices") || line.starts_with('*') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(serial), Some(state)) = (parts.next(), parts.next()) else {
            continue;
        };
        out.push(Device {
            serial: serial.into(),
            state: state.into(),
            details: parts.collect::<Vec<_>>().join(" "),
        });
    }
    out
}

/// Pick a target serial: explicit arg > `ANDROID_SERIAL` env > auto-select
/// (exactly one attached device wins; zero/multiple without a pick is an
/// error that lists what is attached).
pub fn pick_serial(
    explicit: Option<&str>,
    env: Option<&str>,
    devices: &[Device],
) -> Result<Option<String>, String> {
    if let Some(s) = explicit.filter(|s| !s.trim().is_empty()) {
        return Ok(Some(s.to_string()));
    }
    if let Some(s) = env.filter(|s| !s.trim().is_empty()) {
        return Ok(Some(s.to_string()));
    }
    match devices {
        [] => Err("no devices attached (start an emulator or plug one in)".into()),
        [only] => Ok(Some(only.serial.clone())),
        many => {
            let list = many
                .iter()
                .map(|d| format!("{} ({})", d.serial, d.state))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "multiple devices attached ({list}) — pass serial explicitly"
            ))
        }
    }
}

fn env_serial() -> Option<String> {
    std::env::var("ANDROID_SERIAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// `adb [-s serial] <subcommand...>`.
pub fn build_adb_base(serial: Option<&str>) -> Vec<String> {
    let mut v = vec![resolve_adb()];
    if let Some(s) = serial.filter(|s| !s.trim().is_empty()) {
        v.push("-s".into());
        v.push(s.to_string());
    }
    v
}

/// Run `adb devices -l` and parse (pure I/O helper, timeout 30s).
pub async fn query_devices() -> Result<Vec<Device>, String> {
    let adb = resolve_adb();
    let mut cmd = TokioCommand::new(&adb);
    cmd.args(["devices", "-l"]);
    crate::sandbox::apply_sandbox(&mut cmd, "/tmp");
    cmd.kill_on_drop(true);
    match tokio::time::timeout(std::time::Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            Ok(parse_devices(&String::from_utf8_lossy(&out.stdout)))
        }
        Ok(Ok(out)) => Err(format!(
            "adb devices failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Ok(Err(e)) => Err(format!("failed to spawn adb ({adb}): {e}")),
        Err(_) => Err("adb devices timed out after 30s".into()),
    }
}

/// Resolve the target serial for a tool call (explicit > env > auto).
/// Returns the serial plus the device list for context.
pub async fn resolve_serial(args: &Value) -> Result<String, Value> {
    let explicit = args.get("serial").and_then(|v| v.as_str());
    // Fast path: caller (or env) already picked.
    if let Some(s) = explicit
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .or_else(env_serial)
    {
        return Ok(s);
    }
    let devs = query_devices()
        .await
        .map_err(|e| json!({"ok": false, "error": e}))?;
    pick_serial(None, None, &devs)
        .map_err(|e| json!({"ok": false, "error": e}))?
        .ok_or_else(|| json!({"ok": false, "error": "no serial resolved"}))
}

async fn run_foreground(mut cmd: TokioCommand, timeout_secs: u64) -> Value {
    cmd.kill_on_drop(true);
    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await {
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            json!({"ok": out.status.success(), "code": out.status.code(), "stdout": stdout, "stderr": stderr})
        }
        Ok(Err(e)) => json!({"ok": false, "error": format!("spawn failed: {e}")}),
        Err(_) => json!({"ok": false, "error": format!("timed out after {timeout_secs}s")}),
    }
}

fn sandboxed(program: &str, argv: &[String], cwd: &str) -> TokioCommand {
    let mut cmd = TokioCommand::new(program);
    cmd.args(argv);
    cmd.current_dir(cwd);
    crate::sandbox::apply_sandbox(&mut cmd, cwd);
    cmd
}

fn timeout_arg(args: &Value, key: &str, def: u64) -> u64 {
    args.get(key)
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .unwrap_or(def)
}

fn jailed_path(raw: &str) -> Result<std::path::PathBuf, Value> {
    if raw.trim().is_empty() {
        return Err(json!({"ok": false, "error": "missing path"}));
    }
    let p = super::viora::resolve_path(raw);
    if !super::viora::approved_call() && !super::viora::is_within_root(&p) {
        return Err(
            json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y)", p.display())}),
        );
    }
    Ok(p)
}

pub async fn adb_devices(args: Value) -> Value {
    let adb = resolve_adb();
    let cmd = sandboxed(&adb, &["devices".into(), "-l".into()], "/tmp");
    let timeout = timeout_arg(&args, "timeout", 30);
    let mut res = run_foreground(cmd, timeout).await;
    if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        let devs = parse_devices(res.get("stdout").and_then(|v| v.as_str()).unwrap_or(""));
        res["devices"] = json!(devs
            .iter()
            .map(|d| json!({"serial": d.serial, "state": d.state, "details": d.details}))
            .collect::<Vec<_>>());
    }
    res
}

pub async fn adb_shell(args: Value) -> Value {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if command.trim().is_empty() {
        return json!({"ok": false, "error": "missing command"});
    }
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut argv = build_adb_base(Some(&serial));
    argv.extend(["shell".into(), command.into()]);
    let timeout = timeout_arg(&args, "timeout", 60);
    let mut res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), timeout).await;
    res["serial"] = json!(serial);
    res
}

pub async fn adb_install(args: Value) -> Value {
    let apk_raw = args.get("apk").and_then(|v| v.as_str()).unwrap_or("");
    let apk = match jailed_path(apk_raw) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !apk.is_file() {
        return json!({"ok": false, "error": format!("apk not found: {apk_raw}")});
    }
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut argv = build_adb_base(Some(&serial));
    argv.push("install".into());
    for (key, flag) in [
        ("reinstall", "-r"),
        ("downgrade", "-d"),
        ("grant", "-g"),
        ("test", "-t"),
    ] {
        if args.get(key).and_then(|v| v.as_bool()).unwrap_or(false) {
            argv.push(flag.into());
        }
    }
    argv.push(apk.to_string_lossy().to_string());
    let timeout = timeout_arg(&args, "timeout", 180);
    let mut res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), timeout).await;
    res["serial"] = json!(serial);
    res["apk"] = json!(apk_raw);
    res
}

pub async fn adb_logcat(args: Value) -> Value {
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut argv = build_adb_base(Some(&serial));
    argv.extend(["logcat".into(), "-d".into()]);
    if let Some(n) = args.get("lines").and_then(|v| v.as_u64()) {
        argv.extend(["-t".into(), n.to_string()]);
    }
    if let Some(f) = args.get("format").and_then(|v| v.as_str()) {
        if !f.trim().is_empty() {
            argv.extend(["-v".into(), f.into()]);
        }
    }
    for f in match args.get("filter") {
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    } {
        argv.push(f);
    }
    let timeout = timeout_arg(&args, "timeout", 60);
    // Dump mode only: streaming logcat would block the turn. For a live
    // tail, run `adb logcat` via bash background:true and follow /tasks.
    let mut res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), timeout).await;
    res["serial"] = json!(serial);
    res["hint"] =
        json!("dump mode (-d). For a live tail use bash background:true with adb logcat and follow /tasks");
    res
}

pub async fn adb_push(args: Value) -> Value {
    let src_raw = args.get("src").and_then(|v| v.as_str()).unwrap_or("");
    let dst = args.get("dst").and_then(|v| v.as_str()).unwrap_or("");
    if dst.trim().is_empty() {
        return json!({"ok": false, "error": "missing dst (device path)"});
    }
    let src = match jailed_path(src_raw) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !src.exists() {
        return json!({"ok": false, "error": format!("src not found: {src_raw}")});
    }
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut argv = build_adb_base(Some(&serial));
    argv.extend(["push".into(), src.to_string_lossy().to_string(), dst.into()]);
    let timeout = timeout_arg(&args, "timeout", 120);
    let mut res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), timeout).await;
    res["serial"] = json!(serial);
    res
}

/// Capture the device screen in one call: `screencap -p` to a temp device
/// file, pull it to `out`, return base64 vision. Verified live 2026-09-14
/// (emulator-5554 → 224KB real PNG). The remote temp file is removed
/// best-effort afterwards.
pub async fn adb_screenshot(args: Value) -> Value {
    let out_raw = args
        .get("out")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("/tmp/adb_screenshot.png");
    let out = match jailed_path(out_raw) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let remote = format!("/data/local/tmp/vh_screenshot_{}.png", std::process::id());
    let timeout = timeout_arg(&args, "timeout", 60);
    let mut cap_argv = build_adb_base(Some(&serial));
    cap_argv.extend(["shell".into(), format!("screencap -p {remote}")]);
    let cap = run_foreground(sandboxed(&cap_argv[0], &cap_argv[1..], "/tmp"), timeout).await;
    if !cap.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return json!({"ok": false, "serial": serial, "stage": "screencap", "stdout": cap.get("stdout"), "stderr": cap.get("stderr")});
    }
    let mut pull_argv = build_adb_base(Some(&serial));
    pull_argv.extend([
        "pull".into(),
        remote.clone(),
        out.to_string_lossy().to_string(),
    ]);
    let pull = run_foreground(sandboxed(&pull_argv[0], &pull_argv[1..], "/tmp"), timeout).await;
    // Best-effort remote cleanup either way.
    let mut rm_argv = build_adb_base(Some(&serial));
    rm_argv.extend(["shell".into(), format!("rm -f {remote}")]);
    let _ = run_foreground(sandboxed(&rm_argv[0], &rm_argv[1..], "/tmp"), 30).await;
    if !pull.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
        return json!({"ok": false, "serial": serial, "stage": "pull", "stdout": pull.get("stdout"), "stderr": pull.get("stderr")});
    }
    let mut res = json!({"ok": true, "serial": serial, "out": out_raw});
    match tokio::fs::read(&out).await {
        Ok(bytes) => {
            use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
            let b64 = BASE64.encode(&bytes);
            res["base64"] = json!(b64);
            res["base64_len"] = json!(res["base64"].as_str().map(|s| s.len()).unwrap_or(0));
            res["bytes"] = json!(bytes.len());
            res["hint"] =
                json!("PNG vision attached as base64 — describe what you see, don't dump pixels");
        }
        Err(e) => {
            res = json!({"ok": false, "serial": serial, "error": format!("pulled but cannot read {out_raw}: {e}")});
        }
    }
    res
}

pub async fn adb_pull(args: Value) -> Value {
    let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("");
    let dst_raw = args.get("dst").and_then(|v| v.as_str()).unwrap_or("");
    if src.trim().is_empty() {
        return json!({"ok": false, "error": "missing src (device path)"});
    }
    let dst = match jailed_path(dst_raw) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let serial = match resolve_serial(&args).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let mut argv = build_adb_base(Some(&serial));
    argv.extend(["pull".into(), src.into(), dst.to_string_lossy().to_string()]);
    let timeout = timeout_arg(&args, "timeout", 120);
    let mut res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), timeout).await;
    res["serial"] = json!(serial);
    res["out"] = json!(dst_raw);
    res
}

/// Build `emulator -list-avds` argv (pure).
pub fn build_emulator_list_args() -> Vec<String> {
    vec![resolve_emulator(), "-list-avds".into()]
}

/// Build the emulator boot shell command for a detached task (pure).
/// Runs *outside* the bwrap sandbox: the emulator is itself a KVM virtual
/// machine (a stronger boundary than bwrap), and strict sandboxing would
/// hide `/dev/kvm` and break hardware acceleration.
pub fn build_emulator_boot_cmd(avd: &str, wipe: bool, no_snapshot: bool) -> String {
    let emu = resolve_emulator();
    let mut parts = vec![format!("'{emu}' -avd '{avd}' -netdelay none")];
    if wipe {
        parts.push("-wipe-data".into());
    }
    if no_snapshot {
        parts.push("-no-snapshot".into());
    }
    parts.join(" ")
}

/// Wait until `sys.boot_completed == 1` (polls `adb shell getprop`).
pub async fn wait_for_boot(serial: &str, deadline_secs: u64) -> bool {
    let adb = resolve_adb();
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < deadline_secs {
        let mut cmd = TokioCommand::new(&adb);
        cmd.args(["-s", serial, "shell", "getprop", "sys.boot_completed"]);
        crate::sandbox::apply_sandbox(&mut cmd, "/tmp");
        cmd.kill_on_drop(true);
        if let Ok(Ok(out)) =
            tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output()).await
        {
            if String::from_utf8_lossy(&out.stdout).trim() == "1" {
                return true;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    false
}

pub async fn emulator_ctl(args: Value) -> Value {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("list");
    match action {
        "list" => {
            let argv = build_emulator_list_args();
            let res = run_foreground(sandboxed(&argv[0], &argv[1..], "/tmp"), 30).await;
            let mut res = res;
            if res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                let avds: Vec<String> = res
                    .get("stdout")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                res["avds"] = json!(avds);
            }
            res
        }
        "boot" => {
            let avd = args.get("avd").and_then(|v| v.as_str()).unwrap_or("");
            if avd.trim().is_empty() {
                return json!({"ok": false, "error": "boot needs avd (see emulator list)"});
            }
            let wipe = args.get("wipe").and_then(|v| v.as_bool()).unwrap_or(false);
            let no_snapshot = args
                .get("no_snapshot")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let wait = args.get("wait").and_then(|v| v.as_bool()).unwrap_or(false);
            let deadline = timeout_arg(&args, "timeout", 300);
            let cmdline = build_emulator_boot_cmd(avd, wipe, no_snapshot);
            let task = super::tasks::spawn_task_opts(&cmdline, "/tmp", false);
            let mut res = super::tasks::launch_result(&task);
            res["avd"] = json!(avd);
            res["notice"] =
                json!("emulator runs outside the bwrap sandbox (it is itself a KVM VM; sandboxing would hide /dev/kvm)");
            if wait {
                // Best-effort: derive the new serial by diffing device lists.
                let before = query_devices().await.unwrap_or_default();
                let mut serial = None;
                let start = std::time::Instant::now();
                while start.elapsed().as_secs() < deadline {
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    if let Ok(now) = query_devices().await {
                        if let Some(found) = now.iter().find(|d| {
                            !before.iter().any(|b| b.serial == d.serial) && d.state == "device"
                        }) {
                            serial = Some(found.serial.clone());
                            break;
                        }
                    }
                }
                match serial {
                    Some(s) => {
                        res["serial"] = json!(s);
                        res["booted"] = json!(wait_for_boot(&s, 60).await);
                    }
                    None => {
                        res["booted"] = json!(false);
                        res["warning"] = json!("no new device appeared before the deadline — follow the task log, then poll adb_devices");
                    }
                }
            } else {
                res["hint"] =
                    json!("poll adb_devices until the new emulator appears, then adb_shell getprop sys.boot_completed");
            }
            res
        }
        other => {
            json!({"ok": false, "error": format!("unknown emulator action: {other} (list|boot)")})
        }
    }
}

/// Pick the Gradle runner: project `gradlew` wins, else `gradle` on PATH.
pub fn resolve_gradle(project_dir: &str) -> String {
    let wrapper = Path::new(project_dir).join("gradlew");
    if wrapper.is_file() {
        return wrapper.to_string_lossy().to_string();
    }
    scan_path("gradle").unwrap_or_else(|| "gradle".into())
}

/// Build the gradle shell command for a project (pure).
pub fn build_gradle_cmd(runner: &str, tasks: &[String], offline: bool, extra: &[String]) -> String {
    let mut parts = vec![format!("'{runner}'")];
    if offline {
        parts.push("--offline".into());
    }
    parts.push("--no-daemon".into());
    for t in tasks {
        parts.push(format!("'{t}'"));
    }
    for e in extra {
        parts.push(e.clone());
    }
    parts.join(" ")
}

pub async fn gradle(args: Value) -> Value {
    let dir_raw = args.get("dir").and_then(|v| v.as_str()).unwrap_or(".");
    let dir = match jailed_path(dir_raw) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !dir.is_dir() {
        return json!({"ok": false, "error": format!("not a directory: {dir_raw}")});
    }
    let dir_str = dir.to_string_lossy().to_string();
    let tasks: Vec<String> = match args.get("tasks") {
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .collect(),
        _ => vec!["assembleDebug".into()],
    };
    let offline = args
        .get("offline")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let extra: Vec<String> = match args.get("args") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    };
    let runner = resolve_gradle(&dir_str);
    let cmdline = build_gradle_cmd(&runner, &tasks, offline, &extra);
    let timeout = timeout_arg(&args, "timeout", 600);
    if args
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        // Unsandboxed like emulator boot: Gradle needs the SDK, dependency
        // caches (~/.gradle), and its daemon/JDK — bwrap hides all three.
        let task = super::tasks::spawn_task_opts(&cmdline, &dir_str, false);
        let mut res = super::tasks::launch_result(&task);
        res["notice"] = json!("gradle runs outside the bwrap sandbox (needs SDK + ~/.gradle caches); project dir stays jailed");
        return res;
    }
    let mut cmd = TokioCommand::new("bash");
    cmd.args(["-c", &cmdline]);
    cmd.current_dir(&dir_str);
    let mut res = run_foreground(cmd, timeout).await;
    res["notice"] = json!("gradle runs outside the bwrap sandbox (needs SDK + ~/.gradle caches); project dir stays jailed");
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn devices_output_parses() {
        let sample = "List of devices attached\nemulator-5554\tdevice product:sdk_gtablet_x86_64 model:Pixel_Tablet device:emu64xa transport_id:1\nRZ8M1234\tunauthorized\n\n";
        let devs = parse_devices(sample);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].serial, "emulator-5554");
        assert_eq!(devs[0].state, "device");
        assert!(devs[0].details.contains("Pixel_Tablet"));
        assert_eq!(devs[1].state, "unauthorized");
        assert!(parse_devices("List of devices attached\n\n").is_empty());
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn serial_pick_order() {
        let one = vec![Device {
            serial: "emulator-5554".into(),
            state: "device".into(),
            details: String::new(),
        }];
        assert_eq!(
            pick_serial(Some("A"), Some("B"), &one),
            Ok(Some("A".into())),
            "explicit wins"
        );
        assert_eq!(
            pick_serial(None, Some("B"), &one),
            Ok(Some("B".into())),
            "env next"
        );
        assert_eq!(
            pick_serial(None, None, &one),
            Ok(Some("emulator-5554".into())),
            "single device auto-selected"
        );
        assert!(
            pick_serial(None, None, &[]).is_err(),
            "none attached errors"
        );
        let two = vec![
            Device {
                serial: "A".into(),
                state: "device".into(),
                details: String::new(),
            },
            Device {
                serial: "B".into(),
                state: "device".into(),
                details: String::new(),
            },
        ];
        let err = pick_serial(None, None, &two).unwrap_err();
        assert!(err.contains('A') && err.contains('B'), "lists both: {err}");
    }

    #[test]
    fn adb_base_orders_serial_first() {
        let v = build_adb_base(Some("emulator-5554"));
        assert_eq!(&v[1..], &["-s", "emulator-5554"]);
        let v = build_adb_base(None);
        assert_eq!(v.len(), 1, "no -s without a serial");
    }

    #[test]
    fn emulator_boot_cmd_shape() {
        // Point resolution at a fake SDK so the test is hermetic.
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("ANDROID_HOME").ok();
        let dir = std::env::temp_dir().join(format!("vh-sdk-{}", std::process::id()));
        let emu_dir = dir.join("emulator");
        std::fs::create_dir_all(&emu_dir).unwrap();
        std::fs::write(emu_dir.join("emulator"), b"fake").unwrap();
        std::env::set_var("ANDROID_HOME", &dir);
        let cmd = build_emulator_boot_cmd("Pixel_9", false, true);
        assert!(cmd.contains("emulator' -avd 'Pixel_9'"), "{cmd}");
        assert!(cmd.contains("-no-snapshot"), "{cmd}");
        assert!(!cmd.contains("-wipe-data"), "{cmd}");
        let wiped = build_emulator_boot_cmd("Pixel_9", true, false);
        assert!(wiped.contains("-wipe-data"), "{wiped}");
        match prev {
            Some(v) => std::env::set_var("ANDROID_HOME", v),
            None => std::env::remove_var("ANDROID_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gradle_cmd_prefers_wrapper_and_flags() {
        let dir = std::env::temp_dir().join(format!("vh-grad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("gradlew"), b"fake").unwrap();
        let ds = dir.to_string_lossy().to_string();
        assert_eq!(
            resolve_gradle(&ds),
            dir.join("gradlew").to_string_lossy().to_string()
        );
        let cmd = build_gradle_cmd(
            &resolve_gradle(&ds),
            &["assembleDebug".to_string()],
            true,
            &[],
        );
        assert!(
            cmd.contains("gradlew' --offline --no-daemon 'assembleDebug'"),
            "{cmd}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sdk_resolution_is_env_only() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (ph, pr) = (
            std::env::var("ANDROID_HOME").ok(),
            std::env::var("ANDROID_SDK_ROOT").ok(),
        );
        std::env::remove_var("ANDROID_HOME");
        std::env::remove_var("ANDROID_SDK_ROOT");
        assert_eq!(sdk_root(), None, "no hardcoded SDK fallback");
        std::env::set_var("ANDROID_SDK_ROOT", "/opt/android-sdk");
        assert_eq!(sdk_root(), Some("/opt/android-sdk".into()));
        match ph {
            Some(v) => std::env::set_var("ANDROID_HOME", v),
            None => std::env::remove_var("ANDROID_HOME"),
        }
        match pr {
            Some(v) => std::env::set_var("ANDROID_SDK_ROOT", v),
            None => std::env::remove_var("ANDROID_SDK_ROOT"),
        }
    }

    #[tokio::test]
    async fn missing_inputs_fail_closed_without_touching_devices() {
        let r = adb_shell(json!({})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("missing command"),
            "{r}"
        );
        let r = adb_install(json!({"apk": "/nonexistent/x.apk"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("outside project root"),
            "{r}"
        );
        let r = emulator_ctl(json!({"action": "boot"})).await;
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("needs avd"), "{r}");
        let r = emulator_ctl(json!({"action": "frobnicate"})).await;
        assert_eq!(r["ok"], false);
    }
}
