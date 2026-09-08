use crate::loop_mod::{split_shell_segments, strip_wrappers};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command as TokioCommand;

pub fn is_annihilation(command: &str) -> bool {
    let lower = command.to_lowercase();
    if lower.contains(":(){") || lower.contains(": (){") {
        return true;
    }
    for seg in split_shell_segments(&lower) {
        let seg = strip_wrappers(seg);
        let toks: Vec<&str> = seg.split_whitespace().collect();
        let Some(first) = toks.first() else {
            continue;
        };
        let base = first
            .rsplit('/')
            .next()
            .unwrap_or(first)
            .trim_start_matches("./");
        if base == "rm" {
            let recursive = toks[1..].iter().any(|t| {
                (t.starts_with('-') && !t.starts_with("--") && (t.contains('r') || t.contains('R')))
                    || *t == "--recursive"
            });
            let root_target = toks[1..].iter().any(|t| *t == "/" || *t == "/*");
            if recursive && root_target {
                return true;
            }
        }
        if base == "dd" {
            let raw_disk = toks[1..].iter().any(|t| {
                let of = t.strip_prefix("of=").unwrap_or("");
                of.starts_with("/dev/sd")
                    || of.starts_with("/dev/hd")
                    || of.starts_with("/dev/nvme")
                    || of.starts_with("/dev/mmcblk")
                    || of.starts_with("/dev/vd")
                    || of.starts_with("/dev/xvd")
            });
            if raw_disk {
                return true;
            }
        }
    }
    false
}

pub fn prune_old_logs() {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        })
        .join("vioraharness/logs");
    if !base.exists() {
        return;
    }
    let now = std::time::SystemTime::now();
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime, u64)> = Vec::new();
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    let modified = meta.modified().unwrap_or(now);
                    let age = now.duration_since(modified).unwrap_or_default();

                    if age.as_secs() > 7 * 24 * 3600 {
                        let _ = std::fs::remove_file(entry.path());
                        continue;
                    }
                    let size = meta.len();
                    total += size;
                    files.push((entry.path(), modified, size));
                }
            }
        }
    }

    if total > 100 * 1024 * 1024 {
        files.sort_by_key(|(_, m, _)| *m);
        for (path, _, size) in files {
            if total <= 100 * 1024 * 1024 {
                break;
            }
            let _ = std::fs::remove_file(&path);
            total = total.saturating_sub(size);
        }
    }

    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("vioraharness_tool_") {
                if let Ok(meta) = entry.metadata() {
                    let modified = meta.modified().unwrap_or(now);
                    let age = now.duration_since(modified).unwrap_or_default();
                    if age.as_secs() > 7 * 24 * 3600 {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

pub async fn bash(args: Value) -> Value {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    let timeout_ms = args
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(120_000);
    let workdir = args.get("workdir").and_then(|v| v.as_str()).unwrap_or("");
    let cwd = if workdir.is_empty() {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".into())
    } else {
        let resolved = crate::tools::viora::resolve_path(workdir);
        if !crate::tools::viora::approved_call() && !crate::tools::viora::is_within_root(&resolved)
        {
            return json!({"ok": false, "error": format!("access denied: workdir {} outside project root (approve in the Ask dialog or run with -y)", resolved.display())});
        }
        workdir.to_string()
    };

    if command.trim().is_empty() {
        return json!({"ok": false, "error": "missing command"});
    }

    if is_annihilation(command) {
        return json!({"ok": false, "error": "command denied by policy (destructive to the whole system; never auto-run)"});
    }

    if args
        .get("background")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let task = super::tasks::spawn_task(command, &cwd);
        return super::tasks::launch_result(&task);
    }

    let mut cmd = TokioCommand::new("bash");
    cmd.args(["-c", command]);
    cmd.current_dir(&cwd);
    cmd.env("QT_QPA_PLATFORM", "offscreen");
    crate::sandbox::apply_sandbox(&mut cmd, &cwd);
    cmd.kill_on_drop(true);

    let timeout = Duration::from_millis(timeout_ms);

    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => {
            let stdout_raw = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr_raw = String::from_utf8_lossy(&out.stderr).to_string();
            let code = out.status.code().unwrap_or(-1);

            let full = if stderr_raw.is_empty() {
                stdout_raw.clone()
            } else {
                format!("{stdout_raw}\n--- stderr ---\n{stderr_raw}")
            };
            let lines = full.lines().count() as u64;
            let bytes = full.len() as u64;

            let log_path = {
                let base = std::env::var("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        std::path::PathBuf::from(home).join(".local/share")
                    })
                    .join("vioraharness/logs");
                let _ = std::fs::create_dir_all(&base);

                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let hash = {
                    use std::collections::hash_map::DefaultHasher;
                    use std::hash::{Hash, Hasher};
                    let mut h = DefaultHasher::new();
                    command.hash(&mut h);
                    ts.hash(&mut h);
                    format!("{:x}", h.finish())
                };
                let fname = format!("bash_{}_{}.log", ts % 1_000_000, &hash[..8.min(hash.len())]);
                base.join(fname)
            };
            let log_str = log_path.to_string_lossy().to_string();

            let _ = std::fs::write(&log_path, &full);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
            }

            prune_old_logs();
            let stdout_cut = stdout_raw.len() > 7000;
            let stdout = if stdout_cut {
                format!(
                    "{}...(truncated, {} bytes total, full log: {})",
                    super::viora::truncate_to_bytes(&stdout_raw, 7000),
                    stdout_raw.len(),
                    log_str
                )
            } else {
                stdout_raw.clone()
            };
            let (stderr, stderr_cut) = if stderr_raw.len() > 7000 {
                (
                    format!(
                        "{}...(stderr truncated, {} bytes total, full log: {})",
                        super::viora::truncate_to_bytes(&stderr_raw, 7000),
                        stderr_raw.len(),
                        log_str
                    ),
                    true,
                )
            } else {
                (stderr_raw.clone(), false)
            };
            let truncated = stdout_cut || stderr_cut;
            json!({
                "ok": out.status.success(),
                "code": code,
                "stdout": stdout,
                "stderr": stderr,
                "log": log_str,
                "lines": lines,
                "bytes": bytes,
                "truncated": truncated,
                "hint": format!("Full output ({} lines, {} bytes) saved to log: {} — agent MUST read/grep this log for full data, not re-run bash", lines, bytes, log_str)
            })
        }
        Ok(Err(e)) => json!({"ok": false, "error": e.to_string()}),
        Err(_) => json!({"ok": false, "error": format!("timeout after {}ms", timeout_ms)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annihilation_patterns() {
        for cmd in [
            "rm -rf /",
            "rm -fr /",
            "rm --recursive --force /",
            "rm -rf /*",
            "ls; rm -rf /",
            ":(){ :|:& };:",
            "dd if=x of=/dev/sda",
            "dd of=/dev/nvme0n1",
        ] {
            assert!(is_annihilation(cmd), "annihilation: {cmd}");
        }

        for cmd in [
            "rm -rf /tmp/x",
            "rm -rf ~",
            "rm file",
            "dd if=/dev/zero of=/tmp/f bs=1M",
            "mkfs.ext4 /dev/sda1",
            "ls /",
            "echo rm -rf /",
        ] {
            assert!(!is_annihilation(cmd), "not annihilation: {cmd}");
        }

        for cmd in ["nohup rm -rf /", "timeout 5 rm -rf /", "sudo rm -rf /"] {
            assert!(is_annihilation(cmd), "wrapped annihilation: {cmd}");
        }
    }

    #[tokio::test]
    async fn stdout_cut_never_splits_multibyte_chars() {
        // P0 #4: `&stdout[..7000]` panicked when byte 7000 landed in ｱ (3 bytes).
        let r = bash(json!({"command": "python3 -c \"print('A'*6999 + 'ｱ' + 'B'*8000)\""})).await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["truncated"], true);
        assert!(r["stdout"].as_str().unwrap().contains("truncated"));
        assert!(!r["log"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn katakana_round_trips_through_log() {
        // P0 #4: log bytes must be exact UTF-8 (viewers showing M-o are at fault, not us).
        let r = bash(json!({"command": "printf 'MATRIX \\xef\\xbd\\xa1\\xef\\xbd\\xb2\\n'"})).await;
        assert_eq!(r["ok"], true);
        let log = r["log"].as_str().unwrap().to_string();
        let bytes = std::fs::read(&log).unwrap();
        assert!(
            bytes.windows(3).any(|w| w == [0xef, 0xbd, 0xa1]),
            "ｱ intact: {bytes:02x?}"
        );
    }

    #[tokio::test]
    async fn huge_stderr_is_capped() {
        let r = bash(json!({"command": "python3 -c \"import sys; sys.stderr.write('E'*20000)\""}))
            .await;
        assert_eq!(r["ok"], true);
        let stderr = r["stderr"].as_str().unwrap();
        assert!(stderr.len() <= 7300, "capped: {}", stderr.len());
        assert!(stderr.contains("stderr truncated"), "{stderr}");
        assert_eq!(r["truncated"], true);
    }
}
