//! Desktop notifications for moments that need the user's eyes:
//! turn done/failed, permission/question prompts, background task
//! completions.
//!
//! Backend is `notify-send` (libnotify) when present, else a terminal
//! bell. Delivery runs on a short-lived thread so a D-Bus stall never
//! blocks the UI loop. Gated by `tui.notifications` (default on) with
//! a `VIORAHARNESS_NOTIFY` env override. Compiled to a no-op in unit
//! tests.

use super::App;

/// Resolve the notifications switch: explicit
/// `VIORAHARNESS_NOTIFY` (`0/off/false/no` or `1/on/true/yes`) wins,
/// then config `tui.notifications`, default on.
pub(crate) fn resolve_notifications() -> bool {
    if let Ok(v) = std::env::var("VIORAHARNESS_NOTIFY") {
        match v.trim().to_ascii_lowercase().as_str() {
            "0" | "off" | "false" | "no" => return false,
            "1" | "on" | "true" | "yes" => return true,
            _ => {}
        }
    }
    resolve_notifications_from(&vioraharness_core::loop_mod::config_candidates())
}

/// Config-file layer, separated for hermetic tests: first parseable
/// file with a `tui` section wins, missing key means on.
pub(crate) fn resolve_notifications_from(cands: &[String]) -> bool {
    for cand in cands {
        if let Ok(s) = std::fs::read_to_string(cand) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(tui) = v.get("tui") {
                    return tui
                        .get("notifications")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(true);
                }
            }
        }
    }
    true
}

/// Strip control characters and cap length so a huge model output
/// cannot spam the notification daemon.
fn sanitize_notify(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect()
}

#[cfg(not(test))]
fn have_notify_send() -> bool {
    static PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let bin = dir.join("notify-send");
                #[cfg(unix)]
                {
                    std::fs::metadata(&bin).is_ok_and(|m| {
                        use std::os::unix::fs::PermissionsExt;
                        m.is_file() && m.permissions().mode() & 0o111 != 0
                    })
                }
                #[cfg(not(unix))]
                {
                    let _ = bin;
                    false
                }
            })
        })
    })
}

#[cfg(not(test))]
fn terminal_bell() {
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x07");
    let _ = std::io::stdout().flush();
}

/// Raw sender: `notify-send` when available, terminal bell otherwise.
/// Fire-and-forget on a short thread; never blocks the caller.
pub(crate) fn desktop_notify(summary: &str, body: &str, urgent: bool) {
    #[cfg(test)]
    {
        let _ = (summary, body, urgent);
    }
    #[cfg(not(test))]
    {
        let summary = sanitize_notify(summary, 120);
        let body = sanitize_notify(body, 500);
        if summary.is_empty() && body.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("viora-notify".into())
            .spawn(move || {
                if have_notify_send() {
                    let mut cmd = std::process::Command::new("notify-send");
                    cmd.arg("-a").arg("VioraHarness");
                    if urgent {
                        cmd.arg("-u").arg("critical");
                    }
                    cmd.arg("--").arg(&summary).arg(&body);
                    cmd.stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null());
                    match cmd.spawn().and_then(|mut c| c.wait()) {
                        Ok(_) => {}
                        Err(_) => terminal_bell(),
                    }
                } else {
                    terminal_bell();
                }
            });
    }
}

impl App {
    /// User-facing notify: summary names the chat (window-title shape),
    /// body carries the event. Silent when the switch is off.
    pub(crate) fn notify(&self, body: &str, urgent: bool) {
        if !self.notifications {
            return;
        }
        let summary = super::window_title::session_window_title(
            &self.session_id,
            self.session_title.as_deref(),
        );
        desktop_notify(&summary, body, urgent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(dir: &std::path::Path, name: &str, json: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, json).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vh_notify_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sanitize_caps_and_strips() {
        assert_eq!(sanitize_notify("a\x1bb\nc", 100), "abc");
        assert_eq!(sanitize_notify(&"x".repeat(600), 500).len(), 500);
    }

    #[test]
    fn resolve_from_first_tui_section_wins() {
        let dir = temp_dir("order");
        let off = write_config(&dir, "off.json", r#"{"tui": {"notifications": false}}"#);
        let on = write_config(&dir, "on.json", r#"{"tui": {"notifications": true}}"#);
        let bare = write_config(&dir, "bare.json", r#"{}"#);
        assert!(!resolve_notifications_from(&[off.clone(), on.clone()]));
        assert!(resolve_notifications_from(&[on.clone(), off.clone()]));
        assert!(
            resolve_notifications_from(&[bare.clone(), on.clone()]),
            "no tui → skip"
        );
        assert!(
            resolve_notifications_from(&["/nonexistent-dir-xyz/vh.json".into(), on.clone()]),
            "missing file → skip"
        );
        assert!(resolve_notifications_from(&[]), "nothing → default on");
        assert!(resolve_notifications_from(&[bare]), "tui-less config → on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_overrides_config_both_ways() {
        let dir = temp_dir("env");
        let cfg_on = write_config(&dir, "on.json", r#"{"tui": {"notifications": true}}"#);
        let cfg_off = write_config(&dir, "off.json", r#"{"tui": {"notifications": false}}"#);
        // Env layer needs VIORAHARNESS_CONFIG pointing at a controlled
        // file (candidate 0), so the file layer is deterministic.
        let clock = crate::app::testkit::DB_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_cfg = std::env::var("VIORAHARNESS_CONFIG").ok();
        let prev_var = std::env::var("VIORAHARNESS_NOTIFY").ok();
        std::env::set_var("VIORAHARNESS_CONFIG", &cfg_on);
        std::env::set_var("VIORAHARNESS_NOTIFY", "0");
        assert!(!resolve_notifications(), "env off beats config on");
        std::env::set_var("VIORAHARNESS_CONFIG", &cfg_off);
        std::env::set_var("VIORAHARNESS_NOTIFY", "yes");
        assert!(resolve_notifications(), "env on beats config off");
        std::env::set_var("VIORAHARNESS_NOTIFY", "bogus");
        assert!(!resolve_notifications(), "bad env falls back to config");
        match prev_var {
            Some(v) => std::env::set_var("VIORAHARNESS_NOTIFY", v),
            None => std::env::remove_var("VIORAHARNESS_NOTIFY"),
        }
        match prev_cfg {
            Some(v) => std::env::set_var("VIORAHARNESS_CONFIG", v),
            None => std::env::remove_var("VIORAHARNESS_CONFIG"),
        }
        drop(clock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notify_respects_switch_and_never_panics() {
        let mut app = crate::app::testkit::test_app();
        app.notifications = false;
        app.notify("suppressed", false);
        app.notifications = true;
        app.notify("delivered (no-op in tests)", true);
    }
}
