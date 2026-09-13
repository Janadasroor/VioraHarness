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

    let mut envs: Vec<(String, Option<String>)> = base
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
        // Bind (don't mask) the X11/ICE socket dirs: the sandbox shares
        // the host net namespace, so abstract X sockets already work —
        // masking only the fs path breaks X clients without adding
        // isolation. ro-bind still prevents writes outside the sockets.
        for sockdir in ["/tmp/.X11-unix", "/tmp/.ICE-unix"] {
            bwrap.args(["--ro-bind-try", sockdir, sockdir]);
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

    let mut xauth_shim: Option<String> = None;
    if let Ok(home) = std::env::var("HOME") {
        let cargo_home = format!("{home}/.cargo");
        if Path::new(&cargo_home).exists() {
            bwrap.args(["--ro-bind-try", &cargo_home, &cargo_home]);
        }
        let config_home = format!("{home}/.config");
        if Path::new(&config_home).exists() {
            bwrap.args(["--ro-bind-try", &config_home, &config_home]);
        }
        // Sandboxed Chrome gets a throwaway profile shadowing the real
        // one (later binds win): the read-only real profile otherwise
        // spams crashpad/NSS write errors, and sharing it would expose
        // the user's cookies/sessions to agent-driven Chrome.
        let chrome_profile = format!("{config_home}/google-chrome");
        if Path::new(&chrome_profile).exists() {
            let shim = "/tmp/vioraharness-chrome-home/google-chrome";
            if std::fs::create_dir_all(shim).is_ok() {
                bwrap.args(["--bind-try", shim, &chrome_profile]);
            }
        }
        // Same treatment for the NSS cert database Chrome prods at
        // startup: throwaway writable copy, real one stays untouched.
        let nss_db = format!("{home}/.pki/nssdb");
        if Path::new(&nss_db).exists() {
            let shim = "/tmp/vioraharness-chrome-home/nssdb";
            if std::fs::create_dir_all(shim).is_ok() {
                bwrap.args(["--bind-try", shim, &nss_db]);
            }
        }
        // Writable Xauthority copy: X clients must lock the authority
        // file, which fails on the read-only host bind. The copy keeps
        // the same cookie, mode 0600. Empty counts as unset (hosts
        // without XAUTHORITY in env still use ~/.Xauthority by default).
        // Propagated via explicit --setenv so the value is guaranteed
        // inside even when the parent env is scrubbed; the process env
        // is set too for bwrap's own inheritance path.
        let xauth_src = std::env::var("XAUTHORITY")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("{home}/.Xauthority"));
        if Path::new(&xauth_src).exists() {
            let shim_dir = "/tmp/vioraharness-xauth";
            let shim = format!("{shim_dir}/Xauthority");
            if std::fs::create_dir_all(shim_dir).is_ok() && std::fs::copy(&xauth_src, &shim).is_ok()
            {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o600));
                }
                for (k, v) in envs.iter_mut() {
                    if k == "XAUTHORITY" {
                        *v = Some(shim.clone());
                    }
                }
                if !envs.iter().any(|(k, _)| k == "XAUTHORITY") {
                    envs.push(("XAUTHORITY".into(), Some(shim.clone())));
                }
                xauth_shim = Some(shim);
            }
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
    // Explicit --setenv guarantees the values inside regardless of how
    // the parent env was scrubbed; process-env inheritance alone is
    // fragile (empty host XAUTHORITY reads as "no cookie" to X clients).
    if let Some(shim) = xauth_shim {
        bwrap.args([
            "--bind-try",
            "/tmp/vioraharness-xauth",
            "/tmp/vioraharness-xauth",
        ]);
        bwrap.args(["--setenv", "XAUTHORITY", &shim]);
    }
    if let Ok(display) = std::env::var("DISPLAY") {
        if !display.trim().is_empty() {
            bwrap.args(["--setenv", "DISPLAY", &display]);
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
    fn x11_socket_dir_is_bound_not_masked() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cmd = tokio::process::Command::new("true");
        wrap_command(&mut cmd, "/tmp");
        let dbg = format!("{:?}", cmd.as_std());
        assert!(dbg.contains(".X11-unix"), "X11 socket dir forwarded: {dbg}");
        assert!(
            !dbg.contains("tmpfs"),
            "no tmpfs masking of socket dirs: {dbg}"
        );
    }

    #[test]
    fn xauthority_shim_points_at_writable_copy() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("XAUTHORITY").ok();
        let dir = std::env::temp_dir().join(format!("vh-xauth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("Xauthority");
        std::fs::write(&src, b"cookie-data").unwrap();
        std::env::set_var("XAUTHORITY", &src);
        let mut cmd = tokio::process::Command::new("true");
        wrap_command(&mut cmd, "/tmp");
        let dbg = format!("{:?}", cmd.as_std());
        match prev {
            Some(v) => std::env::set_var("XAUTHORITY", v),
            None => std::env::remove_var("XAUTHORITY"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            dbg.contains("/tmp/vioraharness-xauth/Xauthority"),
            "XAUTHORITY rewritten to writable copy: {dbg}"
        );
        assert!(
            dbg.contains("--setenv"),
            "XAUTHORITY pinned via explicit --setenv, not just process env: {dbg}"
        );
        let shim = std::path::Path::new("/tmp/vioraharness-xauth/Xauthority");
        assert_eq!(std::fs::read(shim).unwrap(), b"cookie-data");
    }

    #[test]
    fn empty_xauthority_falls_back_to_home_file() {
        // Hosts without XAUTHORITY in env (GDM default) still use
        // ~/.Xauthority — an empty string must not become the source path.
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("XAUTHORITY").ok();
        let home = std::env::var("HOME").expect("HOME set in test env");
        let home_xauth = std::path::Path::new(&home).join(".Xauthority");
        if !home_xauth.exists() {
            match prev {
                Some(v) => std::env::set_var("XAUTHORITY", v),
                None => std::env::remove_var("XAUTHORITY"),
            }
            return;
        }
        std::env::set_var("XAUTHORITY", "");
        let mut cmd = tokio::process::Command::new("true");
        wrap_command(&mut cmd, "/tmp");
        let dbg = format!("{:?}", cmd.as_std());
        match prev {
            Some(v) => std::env::set_var("XAUTHORITY", v),
            None => std::env::remove_var("XAUTHORITY"),
        }
        assert!(
            dbg.contains("/tmp/vioraharness-xauth/Xauthority"),
            "empty XAUTHORITY still shims ~/.Xauthority: {dbg}"
        );
    }

    #[test]
    fn wrap_includes_viora_state_bind() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut cmd = tokio::process::Command::new("true");
        wrap_command(&mut cmd, "/tmp");
        let dbg = format!("{:?}", cmd.as_std());
        assert!(
            dbg.contains(".local/share/viora"),
            "viora state dir bound writable: {dbg}"
        );
    }
}
