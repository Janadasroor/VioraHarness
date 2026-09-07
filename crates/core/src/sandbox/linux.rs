use std::path::Path;

pub fn is_bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn viora_state_dir() -> Option<String> {
    let home = std::env::var("HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let dir = format!("{home}/.local/share/viora");
    std::fs::create_dir_all(&dir).ok()?;
    Path::new(&dir).exists().then_some(dir)
}

pub fn wrap_command(base: &mut tokio::process::Command, workdir: &str) {
    if crate::tools::viora::approved_call() {
        tracing::warn!("sandbox skipped for explicitly approved call in {workdir}");
        return;
    }

    if std::env::var("VIORAHARNESS_SANDBOX")
        .map(|v| v == "off" || v == "0")
        .unwrap_or(false)
    {
        return;
    }

    let sandbox_mode = std::env::var("VIORAHARNESS_SANDBOX_MODE").unwrap_or_else(|_| {
        if let Ok(s) = std::fs::read_to_string("vioraharness.json") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(mode) = v.get("sandbox").and_then(|v| v.as_str()) {
                    return mode.to_string();
                }
            }
        }
        "strict".into()
    });

    if sandbox_mode != "strict" {
        return;
    }
    if !is_bwrap_available() {
        tracing::warn!(
            "bwrap not found — running without sandbox (install bubblewrap for strict isolation)"
        );
        return;
    }

    let program = base.as_std().get_program().to_string_lossy().to_string();
    let args: Vec<String> = base
        .as_std()
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();

    let envs: Vec<(String, Option<String>)> = base
        .as_std()
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().to_string(),
                v.map(|s| s.to_string_lossy().to_string()),
            )
        })
        .collect();
    let mut bwrap = tokio::process::Command::new("bwrap");
    bwrap.args([
        "--unshare-all",
        "--share-net",
        "--die-with-parent",
        "--new-session",
    ]);
    bwrap.args(["--ro-bind", "/", "/"]);

    if Path::new("/tmp").exists() {
        bwrap.args(["--bind", "/tmp", "/tmp"]);
        for sockdir in ["/tmp/.X11-unix", "/tmp/.ICE-unix"] {
            if Path::new(sockdir).exists() {
                bwrap.args(["--tmpfs", sockdir]);
            }
        }
    } else {
        bwrap.args(["--tmpfs", "/tmp"]);
    }
    bwrap.args(["--proc", "/proc"]);
    bwrap.args(["--dev", "/dev"]);
    bwrap.arg("--chdir");
    bwrap.arg(workdir);

    if Path::new(workdir).exists() {
        bwrap.args(["--bind", workdir, workdir]);
    }
    let log_base = std::env::var("XDG_DATA_HOME")
        .map(|p| format!("{p}/vioraharness/logs"))
        .unwrap_or_else(|_| {
            format!(
                "{}/.local/share/vioraharness/logs",
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
            )
        });
    let _ = std::fs::create_dir_all(&log_base);
    bwrap.args(["--bind", &log_base, &log_base]);

    if let Ok(home) = std::env::var("HOME") {
        let cargo_home = format!("{home}/.cargo");
        if Path::new(&cargo_home).exists() {
            bwrap.args(["--ro-bind-try", &cargo_home, &cargo_home]);
        }
        let config_home = format!("{home}/.config");
        if Path::new(&config_home).exists() {
            bwrap.args(["--ro-bind-try", &config_home, &config_home]);
        }
    }

    if let Some(dir) = viora_state_dir() {
        bwrap.args(["--bind", &dir, &dir]);
    }

    for (k, v) in envs {
        if let Some(val) = v {
            bwrap.env(k, val);
        } else {
            bwrap.env_remove(k);
        }
    }
    bwrap.arg("--");
    bwrap.arg(&program);
    for arg in args {
        bwrap.arg(arg);
    }
    bwrap.kill_on_drop(true);
    *base = bwrap;
    tracing::info!(
        "sandbox: wrapped {} with bwrap for workdir {}",
        program,
        workdir
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viora_state_dir_shape() {
        let d = viora_state_dir().expect("HOME is set in test env");
        assert!(d.ends_with(".local/share/viora"), "{d}");
        assert!(Path::new(&d).exists(), "created on demand");
    }

    #[test]
    fn wrap_includes_viora_state_bind() {
        let mut cmd = tokio::process::Command::new("true");
        wrap_command(&mut cmd, "/tmp");
        let dbg = format!("{:?}", cmd.as_std());
        assert!(
            dbg.contains(".local/share/viora"),
            "viora state dir bound writable: {dbg}"
        );
    }
}
