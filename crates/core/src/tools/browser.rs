use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const CHROME_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];

const PROFILE_DIR: &str = "/tmp/vioraharness-chrome";
const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 800;
const DEFAULT_WAIT_MS: u64 = 2000;
const BROWSER_TIMEOUT_SECS: u64 = 60;
const DOM_DEFAULT_MAX_CHARS: usize = 30_000;
const DOM_HARD_MAX_CHARS: usize = 100_000;

pub fn chrome_candidates() -> Vec<String> {
    if let Ok(explicit) = std::env::var("CHROME_BIN") {
        let explicit = explicit.trim().to_string();
        if !explicit.is_empty() {
            return vec![explicit];
        }
    }
    CHROME_CANDIDATES.iter().map(|s| s.to_string()).collect()
}

pub fn find_chrome() -> Option<String> {
    for cand in chrome_candidates() {
        if cand.contains('/') {
            if Path::new(&cand).exists() {
                return Some(cand);
            }
            continue;
        }
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in path_var.split(':') {
                if dir.is_empty() {
                    continue;
                }
                let full = Path::new(dir).join(&cand);
                if full.exists() {
                    return Some(full.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

fn ensure_profile_dir() {
    let _ = std::fs::create_dir_all(PROFILE_DIR);
}

/// Resolve a browser target to the URL passed to headless Chrome plus a
/// short display string. Accepts `http(s)://`, `file://`, or a local path
/// (ROOT-jailed like `read`).
pub fn resolve_target(target: &str) -> Result<(String, String), String> {
    let t = target.trim();
    if t.is_empty() {
        return Err("missing target (http(s) URL or local .html path)".into());
    }
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return match super::web::validate_url(t) {
            Ok(u) => Ok((u.clone(), u)),
            Err(e) => Err(e),
        };
    }
    if lower.starts_with("file://") {
        let path_part = &t[7..];
        return resolve_file_target(path_part);
    }
    if t.contains("://") {
        let scheme = t.split("://").next().unwrap_or("");
        return Err(format!(
            "unsupported scheme '{scheme}://' (use http(s):// or a local path)"
        ));
    }
    resolve_file_target(t)
}

fn resolve_file_target(path_part: &str) -> Result<(String, String), String> {
    if path_part.trim().is_empty() {
        return Err("missing file path".into());
    }
    let path = super::viora::resolve_path(path_part);
    if !super::viora::approved_call() && !super::viora::is_within_root(&path) {
        return Err(format!(
            "access denied: {} outside project root (approve in the Ask dialog or run with -y)",
            path.display()
        ));
    }
    if !path.exists() {
        return Err(format!("file not found: {path_part}"));
    }
    let url = format!("file://{}", path.display());
    Ok((url, path_part.to_string()))
}

fn clamp_u64(v: u64, lo: u64, hi: u64) -> u64 {
    v.clamp(lo, hi)
}

fn default_out(suffix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("/tmp/vioraharness_browser_{nanos}{suffix}")
}

fn check_out_path(out: &str) -> Result<PathBuf, Value> {
    if out.trim().is_empty() {
        return Err(json!({"ok": false, "error": "missing out path"}));
    }
    let p = super::viora::resolve_path(out);
    if !super::viora::approved_call() && !super::viora::is_within_root(&p) {
        return Err(
            json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y)", p.display())}),
        );
    }
    Ok(p)
}

fn run_args_screenshot(out: &str, width: u32, height: u32, wait_ms: u64, url: &str) -> Vec<String> {
    vec![
        "--headless=new".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--disable-dev-shm-usage".into(),
        format!("--user-data-dir={PROFILE_DIR}"),
        "--hide-scrollbars".into(),
        format!("--window-size={width},{height}"),
        format!("--screenshot={out}"),
        format!("--virtual-time-budget={wait_ms}"),
        "--timeout=30000".into(),
        url.to_string(),
    ]
}

fn run_args_dom(wait_ms: u64, url: &str) -> Vec<String> {
    vec![
        "--headless=new".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--disable-dev-shm-usage".into(),
        format!("--user-data-dir={PROFILE_DIR}"),
        "--dump-dom".into(),
        format!("--virtual-time-budget={wait_ms}"),
        url.to_string(),
    ]
}

fn run_args_pdf(out: &str, url: &str) -> Vec<String> {
    vec![
        "--headless=new".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--disable-dev-shm-usage".into(),
        format!("--user-data-dir={PROFILE_DIR}"),
        format!("--print-to-pdf={out}"),
        "--no-pdf-header-footer".into(),
        url.to_string(),
    ]
}

async fn run_chrome(args: &[String]) -> Result<std::process::Output, String> {
    let exe = find_chrome().ok_or_else(|| {
        "chrome not found (install google-chrome or chromium, or set CHROME_BIN)".to_string()
    })?;
    ensure_profile_dir();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.args(args);
    cmd.current_dir(&cwd);
    cmd.kill_on_drop(true);
    super::super::sandbox::apply_sandbox(&mut cmd, &cwd);
    match tokio::time::timeout(
        std::time::Duration::from_secs(BROWSER_TIMEOUT_SECS),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("failed to spawn chrome ({exe}): {e}")),
        Err(_) => Err(format!("chrome timed out after {BROWSER_TIMEOUT_SECS}s")),
    }
}

fn is_local_url(url: &str) -> bool {
    url.starts_with("file://")
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1")
        || url.starts_with("https://localhost")
        || url.starts_with("https://127.0.0.1")
}

fn truncate_chars_capped(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    (s.chars().take(max).collect(), true)
}

pub async fn browser_screenshot(args: Value) -> Value {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let (url, display) = match resolve_target(target) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let out_defaulted = args.get("out").and_then(|v| v.as_str()).is_none();
    let out_str = args
        .get("out")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_out(".png"));
    let out_path = match check_out_path(&out_str) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let width = args
        .get("width")
        .and_then(|v| v.as_u64())
        .map(|v| clamp_u64(v, 320, 3840) as u32)
        .unwrap_or(DEFAULT_WIDTH);
    let height = args
        .get("height")
        .and_then(|v| v.as_u64())
        .map(|v| clamp_u64(v, 240, 2160) as u32)
        .unwrap_or(DEFAULT_HEIGHT);
    let wait_ms = args
        .get("delay_ms")
        .or_else(|| args.get("wait_ms"))
        .and_then(|v| v.as_u64())
        .map(|v| clamp_u64(v, 0, 15000))
        .unwrap_or(DEFAULT_WAIT_MS);

    if let Some(parent) = out_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return json!({"ok": false, "error": format!("create out dir: {e}")});
        }
    }
    let out_cli = out_path.to_string_lossy().to_string();
    let cargs = run_args_screenshot(&out_cli, width, height, wait_ms, &url);
    if let Err(e) = run_chrome(&cargs).await {
        return json!({"ok": false, "error": e, "target": display});
    }
    let bytes = match tokio::fs::read(&out_path).await {
        Ok(b) => b,
        Err(e) => {
            return json!({"ok": false, "error": format!("screenshot missing (chrome wrote nothing to {out_cli}: {e})"), "target": display});
        }
    };
    if bytes.is_empty() {
        return json!({"ok": false, "error": format!("screenshot empty: {out_cli}"), "target": display});
    }
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    let b64 = BASE64.encode(&bytes);
    let mut res = json!({
        "ok": true,
        "target": display,
        "out": out_cli,
        "width": width,
        "height": height,
        "bytes": bytes.len(),
        "base64": b64,
        "base64_len": b64.len(),
        "note": "screenshot attached as vision (base64 stripped from text) — describe what you see, don't dump pixels"
    });
    // Local targets with default out: also drop a copy at
    // ./screenshot-latest.png so web loops don't need a manual cp.
    // Best-effort — a copy failure never fails the screenshot.
    if out_defaulted && is_local_url(&url) {
        let latest = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join("screenshot-latest.png");
        match tokio::fs::copy(&out_path, &latest).await {
            Ok(_) => {
                res["latest"] = json!(latest.to_string_lossy());
            }
            Err(e) => {
                res["latest_error"] = json!(format!(
                    "auto-copy to ./screenshot-latest.png failed: {e} (out kept at {out_cli})"
                ));
            }
        }
    }
    res
}

pub async fn browser_dom(args: Value) -> Value {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let (url, display) = match resolve_target(target) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let cap = args
        .get("max_chars")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).clamp(100, DOM_HARD_MAX_CHARS))
        .unwrap_or(DOM_DEFAULT_MAX_CHARS);
    let wait_ms = args
        .get("wait_ms")
        .or_else(|| args.get("delay_ms"))
        .and_then(|v| v.as_u64())
        .map(|v| clamp_u64(v, 0, 15000))
        .unwrap_or(DEFAULT_WAIT_MS);

    let cargs = run_args_dom(wait_ms, &url);
    let out = match run_chrome(&cargs).await {
        Ok(o) => o,
        Err(e) => return json!({"ok": false, "error": e, "target": display}),
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .next()
            .unwrap_or("chrome failed")
            .trim()
            .to_string();
        return json!({"ok": false, "error": first, "target": display});
    }
    let html = String::from_utf8_lossy(&out.stdout).to_string();
    if html.trim().is_empty() {
        return json!({"ok": false, "error": "chrome returned empty DOM", "target": display});
    }
    let text = super::web::html_to_text(&html);
    let total = text.chars().count();
    let (content, truncated) = truncate_chars_capped(&text, cap);
    json!({
        "ok": true,
        "target": display,
        "chars": total,
        "truncated": truncated,
        "html_chars": html.chars().count(),
        "text": content,
    })
}

/// Open a page in the user's visible host Chrome. Unlike the other
/// browser tools this deliberately runs OUTSIDE the bwrap sandbox (a
/// sandboxed window is invisible in another PID/mount namespace), so it
/// is Ask-gated: approve each launch in the dialog or run with -y.
pub async fn browser_open(args: Value) -> Value {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let (url, display) = match resolve_target(target) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    // Headless screenshot needs no X server, but a visible window does.
    // When the X socket is hidden (sandbox masks /tmp/.X11-unix, or no
    // DISPLAY at all) say so upfront instead of failing silently.
    let x11_hint = match std::env::var("DISPLAY").ok().filter(|s| !s.trim().is_empty()) {
        None => Some(
            "no DISPLAY in env (headless-only session?) — visible Chrome needs an X server; use browser_screenshot (headless) instead".to_string(),
        ),
        Some(d) => {
            let num = d.trim_start_matches(':').split('.').next().unwrap_or("");
            let sock = format!("/tmp/.X11-unix/X{num}");
            if !Path::new(&sock).exists() {
                Some(format!(
                    "X11 socket {sock} not visible for DISPLAY={d} — use browser_screenshot (headless) or the browser_open host launcher"
                ))
            } else {
                None
            }
        }
    };
    let exe = match find_chrome() {
        Some(e) => e,
        None => {
            return json!({"ok": false, "error": "chrome not found (install google-chrome or chromium, or set CHROME_BIN)"})
        }
    };
    let mut cmd = tokio::process::Command::new(&exe);
    cmd.arg("--new-window").arg(&url);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());
    cmd.kill_on_drop(false);
    match cmd.spawn() {
        Ok(child) => {
            let mut res = json!({
                "ok": true,
                "target": display,
                "pid": child.id(),
                "note": "opened in host Chrome (outside sandbox) for the user to see — verify with browser_screenshot, never xdotool windowclose"
            });
            if let Some(hint) = x11_hint {
                res["x11_hint"] = json!(hint);
            }
            res
        }
        Err(e) => {
            let mut res =
                json!({"ok": false, "error": format!("open {display}: {e}"), "target": display});
            if let Some(hint) = x11_hint {
                res["x11_hint"] = json!(hint);
            }
            res
        }
    }
}

pub async fn browser_pdf(args: Value) -> Value {
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("");
    let (url, display) = match resolve_target(target) {
        Ok(t) => t,
        Err(e) => return json!({"ok": false, "error": e}),
    };
    let out_str = args
        .get("out")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_out(".pdf"));
    let out_path = match check_out_path(&out_str) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = out_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return json!({"ok": false, "error": format!("create out dir: {e}")});
        }
    }
    let out_cli = out_path.to_string_lossy().to_string();
    let cargs = run_args_pdf(&out_cli, &url);
    if let Err(e) = run_chrome(&cargs).await {
        return json!({"ok": false, "error": e, "target": display});
    }
    let meta = tokio::fs::metadata(&out_path).await;
    match meta {
        Ok(m) if m.len() > 0 => json!({
            "ok": true,
            "target": display,
            "out": out_cli,
            "bytes": m.len(),
        }),
        _ => {
            json!({"ok": false, "error": format!("chrome wrote no PDF to {out_cli}"), "target": display})
        }
    }
}

fn pick_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    listener.local_addr().ok()?.port().into()
}

pub async fn dev_serve(args: Value) -> Value {
    let dir_str = args.get("dir").and_then(|v| v.as_str()).unwrap_or(".");
    let dir_path = super::viora::resolve_path(dir_str);
    if !super::viora::approved_call() && !super::viora::is_within_root(&dir_path) {
        return json!({"ok": false, "error": format!("access denied: {} outside project root (approve in the Ask dialog or run with -y)", dir_path.display())});
    }
    if !dir_path.is_dir() {
        return json!({"ok": false, "error": format!("not a directory: {dir_str}")});
    }
    let port = match args.get("port").and_then(|v| v.as_u64()) {
        Some(0) | None => match pick_free_port() {
            Some(p) => p,
            None => return json!({"ok": false, "error": "no free port available"}),
        },
        Some(p) if (1..=65535).contains(&p) => p as u16,
        Some(p) => return json!({"ok": false, "error": format!("invalid port: {p}")}),
    };
    let cwd = dir_path.to_string_lossy().to_string();
    let command = format!("python3 -m http.server {port}");
    let task = super::tasks::spawn_task(&command, &cwd);
    let mut res = super::tasks::launch_result(&task);
    res["url"] = json!(format!("http://127.0.0.1:{port}/"));
    res["dir"] = json!(dir_str);
    res["port"] = json!(port);
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chrome_arg_shapes() {
        let a = run_args_screenshot("/tmp/o.png", 1280, 800, 2000, "https://example.com");
        assert!(a.iter().any(|x| x == "--screenshot=/tmp/o.png"));
        assert!(a.iter().any(|x| x == "--window-size=1280,800"));
        assert!(a.iter().any(|x| x == "--virtual-time-budget=2000"));
        assert!(a.iter().any(|x| x.contains("--user-data-dir=/tmp/")));
        assert_eq!(a.last().unwrap(), "https://example.com");

        let d = run_args_dom(1000, "https://example.com");
        assert!(d.contains(&"--dump-dom".to_string()));
        assert!(d.iter().any(|x| x == "--virtual-time-budget=1000"));

        let p = run_args_pdf("/tmp/o.pdf", "https://example.com");
        assert!(p.iter().any(|x| x == "--print-to-pdf=/tmp/o.pdf"));
    }

    #[test]
    fn target_validation() {
        assert!(resolve_target("").is_err());
        assert!(resolve_target("javascript:alert(1)").is_err());
        assert!(resolve_target("ftp://x/y").is_err());
        assert!(resolve_target("https://example.com/x").is_ok());
        assert!(resolve_target("http://127.0.0.1:8000/").is_ok());
    }

    #[test]
    fn chrome_candidates_prefers_env() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("CHROME_BIN").ok();
        std::env::set_var("CHROME_BIN", "/tmp/custom-chrome");
        assert_eq!(chrome_candidates(), vec!["/tmp/custom-chrome".to_string()]);
        match prev {
            Some(v) => std::env::set_var("CHROME_BIN", v),
            None => std::env::remove_var("CHROME_BIN"),
        }
        assert!(chrome_candidates().contains(&"google-chrome".to_string()));
    }

    #[tokio::test]
    async fn screenshot_rejects_bad_target_without_spawning() {
        let r = browser_screenshot(json!({"target": ""})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("missing target"),
            "{r}"
        );
    }

    #[tokio::test]
    async fn open_rejects_bad_target_without_spawning() {
        let r = browser_open(json!({"target": ""})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("missing target"),
            "{r}"
        );
        assert!(r.get("pid").is_none(), "nothing spawned: {r}");
    }

    #[tokio::test]
    async fn dom_rejects_bad_target_without_spawning() {
        let r = browser_dom(json!({"target": "ftp://x/y"})).await;
        assert_eq!(r["ok"], false, "{r}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn pdf_outside_root_denied() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        std::env::remove_var("VIORAHARNESS_APPROVED_CALL");
        let pid = std::process::id();
        let r = browser_pdf(
            json!({"target": "https://example.com", "out": format!("/tmp/../vh-outside-{pid}/o.pdf")}),
        )
        .await;
        assert_eq!(r["ok"], false, "{r}");
        assert!(
            r["error"]
                .as_str()
                .unwrap()
                .contains("outside project root"),
            "{r}"
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }

    #[tokio::test]
    async fn dev_serve_rejects_missing_dir() {
        let r = dev_serve(json!({"dir": "/tmp/vh-nope-dir-xyz-12345"})).await;
        assert_eq!(r["ok"], false);
        assert!(
            r["error"].as_str().unwrap().contains("not a directory"),
            "{r}"
        );
    }

    #[test]
    fn local_url_detection() {
        for u in [
            "file:///tmp/x.html",
            "http://localhost:5173/",
            "http://127.0.0.1:8000/a",
        ] {
            assert!(is_local_url(u), "{u}");
        }
        for u in ["https://example.com/", "http://192.168.1.50:3000/"] {
            assert!(!is_local_url(u), "{u}");
        }
    }

    #[test]
    fn truncate_chars_never_splits() {
        let s = "A".repeat(100) + "ｱ" + &"B".repeat(100);
        let (t, trunc) = truncate_chars_capped(&s, 100);
        assert_eq!(t, "A".repeat(100));
        assert!(trunc);
        let (t2, trunc2) = truncate_chars_capped("hi", 100);
        assert_eq!(t2, "hi");
        assert!(!trunc2);
    }
}
