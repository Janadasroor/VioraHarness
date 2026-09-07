use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const VIORA_TIMEOUT_SECS: u64 = 120;

pub fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

pub fn viospice_root() -> String {
    std::env::var("VIOSPICE_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{}/qt_projects/viospice", home_dir()))
}

pub fn resolve_viora() -> String {
    if let Ok(p) = std::env::var("VIORA_BIN") {
        if !p.is_empty() {
            return p;
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let cand = Path::new(dir).join("viora");
            if cand.exists() {
                return cand.to_string_lossy().to_string();
            }
        }
    }

    let local_bin = format!(
        "{}/.local/bin/viora",
        std::env::var("HOME").unwrap_or_default()
    );
    if Path::new(&local_bin).exists() {
        return local_bin;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let viospice_root = viospice_root();
        let cwd_is_viospice =
            cwd.starts_with(&viospice_root) || cwd.to_string_lossy().contains("viospice");
        if cwd_is_viospice {
            for cand in [
                format!("{viospice_root}/build/viora"),
                format!("{viospice_root}/build-debug/viora"),
                format!("{viospice_root}/build-asan/viora"),
                format!("{viospice_root}/build-release/viora"),
            ] {
                if Path::new(&cand).exists() {
                    return cand;
                }
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

fn extract_json_maybe(out: &str) -> Option<Value> {
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
    let mut s = path_str.replace('\\', "/");

    let p = PathBuf::from(&s);
    let mut out: Vec<String> = Vec::new();
    let is_abs = s.starts_with('/');
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
        format!("/{}", out.join("/"))
    } else {
        out.join("/")
    };
    if cleaned.is_empty() {
        cleaned = ".".into();
    }

    s = cleaned;
    s
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
    if p.is_absolute() {
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
        let pb = PathBuf::from(s);
        if pb.is_absolute() {
            pb
        } else {
            cwd.join(pb)
        }
    };

    if path.starts_with("/tmp") || path_norm.starts_with("/tmp") {
        return true;
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

    let viospice_root = viospice_root();

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
    let viospice_path = PathBuf::from(&viospice_root);
    let harness_path = PathBuf::from(&harness_root);

    let cwd_is_viospice = cwd_path.starts_with(&viospice_path)
        || cwd
            .canonicalize()
            .ok()
            .zip(viospice_path.canonicalize().ok())
            .map(|(a, b)| a.starts_with(b))
            .unwrap_or(false);
    if cwd_is_viospice && check(&viospice_root) {
        return true;
    }
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
