// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const VIORA_TIMEOUT_SECS: u64 = 120;

/// Optional VioraEDA checkout root, explicit opt-in via `VIOSPICE_ROOT` only.
///
/// No hardcoded fallback: the harness must not assume where (or whether) the
/// VioraEDA source tree lives. The file jail never grants it access; tools
/// only ever invoke the `viora` *binary* (see [`resolve_viora`]).
pub fn viospice_root() -> Option<String> {
    std::env::var("VIOSPICE_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// Resolve the `viora` binary: explicit `VIORA_BIN` wins, otherwise the first
/// `viora` found on `PATH` (which already covers install locations such as
/// `~/.local/bin`). No hardcoded checkout/build/output paths — installers put
/// Split a PATH-style variable the platform way (`:` vs `;`). A hardcoded
/// `split(':')` silently breaks every tool lookup on Windows (whose PATH
/// entries contain drive-letter colons).
pub fn each_path_dir(path_var: &str) -> impl Iterator<Item = PathBuf> + '_ {
    std::env::split_paths(path_var).filter(|p| !p.as_os_str().is_empty())
}

/// `dir/name`, with an `.exe` fallback so bare-name PATH probes work on
/// Windows (binaries carry the extension but callers name them bare).
/// Returns `None` when neither exists.
pub fn join_exe(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name);
    if p.exists() {
        return Some(p);
    }
    let e = dir.join(format!("{name}.exe"));
    if e.exists() {
        return Some(e);
    }
    None
}

/// `viora` on PATH; the harness must never reach into a source tree.
pub fn resolve_viora() -> String {
    if let Ok(p) = std::env::var("VIORA_BIN") {
        if !p.trim().is_empty() {
            return p;
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in each_path_dir(&path_var) {
            if let Some(cand) = join_exe(&dir, "viora") {
                return cand.to_string_lossy().to_string();
            }
        }
    }

    "viora".into()
}

#[derive(Debug, Clone)]
pub struct VioraOutput {
    pub ok: bool,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub data: Option<Value>,
}

pub(crate) fn extract_json_maybe(out: &str) -> Option<Value> {
    if out.trim().is_empty() {
        return None;
    }

    if let Ok(v) = serde_json::from_str::<Value>(out.trim()) {
        return Some(v);
    }

    let start = out.find('{')?;
    let end = out.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = &out[start..=end];
    serde_json::from_str::<Value>(slice).ok()
}

pub async fn run_viora_command(args: &[String], timeout_secs: Option<u64>) -> VioraOutput {
    let exe = resolve_viora();
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(VIORA_TIMEOUT_SECS));

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());

    tracing::debug!("viora exe={exe} args={args:?} timeout={timeout:?} cwd={cwd}");

    let is_view = args
        .first()
        .map(|a| a == "view" || a == "simulate" || a == "dialog-render")
        .unwrap_or(false);

    let mut use_no_daemon = std::env::var("VIORA_NO_DAEMON").is_ok();
    for attempt in 0..=2 {
        let mut cmd = Command::new(&exe);
        cmd.args(args);
        cmd.current_dir(&cwd);
        cmd.env("QT_QPA_PLATFORM", if is_view { "" } else { "offscreen" });
        if use_no_daemon {
            cmd.env("VIORA_NO_DAEMON", "1");
        }
        crate::sandbox::apply_sandbox(&mut cmd, &cwd);
        cmd.kill_on_drop(true);

        let res = tokio::time::timeout(timeout, cmd.output()).await;
        match res {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let code = output.status.code().unwrap_or(-1);
                let data = extract_json_maybe(&stdout);
                if output.status.success() {
                    return VioraOutput {
                        ok: true,
                        code,
                        stdout,
                        stderr,
                        data,
                    };
                } else {
                    let err_msg = if !stderr.trim().is_empty() {
                        stderr.clone()
                    } else if !stdout.trim().is_empty() {
                        stdout.clone()
                    } else {
                        format!("viora exited {}", code)
                    };

                    if code == 11 || err_msg.contains("crashed inside the engine worker") {
                        use_no_daemon = true;
                        tracing::warn!("viora worker crash detected (code 11) — next retry will use VIORA_NO_DAEMON=1");
                    }
                    if attempt < 2 {
                        tracing::warn!(
                            "viora attempt {attempt} failed (code {code}): {err_msg} — retrying"
                        );
                        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1) as u64)).await;
                        continue;
                    }
                    return VioraOutput {
                        ok: false,
                        code,
                        stdout: err_msg,
                        stderr,
                        data,
                    };
                }
            }
            Ok(Err(e)) => {
                let msg = format!("failed to spawn viora ({exe}): {e}");
                if attempt < 2 {
                    tracing::warn!("{msg} — retrying");
                    tokio::time::sleep(Duration::from_millis(500 * (attempt + 1) as u64)).await;
                    continue;
                }
                return VioraOutput {
                    ok: false,
                    code: -1,
                    stdout: msg.clone(),
                    stderr: msg,
                    data: None,
                };
            }
            Err(_) => {
                let msg = format!(
                    "viora timed out after {}s (args={args:?})",
                    timeout.as_secs()
                );
                if attempt < 2 {
                    tracing::warn!("{msg} — retrying");
                    continue;
                }
                return VioraOutput {
                    ok: false,
                    code: 124,
                    stdout: msg.clone(),
                    stderr: msg,
                    data: None,
                };
            }
        }
    }
    unreachable!()
}

pub fn normalize_portable(path_str: &str) -> String {
    let s = path_str.replace('\\', "/");
    // Split off a Windows drive prefix (`D:`) first: `components()` reports
    // it as Prefix+RootDir, which the cleaner below would otherwise drop —
    // turning every absolute Windows path into a wrong relative one (drive
    // letter lost, jail checks fail closed on everything).
    let (drive, rest) = match s.split_once(':') {
        Some((d, tail)) if d.len() == 1 && d.as_bytes()[0].is_ascii_alphabetic() => {
            (format!("{d}:"), tail)
        }
        _ => (String::new(), s.as_str()),
    };
    let p = PathBuf::from(rest);
    let mut out: Vec<String> = Vec::new();
    let is_abs = !drive.is_empty() || rest.starts_with('/');
    for comp in p.components() {
        use std::path::Component::*;
        match comp {
            Prefix(_) | RootDir => {}
            CurDir => {}
            ParentDir => {
                out.pop();
            }
            Normal(c) => out.push(c.to_string_lossy().to_string()),
        }
    }
    let mut cleaned = if is_abs {
        format!("{drive}/{}", out.join("/"))
    } else {
        out.join("/")
    };
    if cleaned.is_empty() {
        cleaned = ".".into();
    }

    cleaned
}

pub fn relativize_if_under_base(path: &Path, base: &str) -> String {
    let p_str = normalize_portable(&path.to_string_lossy());
    let b_str = normalize_portable(base);
    let b_prefix = if b_str.ends_with('/') {
        b_str.clone()
    } else {
        format!("{b_str}/")
    };
    if p_str.starts_with(&b_prefix) {
        p_str[b_prefix.len()..].to_string()
    } else {
        p_str
    }
}

pub fn resolve_path(path_str: &str) -> PathBuf {
    let normalized = normalize_portable(path_str);
    if let Some(rest) = normalized.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    let p = PathBuf::from(&normalized);
    // A leading `/` is absolute intent even where the OS disagrees
    // (Windows: rooted without a drive is not `is_absolute`, but joining
    // it onto the cwd drive would silently relocate it inside the project
    // — return it as-is and let the jail vet it instead).
    if p.is_absolute() || normalized.starts_with('/') {
        return p;
    }

    if let Ok(cwd) = std::env::current_dir() {
        return cwd.join(&p);
    }

    p
}

pub fn approved_call() -> bool {
    matches!(
        std::env::var("VIORAHARNESS_APPROVED_CALL")
            .map(|v| v.to_lowercase())
            .as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

pub fn is_within_root(path: &Path) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"));

    let path_norm = {
        let s = normalize_portable(&path.to_string_lossy());
        let pb = PathBuf::from(&s);
        // A leading `/` is rooted intent: never join it onto the cwd drive
        // (Windows would silently relocate `\evil` to `D:\evil`, landing
        // inside the project and defeating the jail — the traversal tests
        // cover exactly this). Unjoined it matches no root and denies.
        if pb.is_absolute() || s.starts_with('/') {
            pb
        } else {
            cwd.join(pb)
        }
    };

    if path.starts_with("/tmp") || path_norm.starts_with("/tmp") {
        return true;
    }
    // Scratch allowance: the platform temp dir (`/tmp` on Linux, a per-user
    // sandbox on macOS/Windows). Without this, every scratch-file workflow
    // fails closed off-Linux (macOS temp lives under /var, Windows under
    // %TEMP%) — for tests and real users alike.
    {
        let tmp = std::env::temp_dir();
        if path.starts_with(&tmp) || path_norm.starts_with(&tmp) {
            return true;
        }
        // Case-insensitive filesystems (macOS/Windows) plus safety checks
        // that lowercase commands before resolving: a lowercased temp path
        // must still match. Linux keeps exact semantics (folding there
        // would wrongly admit e.g. /TMP).
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let tmp_folded = normalize_portable(&tmp.to_string_lossy()).to_lowercase();
            let path_folded = path_norm.to_string_lossy().to_lowercase();
            if path_folded.starts_with(&tmp_folded) {
                return true;
            }
        }
    }

    let path_str_lossy = path_norm.to_string_lossy().to_string();
    if path_str_lossy.contains("vioraharness/logs")
        || path_str_lossy.contains(".local/share/vioraharness")
        || path.to_string_lossy().contains("vioraharness/logs")
    {
        return true;
    }

    let check = |root: &str| {
        let root_norm = normalize_portable(root);
        let root_pb = PathBuf::from(&root_norm);
        let root_canonical = root_pb.canonicalize().unwrap_or_else(|_| root_pb.clone());
        let path_canonical = path_norm
            .canonicalize()
            .unwrap_or_else(|_| path_norm.clone());
        path_canonical.starts_with(&root_canonical)
            || path_norm.starts_with(&root_pb)
            || path_norm.to_string_lossy().starts_with(&root_norm)
    };

    if check(&cwd.to_string_lossy()) {
        return true;
    }

    // NOTE: the VioraEDA source tree is deliberately NOT an allowed root —
    // the harness drives the `viora` binary via PATH/VIORA_BIN and must not
    // read or write the simulator's own sources, even when cwd sits inside
    // a checkout (e.g. `VIOSPICE_ROOT` set for examples). Explicit per-call
    // approval (`VIORAHARNESS_APPROVED_CALL`) still lifts the jail.

    let harness_root = std::env::var("VIORAHARNESS_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            let mut cur = cwd.clone();
            loop {
                if cur.join("vioraharness.json").exists() {
                    return Some(cur.to_string_lossy().to_string());
                }
                match cur.parent() {
                    Some(p) => cur = p.to_path_buf(),
                    None => break,
                }
            }
            None
        })
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").to_string());

    let harness_root = if harness_root.ends_with("crates/core") {
        harness_root.trim_end_matches("/crates/core").to_string()
    } else if harness_root.ends_with("crates\\core") {
        harness_root.trim_end_matches("\\crates\\core").to_string()
    } else {
        harness_root
    };
    let cwd_path = cwd.clone();
    let harness_path = PathBuf::from(&harness_root);

    let cwd_is_harness = cwd_path.starts_with(&harness_path)
        || cwd
            .canonicalize()
            .ok()
            .zip(harness_path.canonicalize().ok())
            .map(|(a, b)| a.starts_with(b))
            .unwrap_or(false);
    if cwd_is_harness && check(&harness_root) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn extract_json_shapes() {
        assert_eq!(
            extract_json_maybe(r#"{"ok":true,"n":1}"#).unwrap()["n"],
            serde_json::json!(1)
        );
        let mixed = "INFO booting\nWARN slow\n{\"ok\":true}\nbye\n";
        assert_eq!(
            extract_json_maybe(mixed).unwrap()["ok"],
            serde_json::json!(true)
        );
        assert!(extract_json_maybe("").is_none());
        assert!(extract_json_maybe("   ").is_none());
        assert!(extract_json_maybe("no braces here").is_none());
        assert!(extract_json_maybe("{not json}").is_none());
        assert!(extract_json_maybe("} backwards {").is_none());
    }

    #[test]
    fn normalize_portable_battery() {
        assert_eq!(normalize_portable("a\\b\\c"), "a/b/c");
        assert_eq!(normalize_portable("./a/./b"), "a/b");
        assert_eq!(normalize_portable("a/b/../c"), "a/c");
        assert_eq!(normalize_portable("/x/../y"), "/y");
        assert_eq!(normalize_portable(""), ".");
        assert_eq!(normalize_portable("."), ".");
        assert_eq!(normalize_portable("a//b"), "a/b");
        // Windows drive prefixes survive (components() would drop them,
        // turning absolute paths relative and breaking the jail).
        assert_eq!(normalize_portable("D:\\a\\..\\b"), "D:/b");
        assert_eq!(normalize_portable("d:/x/y"), "d:/x/y");
        assert_eq!(normalize_portable("C:/"), "C:/");
    }

    #[test]
    fn relativize_under_base() {
        assert_eq!(
            relativize_if_under_base(Path::new("/base/sub/f.cir"), "/base"),
            "sub/f.cir"
        );
        assert_eq!(
            relativize_if_under_base(Path::new("/base/sub/f.cir"), "/base/"),
            "sub/f.cir"
        );
        assert_eq!(
            relativize_if_under_base(Path::new("/other/f.cir"), "/base"),
            "/other/f.cir"
        );
        assert_eq!(
            relativize_if_under_base(Path::new("/baseother/f"), "/base"),
            "/baseother/f",
            "prefix without slash boundary must not relativize"
        );
    }

    #[test]
    fn resolve_path_home_and_relative() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("HOME").ok();
        std::env::set_var("HOME", "/home/tester");
        assert_eq!(
            resolve_path("~/docs/f.cir"),
            PathBuf::from("/home/tester/docs/f.cir")
        );
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_path("rel/f.cir"), cwd.join("rel/f.cir"));
        assert_eq!(resolve_path("/abs/f.cir"), PathBuf::from("/abs/f.cir"));
    }

    #[test]
    fn approved_call_variants() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        for v in ["1", "true", "TRUE", "yes", "Yes"] {
            std::env::set_var("VIORAHARNESS_APPROVED_CALL", v);
            assert!(approved_call(), "truthy: {v}");
        }
        for v in ["0", "false", "no", ""] {
            std::env::set_var("VIORAHARNESS_APPROVED_CALL", v);
            assert!(!approved_call(), "falsy: {v}");
        }
        std::env::remove_var("VIORAHARNESS_APPROVED_CALL");
        assert!(!approved_call());
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }

    #[test]
    fn resolve_viora_env_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORA_BIN").ok();
        std::env::set_var("VIORA_BIN", "/custom/path/viora");
        assert_eq!(resolve_viora(), "/custom/path/viora");
        match prev {
            Some(v) => std::env::set_var("VIORA_BIN", v),
            None => std::env::remove_var("VIORA_BIN"),
        }
    }

    #[test]
    fn within_root_basics() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cwd = std::env::current_dir().unwrap();
        assert!(is_within_root(&cwd.join("sub/file.cir")));
        assert!(is_within_root(Path::new("/tmp/anything.txt")));
        assert!(!is_within_root(Path::new("/etc/passwd")));
        assert!(!is_within_root(Path::new("/home/someone-else/x")));
    }

    #[test]
    fn viospice_root_is_opt_in_only() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIOSPICE_ROOT").ok();
        std::env::remove_var("VIOSPICE_ROOT");
        assert_eq!(viospice_root(), None, "no hardcoded checkout fallback");
        std::env::set_var("VIOSPICE_ROOT", "/opt/viora-checkout");
        assert_eq!(viospice_root(), Some("/opt/viora-checkout".to_string()));
        match prev {
            Some(v) => std::env::set_var("VIOSPICE_ROOT", v),
            None => std::env::remove_var("VIOSPICE_ROOT"),
        }
    }

    #[test]
    fn resolve_viora_never_probes_checkout_paths() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_bin = std::env::var("VIORA_BIN").ok();
        let prev_path = std::env::var("PATH").ok();
        let prev_root = std::env::var("VIOSPICE_ROOT").ok();
        // Fake checkout with a build-tree binary: the resolver must ignore it.
        let dir = std::env::temp_dir().join(format!("vh-noprobe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let build = dir.join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("viora"), b"fake").unwrap();
        std::env::set_var("VIOSPICE_ROOT", &dir);
        std::env::remove_var("VIORA_BIN");
        let empty = std::env::temp_dir().join(format!("vh-empty-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        std::env::set_var("PATH", &empty);
        assert_eq!(
            resolve_viora(),
            "viora",
            "VIORA_BIN unset + empty PATH falls back to PATH lookup, never build/ dirs"
        );
        match prev_bin {
            Some(v) => std::env::set_var("VIORA_BIN", v),
            None => std::env::remove_var("VIORA_BIN"),
        }
        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match prev_root {
            Some(v) => std::env::set_var("VIOSPICE_ROOT", v),
            None => std::env::remove_var("VIOSPICE_ROOT"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn jail_denies_simulator_sources() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIOSPICE_ROOT").ok();
        std::env::set_var("VIOSPICE_ROOT", "/opt/viora-checkout");
        // Even with the checkout root exported, its sources stay outside
        // the jail (cwd is the harness repo here, /opt is neither cwd,
        // harness root, /tmp, nor logs).
        assert!(!is_within_root(Path::new(
            "/opt/viora-checkout/cli/main.cpp"
        )));
        assert!(!is_within_root(Path::new(
            "/opt/viora-checkout/build/viora"
        )));
        match prev {
            Some(v) => std::env::set_var("VIOSPICE_ROOT", v),
            None => std::env::remove_var("VIOSPICE_ROOT"),
        }
    }
}

/// Truncate to at most `max_bytes` bytes without splitting a UTF-8 char
/// (plain byte slicing / `String::truncate` panic on multi-byte text).
pub fn truncate_to_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate_to_bytes;

    #[test]
    fn never_splits_multibyte_chars() {
        // ｱ is 3 bytes at 6999..7002; byte 7000 lands inside it.
        let s = "A".repeat(6999) + "ｱ" + &"B".repeat(8000);
        let t = truncate_to_bytes(&s, 7000);
        assert_eq!(t, "A".repeat(6999), "backs up to the char boundary");
        assert!(t.len() <= 7000);
        assert_eq!(truncate_to_bytes("hi", 7000), "hi");
        assert_eq!(truncate_to_bytes("", 0), "");
        assert_eq!(truncate_to_bytes("éé", 3), "é");
    }
}
