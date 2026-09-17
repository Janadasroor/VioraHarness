// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use tokio::process::Command as TokioCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgStatus {
    Running,
    Done,
    Error,
    Killed,
}

impl BgStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BgStatus::Running => "running",
            BgStatus::Done => "done",
            BgStatus::Error => "error",
            BgStatus::Killed => "killed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BgTask {
    pub id: String,
    pub command: String,
    pub cwd: String,
    /// Agent mode that spawned the task (origin tag — kept for the task's
    /// lifetime so mode switches never orphan or misattribute it).
    pub mode: String,
    pub status: BgStatus,
    pub exit_code: Option<i32>,
    pub log_path: String,
    pub started_at: i64,
    pub pid: Option<u32>,
}

struct BgEntry {
    meta: BgTask,
    child: Option<tokio::process::Child>,

    notified: bool,
}

static REGISTRY: LazyLock<Mutex<HashMap<String, BgEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const MAX_KEPT: usize = 50;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    // Nanos alone collide under parallel spawns (two threads, same
    // nanosecond → same key → registry entries overwrite each other).
    static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("task_{:x}{:04x}", nanos & 0xffffffffffffff, c & 0xffff)
}

pub fn log_path_for(id: &str) -> String {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        })
        .join("vioraharness/logs");
    let _ = std::fs::create_dir_all(&base);
    base.join(format!("{id}.log")).to_string_lossy().to_string()
}

pub fn spawn_task(command: &str, cwd: &str) -> BgTask {
    spawn_task_opts(command, cwd, true)
}

/// Like [`spawn_task`], but `sandboxed=false` skips the bwrap wrapper with
/// the caller's notice. Reserved for processes that are isolation
/// boundaries themselves (emulator = KVM VM, Gradle = needs SDK/caches)
/// where strict sandboxing would break hardware access or the build.
pub fn spawn_task_opts(command: &str, cwd: &str, sandboxed: bool) -> BgTask {
    let id = new_id();
    let log_path = log_path_for(&id);

    let _ = std::fs::write(&log_path, format!("$ {command}\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
    }
    let mut cmd = TokioCommand::new("bash");
    cmd.args(["-c", command]);
    cmd.current_dir(cwd);
    cmd.env("QT_QPA_PLATFORM", "offscreen");

    if sandboxed {
        crate::sandbox::apply_sandbox(&mut cmd, cwd);
    } else {
        tracing::warn!("background task {id} spawned without sandbox: {command}");
    }

    let log_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .ok();
    let log_file2 = log_file.as_ref().and_then(|f| f.try_clone().ok());
    if let Some(f) = log_file {
        cmd.stdout(std::process::Stdio::from(f));
    } else {
        cmd.stdout(std::process::Stdio::null());
    }
    if let Some(f) = log_file2 {
        cmd.stderr(std::process::Stdio::from(f));
    } else {
        cmd.stderr(std::process::Stdio::null());
    }

    let child = cmd.spawn().ok();
    let pid = child.as_ref().and_then(|c| c.id());
    let meta = BgTask {
        id: id.clone(),
        command: command.to_string(),
        cwd: cwd.to_string(),
        mode: crate::mode::ModeGuard::current(),
        status: BgStatus::Running,
        exit_code: None,
        log_path: log_path.clone(),
        started_at: now_secs(),
        pid,
    };
    {
        let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());

        if reg.len() >= MAX_KEPT {
            let mut done: Vec<(i64, String)> = reg
                .iter()
                .filter(|(_, e)| e.meta.status != BgStatus::Running)
                .map(|(k, e)| (e.meta.started_at, k.clone()))
                .collect();
            done.sort();
            let over = reg.len().saturating_sub(MAX_KEPT) + 1;
            for (_, key) in done.into_iter().take(over) {
                reg.remove(&key);
            }
        }
        reg.insert(
            id.clone(),
            BgEntry {
                meta: meta.clone(),
                child,
                notified: false,
            },
        );
    }

    tokio::spawn(async move {
        let code: Option<i32> = loop {
            let done = {
                let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
                match reg.get_mut(&id) {
                    Some(e) => match e.child.as_mut() {
                        Some(c) => match c.try_wait() {
                            Ok(Some(st)) => Some(st.code()),
                            Ok(None) => None,
                            Err(_) => Some(None),
                        },
                        None => Some(None),
                    },
                    None => return,
                }
            };
            match done {
                Some(code) => break code,
                None => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            }
        };
        let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = reg.get_mut(&id) {
            if entry.meta.status == BgStatus::Running {
                match code {
                    Some(0) => {
                        entry.meta.status = BgStatus::Done;
                        entry.meta.exit_code = Some(0);
                    }
                    other => {
                        entry.meta.status = BgStatus::Error;
                        entry.meta.exit_code = other;
                    }
                }
            }
            entry.child = None;
        }
    });
    meta
}

pub fn list_tasks() -> Vec<BgTask> {
    let reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<BgTask> = reg.values().map(|e| e.meta.clone()).collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    out
}

pub fn get_task(id: &str) -> Option<BgTask> {
    REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| e.meta.clone())
}

pub fn kill_task(id: &str) -> bool {
    let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(entry) = reg.get_mut(id) else {
        return false;
    };
    if entry.meta.status != BgStatus::Running {
        return false;
    }
    entry.meta.status = BgStatus::Killed;
    if let Some(child) = entry.child.as_mut() {
        let _ = child.start_kill();
    }
    true
}

pub fn take_completions() -> Vec<BgTask> {
    let mut reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for entry in reg.values_mut() {
        if entry.meta.status != BgStatus::Running && !entry.notified {
            entry.notified = true;
            out.push(entry.meta.clone());
        }
    }
    out.sort_by_key(|t| t.started_at);
    out
}

pub fn read_task_log(id: &str) -> Option<String> {
    let path = REGISTRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| e.meta.log_path.clone())?;
    let bytes = std::fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&bytes).to_string();
    const CAP: usize = 100 * 1024;
    if text.len() > CAP {
        let mut start = text.len() - CAP;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        Some(format!(
            "… ({} bytes total, showing last {}KB)\n{}",
            text.len(),
            CAP / 1024,
            &text[start..]
        ))
    } else {
        Some(text)
    }
}

pub fn launch_result(task: &BgTask) -> Value {
    json!({
        "ok": true,
        "background": true,
        "task_id": task.id,
        "status": task.status.as_str(),
        "mode": task.mode,
        "log": task.log_path,
        "note": format!("detached task {} running in mode '{}' — result will NOT return here; check /tasks (or poll the log) for completion, then read/grep the log", task.id, task.mode),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn wait_for(id: &str, timeout_ms: u64) -> BgTask {
        let start = std::time::Instant::now();
        loop {
            if let Some(t) = get_task(id) {
                if t.status != BgStatus::Running {
                    return t;
                }
            }
            if start.elapsed().as_millis() > timeout_ms as u128 {
                panic!("task {id} still running after {timeout_ms}ms");
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn spawn_tags_origin_mode() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(crate::mode::MODE_ENV_VAR).ok();
        std::env::set_var(crate::mode::MODE_ENV_VAR, "web");
        let t = spawn_task("true", "/tmp");
        let done = wait_for(&t.id, 5000).await;
        assert_eq!(done.mode, "web", "task keeps its origin mode");
        assert_eq!(launch_result(&done)["mode"], "web");
        match prev {
            Some(v) => std::env::set_var(crate::mode::MODE_ENV_VAR, v),
            None => std::env::remove_var(crate::mode::MODE_ENV_VAR),
        }
    }

    #[tokio::test]
    async fn spawn_runs_detached_with_live_log() {
        let t = spawn_task("echo hello-bg && echo err-line >&2", "/tmp");
        assert_eq!(t.status, BgStatus::Running);
        assert!(t.id.starts_with("task_"));
        let done = wait_for(&t.id, 5000).await;
        assert_eq!(done.status, BgStatus::Done);
        assert_eq!(done.exit_code, Some(0));
        let log = read_task_log(&t.id).expect("log readable");
        assert!(log.contains("hello-bg"), "stdout in log: {log:?}");
        assert!(log.contains("err-line"), "stderr in log: {log:?}");

        let first = take_completions();
        assert!(first.iter().any(|x| x.id == t.id));
        let second = take_completions();
        assert!(!second.iter().any(|x| x.id == t.id), "no repeat notice");
    }

    #[tokio::test]
    async fn bash_background_arg_detaches() {
        let r = crate::tools::bash::bash(serde_json::json!({
            "command": "echo bg-via-bash",
            "background": true,
        }))
        .await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["background"], true);
        let tid = r["task_id"].as_str().expect("task id").to_string();
        let done = wait_for(&tid, 5000).await;
        assert_eq!(done.status, BgStatus::Done);
        let log = read_task_log(&tid).expect("log");
        assert!(log.contains("bg-via-bash"), "{log:?}");

        let r = crate::tools::bash::bash(serde_json::json!({
            "command": "rm -rf /",
            "background": true,
        }))
        .await;
        assert_eq!(r["ok"], false);
        assert!(list_tasks().iter().all(|t| t.command != "rm -rf /"));
    }

    #[tokio::test]
    async fn failing_command_reports_error() {
        let t = spawn_task("exit 3", "/tmp");
        let done = wait_for(&t.id, 5000).await;
        assert_eq!(done.status, BgStatus::Error);
        assert_eq!(done.exit_code, Some(3));
    }

    #[tokio::test]
    async fn kill_stops_running_task() {
        let t = spawn_task("sleep 30", "/tmp");
        assert!(kill_task(&t.id), "kill accepted");
        let done = wait_for(&t.id, 5000).await;
        assert_eq!(done.status, BgStatus::Killed, "killed wins over exit");
        assert!(!kill_task(&t.id), "second kill refused (not running)");
        assert!(get_task("nope").is_none());
        assert!(!kill_task("nope"));
        assert!(read_task_log("nope").is_none());
    }

    #[test]
    fn registry_lists_newest_first() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let a = spawn_task("true", "/tmp");
            let _ = wait_for(&a.id, 5000).await;
        });
        let list = list_tasks();
        assert!(!list.is_empty());
        for w in list.windows(2) {
            assert!(
                (w[0].started_at, &w[0].id) >= (w[1].started_at, &w[1].id),
                "newest first"
            );
        }
    }

    #[tokio::test]
    async fn multibyte_log_tail_never_panics() {
        let t = spawn_task("python3 -c \"print('\u{2713}'*50000)\"", "/tmp");
        let done = wait_for(&t.id, 15000).await;
        assert_eq!(done.status, BgStatus::Done);
        let log = read_task_log(&t.id).expect("log");
        assert!(
            log.contains("showing last 100KB"),
            "tail note: {}",
            &log[..80]
        );
        assert!(log.ends_with('\u{2713}') || log.ends_with('\n'));
    }

    #[tokio::test]
    async fn registry_evicts_oldest_finished_first() {
        // Invariant: finished entries stay bounded

        let mut mine = Vec::new();
        for i in 0..60 {
            let t = spawn_task(&format!("echo evict-batch-{i}"), "/tmp");
            mine.push(t.id.clone());
            let _ = wait_for(&t.id, 5000).await;
        }
        let list = list_tasks();
        let finished = list
            .iter()
            .filter(|t| t.status != BgStatus::Running)
            .count();
        assert!(
            finished <= 52,
            "finished bounded (running excluded): {finished}"
        );
        let mine_left = list.iter().filter(|t| mine.contains(&t.id)).count();
        assert!(
            mine_left <= 52,
            "oldest of the batch evicted first: {mine_left}/60 left"
        );
        assert!(
            mine_left < 60,
            "batch overflow evicted something: {mine_left}/60 left"
        );
    }
}
