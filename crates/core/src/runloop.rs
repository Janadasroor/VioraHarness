// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Task files: tasks/pending/<name>.md
//
// ---
// title: Short human title
// files:
//   - path/relative/to/repo.rs
//   - other/dir/
// validate:
//   - "cargo test -p mycrate"
// retries: 2
// ---
// Goal, acceptance criteria, notes. One focused session of work.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TaskSpec {
    pub name: String,
    pub title: String,
    /// In-scope files/dirs, relative to the workdir. Empty = whole tree.
    pub files: Vec<String>,
    /// Validation shell commands run after the agent finishes.
    pub validate: Vec<String>,
    /// Extra attempts after the first failure before quarantine.
    pub retries: u32,
    /// Markdown body: goal + acceptance criteria.
    pub body: String,
    pub path: PathBuf,
}

impl TaskSpec {
    pub fn runbook(&self) -> String {
        let scope = if self.files.is_empty() {
            "whole repository".to_string()
        } else {
            self.files.join(", ")
        };
        format!(
            "{}\n\n---\nRunbook (follow exactly):\n\
             1. Do ONLY this task. Stay inside: {scope}. Do not expand scope.\n\
             2. Do NOT commit anything yourself — the runner commits on green.\n\
             3. Append durable decisions to DECISIONS.md (one line each, with why).\n\
             4. Finish with a short summary of what changed and its validation output.\n",
            self.body.trim()
        )
    }
}

pub fn parse_task_file(path: &Path) -> Result<TaskSpec, String> {
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "task file needs a name".to_string())?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let (front, body) = split_frontmatter(&text)?;
    let mut title = String::new();
    let mut files = Vec::new();
    let mut validate = Vec::new();
    let mut retries = 1u32;
    let mut current: Option<&str> = None;
    for (no, raw) in front.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            match current {
                Some("files") => files.push(item.trim().to_string()),
                Some("validate") => validate.push(unquote(item.trim())),
                Some("title") | Some("retries") => {
                    return Err(format!("line {}: unexpected list item", no + 1))
                }
                _ => {
                    return Err(format!(
                        "line {}: list item outside files:/validate:",
                        no + 1
                    ))
                }
            }
        } else if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "title" | "files" | "validate" | "retries" => {
                    current = Some(match key {
                        "title" => "title",
                        "files" => "files",
                        "validate" => "validate",
                        "retries" => "retries",
                        _ => unreachable!("matched above"),
                    });
                    if key == "title" {
                        title = unquote(val);
                    } else if key == "retries" {
                        retries = val
                            .parse::<u32>()
                            .map_err(|_| format!("line {}: bad retries {val:?}", no + 1))?;
                    } else if !val.is_empty() && val != "[]" {
                        return Err(format!(
                            "line {}: {key} takes a list, one `- item` per line",
                            no + 1
                        ));
                    }
                    if val == "[]" {
                        current = None;
                    }
                }
                _ => return Err(format!("line {}: unknown key {key:?}", no + 1)),
            }
        } else {
            return Err(format!("line {}: cannot parse {line:?}", no + 1));
        }
    }
    if body.trim().is_empty() {
        return Err("task body is empty (goal + acceptance criteria go below ---)".into());
    }
    Ok(TaskSpec {
        title: if title.is_empty() {
            name.clone()
        } else {
            title
        },
        name,
        files: files.into_iter().filter(|f| !f.is_empty()).collect(),
        validate,
        retries,
        body: body.trim().to_string() + "\n",
        path: path.to_path_buf(),
    })
}

fn split_frontmatter(text: &str) -> Result<(String, String), String> {
    let mut lines = text.lines();
    match lines.next() {
        Some("---") => {}
        _ => return Ok((String::new(), text.to_string())),
    }
    let mut front = Vec::new();
    for line in lines.by_ref() {
        if line.trim() == "---" {
            return Ok((front.join("\n"), lines.collect::<Vec<_>>().join("\n")));
        }
        front.push(line);
    }
    Err("unclosed frontmatter: missing closing ---".into())
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Runner pieces (pure / process-level, model-free for testability)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    pub command: String,
    pub ok: bool,
    pub output: String,
}

pub const VALIDATE_TIMEOUT: Duration = Duration::from_secs(300);

pub fn run_validations(commands: &[String], cwd: &Path) -> Vec<ValidationResult> {
    commands
        .iter()
        .map(|c| {
            let out = std::process::Command::new("bash")
                .args(["-c", c])
                .current_dir(cwd)
                .env("QT_QPA_PLATFORM", "offscreen")
                .output();
            match out {
                Ok(o) => {
                    let mut text = String::from_utf8_lossy(&o.stdout).to_string();
                    let err = String::from_utf8_lossy(&o.stderr);
                    if !err.trim().is_empty() {
                        text.push_str("\n--- stderr ---\n");
                        text.push_str(&err);
                    }
                    ValidationResult {
                        command: c.clone(),
                        ok: o.status.success(),
                        output: text,
                    }
                }
                Err(e) => ValidationResult {
                    command: c.clone(),
                    ok: false,
                    output: format!("failed to spawn bash: {e}"),
                },
            }
        })
        .collect()
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| anyhow::anyhow!("git not available: {e}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Stage ONLY in-scope files and commit. Empty scope = whole tree.
/// Returns the commit hash, or a no-change note when nothing staged.
pub fn git_commit_scoped(cwd: &Path, files: &[String], message: &str) -> Result<String> {
    if files.is_empty() {
        git(cwd, &["add", "-A"])?;
    } else {
        let mut args = vec!["add", "--"];
        args.extend(files.iter().map(|f| f.as_str()));
        git(cwd, &args)?;
    }
    let staged = git(cwd, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok("(nothing to commit)".into());
    }
    git(cwd, &["commit", "-m", message])?;
    git(cwd, &["rev-parse", "--short", "HEAD"])
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    Done { session: String, commit: String },
    Failed { session: String, log: String },
}

pub struct BatchOptions {
    pub queue_dir: PathBuf,
    pub model: String,
    pub max_tasks: usize,
    pub token_budget: u64,
    pub workdir: PathBuf,
}

pub struct BatchSummary {
    pub done: Vec<(String, String)>,
    pub failed: Vec<String>,
    pub skipped_budget: Vec<String>,
    pub tokens_spent: u64,
}

#[async_trait::async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(
        &self,
        prompt: &str,
        model: &str,
        session: Option<String>,
    ) -> anyhow::Result<String>;
}

pub struct LoopExecutor {
    pub agent: crate::loop_mod::AgentLoop,
}

#[async_trait::async_trait]
impl TaskExecutor for LoopExecutor {
    async fn execute(
        &self,
        prompt: &str,
        model: &str,
        session: Option<String>,
    ) -> anyhow::Result<String> {
        self.agent.run(prompt, model, session).await
    }
}

pub fn list_pending(queue_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(queue_dir.join("pending")) {
        let mut names: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        names.sort();
        out = names;
    }
    out
}

fn file_move(from: &Path, to_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(to_dir)?;
    let name = from
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("task file has no name"))?;
    let to = to_dir.join(name);
    std::fs::rename(from, &to)?;
    Ok(to)
}

fn new_session_id(prefix: &str) -> String {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{ns:x}")
}

fn session_tokens(store: &crate::session::SessionStore, session: &str) -> u64 {
    store
        .get_session(session)
        .ok()
        .flatten()
        .map(|_| {
            store
                .export_jsonl(session)
                .map(|jl| {
                    jl.lines()
                        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                        .filter(|v| v["type"] == "message")
                        .map(|v| v["content"].as_str().unwrap_or("").len() as u64 / 4)
                        .sum()
                })
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Run the queue: one fresh session per task (full context reset),
/// validate → scoped commit → done/, retry then quarantine to failed/.
pub async fn run_batch<E: TaskExecutor>(
    opts: &BatchOptions,
    store: &crate::session::SessionStore,
    exec: &E,
) -> anyhow::Result<BatchSummary> {
    let pending = list_pending(&opts.queue_dir);
    if pending.is_empty() {
        println!("run-loop: queue empty ({}).", opts.queue_dir.display());
        return Ok(BatchSummary {
            done: vec![],
            failed: vec![],
            skipped_budget: vec![],
            tokens_spent: 0,
        });
    }
    let mut summary = BatchSummary {
        done: vec![],
        failed: vec![],
        skipped_budget: vec![],
        tokens_spent: 0,
    };
    let mut ran = 0usize;
    for path in pending {
        if ran >= opts.max_tasks {
            break;
        }
        if opts.token_budget > 0 && summary.tokens_spent >= opts.token_budget {
            println!("run-loop: token budget exhausted, stopping.");
            summary.skipped_budget.push(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
            continue;
        }
        let task = match parse_task_file(&path) {
            Ok(t) => t,
            Err(e) => {
                println!("run-loop: SKIP {} ({e})", path.display());
                continue;
            }
        };
        ran += 1;
        let outcome = run_one_task(opts, store, exec, &task).await;
        match outcome {
            TaskOutcome::Done { session, commit } => {
                let dest =
                    file_move(&path, &opts.queue_dir.join("done")).unwrap_or_else(|_| path.clone());
                println!("run-loop: DONE {} ({dest:?} @ {commit})", task.name);
                summary.tokens_spent += session_tokens(store, &session);
                summary.done.push((task.name, commit));
            }
            TaskOutcome::Failed { session, log } => {
                let dest = file_move(&path, &opts.queue_dir.join("failed"))
                    .unwrap_or_else(|_| path.clone());
                let log_path = dest.with_extension("log");
                let _ = std::fs::write(&log_path, &log);
                println!("run-loop: FAILED {} (see {log_path:?})", task.name);
                summary.tokens_spent += session_tokens(store, &session);
                summary.failed.push(task.name);
            }
        }
    }
    println!(
        "run-loop: {} done, {} failed, {} skipped (budget), ~{}k tokens",
        summary.done.len(),
        summary.failed.len(),
        summary.skipped_budget.len(),
        summary.tokens_spent / 1000
    );
    Ok(summary)
}

async fn run_one_task<E: TaskExecutor>(
    opts: &BatchOptions,
    store: &crate::session::SessionStore,
    exec: &E,
    task: &TaskSpec,
) -> TaskOutcome {
    let attempts = task.retries.saturating_add(1);
    let mut log = String::new();
    for attempt in 1..=attempts {
        let session = new_session_id("batch");
        println!(
            "run-loop: {} attempt {attempt}/{attempts} (session {})",
            task.name, session
        );
        match exec
            .execute(&task.runbook(), &opts.model, Some(session.clone()))
            .await
        {
            Ok(summary) => {
                log.push_str(&format!(
                    "--- attempt {attempt} agent summary ---\n{summary}\n"
                ));
                let results = run_validations(&task.validate, &opts.workdir);
                let mut ok = true;
                for r in &results {
                    log.push_str(&format!(
                        "--- validate: {} -> {} ---\n{}\n",
                        r.command,
                        if r.ok { "PASS" } else { "FAIL" },
                        r.output.trim_end()
                    ));
                    if !r.ok {
                        ok = false;
                    }
                }
                if task.validate.is_empty() {
                    log.push_str("--- no validation commands; accepting agent summary ---\n");
                }
                if ok {
                    let msg = format!("batch({}): {}", task.name, task.title);
                    match git_commit_scoped(&opts.workdir, &task.files, &msg) {
                        Ok(commit) => {
                            let _ = store.touch_session(&session, Some(&opts.model));
                            return TaskOutcome::Done { session, commit };
                        }
                        Err(e) => {
                            log.push_str(&format!("--- commit failed: {e:#} ---\n"));
                        }
                    }
                }
                if attempt == attempts {
                    return TaskOutcome::Failed { session, log };
                }
                log.push_str(&format!("--- retrying ({attempt}/{attempts}) ---\n"));
            }
            Err(e) => {
                log.push_str(&format!("--- attempt {attempt} agent error: {e:#} ---\n"));
                if attempt == attempts {
                    return TaskOutcome::Failed { session, log };
                }
            }
        }
    }
    TaskOutcome::Failed {
        session: String::new(),
        log,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_task(dir: &Path, name: &str, content: &str) -> PathBuf {
        let pending = dir.join("tasks/pending");
        std::fs::create_dir_all(&pending).unwrap();
        let p = pending.join(format!("{name}.md"));
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn parses_full_task_file() {
        let dir = std::env::temp_dir().join(format!("vh-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = write_task(
            &dir,
            "retry-snapshots",
            "---\ntitle: Add retry to snapshot prune\nfiles:\n  - crates/core/src/session/store/snapshots.rs\nvalidate:\n  - \"cargo test session::\"\nretries: 2\n---\nFix the flake.\n\nAccept: green suite.\n",
        );
        let t = parse_task_file(&p).unwrap();
        assert_eq!(t.name, "retry-snapshots");
        assert_eq!(t.title, "Add retry to snapshot prune");
        assert_eq!(t.files, vec!["crates/core/src/session/store/snapshots.rs"]);
        assert_eq!(t.validate, vec!["cargo test session::"]);
        assert_eq!(t.retries, 2);
        assert!(t.body.contains("Accept: green suite."));
        assert!(t.runbook().contains("Do NOT commit"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_body_only_with_defaults() {
        let dir = std::env::temp_dir().join(format!("vh-batch2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = write_task(&dir, "plain", "Just do it.\n");
        let t = parse_task_file(&p).unwrap();
        assert_eq!(t.title, "plain");
        assert!(t.files.is_empty());
        assert!(t.validate.is_empty());
        assert_eq!(t.retries, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_bad_frontmatter() {
        let dir = std::env::temp_dir().join(format!("vh-batch3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bad = write_task(&dir, "bad", "---\ntitle: x\n");
        assert!(parse_task_file(&bad).is_err(), "unclosed frontmatter");
        let bad = write_task(&dir, "bad2", "---\nbogus: 1\n---\nbody here\n");
        assert!(parse_task_file(&bad).is_err(), "unknown key");
        let bad = write_task(&dir, "bad3", "---\ntitle: x\n---\n   \n");
        assert!(parse_task_file(&bad).is_err(), "empty body");
        let bad = write_task(&dir, "bad4", "---\nretries: many\n---\nbody\n");
        assert!(parse_task_file(&bad).is_err(), "bad retries");
        let bad = write_task(&dir, "bad5", "---\ntitle: x\n- stray\n---\nbody\n");
        assert!(parse_task_file(&bad).is_err(), "stray list item");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validations_report_honestly() {
        let dir = std::env::temp_dir().join(format!("vh-batch4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rs = run_validations(&["true".to_string(), "false".to_string()], &dir);
        assert_eq!(rs.len(), 2);
        assert!(rs[0].ok);
        assert!(!rs[1].ok);
        let rs = run_validations(&["no-such-binary-xyz --version".to_string()], &dir);
        assert!(!rs[0].ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_commit_scopes_to_listed_files() {
        let dir = std::env::temp_dir().join(format!("vh-batch5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(&dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&dir)
            .output()
            .unwrap();
        std::fs::write(dir.join("in.txt"), "v1").unwrap();
        std::fs::write(dir.join("out.txt"), "v1").unwrap();
        std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-qm", "seed"])
            .current_dir(&dir)
            .output()
            .unwrap();
        std::fs::write(dir.join("in.txt"), "v2").unwrap();
        std::fs::write(dir.join("out.txt"), "v2").unwrap();
        let hash = git_commit_scoped(&dir, &["in.txt".to_string()], "batch(t): x").unwrap();
        assert_ne!(hash, "(nothing to commit)");
        let staged = std::process::Command::new("git")
            .args(["show", "--name-only", "--format=", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let files = String::from_utf8_lossy(&staged.stdout);
        assert!(files.contains("in.txt"), "{files}");
        assert!(
            !files.contains("out.txt"),
            "out of scope stays dirty: {files}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct StubExec {
        fail_first: std::sync::Mutex<bool>,
    }

    #[async_trait::async_trait]
    impl TaskExecutor for StubExec {
        async fn execute(
            &self,
            _prompt: &str,
            _model: &str,
            _session: Option<String>,
        ) -> anyhow::Result<String> {
            let mut f = self.fail_first.lock().unwrap();
            if *f {
                *f = false;
                anyhow::bail!("stub boom");
            }
            Ok("stub done".into())
        }
    }

    fn batch_opts(dir: &Path) -> BatchOptions {
        BatchOptions {
            queue_dir: dir.join("tasks"),
            model: "stub/model".into(),
            max_tasks: 10,
            token_budget: 0,
            workdir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn failing_task_retries_then_quarantines() {
        let dir = std::env::temp_dir().join(format!("vh-batch6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_task(
            &dir,
            "flaky",
            "---\ntitle: Flaky\nvalidate:\n  - \"false\"\nretries: 1\n---\nBody.\n",
        );
        let store = crate::session::SessionStore::new_in_memory().unwrap();
        let exec = StubExec {
            fail_first: std::sync::Mutex::new(false),
        };
        let summary = run_batch(&batch_opts(&dir), &store, &exec).await.unwrap();
        assert_eq!(summary.failed, vec!["flaky"]);
        assert!(dir.join("tasks/failed/flaky.md").exists());
        assert!(dir.join("tasks/failed/flaky.log").exists());
        assert!(!dir.join("tasks/pending/flaky.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn passing_task_commits_and_moves_to_done() {
        let dir = std::env::temp_dir().join(format!("vh-batch7-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for c in [
            &["init", "-q"][..],
            &["config", "user.email", "t@t"][..],
            &["config", "user.name", "t"][..],
            &["commit", "-q", "--allow-empty", "-m", "seed"][..],
        ] {
            std::process::Command::new("git")
                .args(c)
                .current_dir(&dir)
                .output()
                .unwrap();
        }
        write_task(
            &dir,
            "easy",
            "---\ntitle: Easy\nvalidate:\n  - \"true\"\n---\nBody.\n",
        );
        let store = crate::session::SessionStore::new_in_memory().unwrap();
        let exec = StubExec {
            fail_first: std::sync::Mutex::new(false),
        };
        let summary = run_batch(&batch_opts(&dir), &store, &exec).await.unwrap();
        assert_eq!(summary.done.len(), 1);
        assert!(dir.join("tasks/done/easy.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn agent_error_retries_before_quarantine() {
        let dir = std::env::temp_dir().join(format!("vh-batch8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_task(
            &dir,
            "oops",
            "---\ntitle: Oops\n---\nBody with no validation.\n",
        );
        let store = crate::session::SessionStore::new_in_memory().unwrap();
        let exec = StubExec {
            fail_first: std::sync::Mutex::new(true),
        };
        let summary = run_batch(&batch_opts(&dir), &store, &exec).await.unwrap();
        // Default retries:1 → first fails, retry succeeds; empty validate accepts.
        // No git repo here: commit fails → quarantined (commit is a gate too).
        assert_eq!(summary.failed, vec!["oops"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn token_budget_stops_the_run() {
        struct BurningExec<'a> {
            store: &'a crate::session::SessionStore,
        }
        #[async_trait::async_trait]
        impl TaskExecutor for BurningExec<'_> {
            async fn execute(
                &self,
                _prompt: &str,
                model: &str,
                session: Option<String>,
            ) -> anyhow::Result<String> {
                let sid = session.unwrap();
                self.store.ensure_session(&sid, model, None)?;
                // ~2k tokens of fake work.
                self.store
                    .append_message(&sid, "assistant", &"x".repeat(8000))?;
                Ok("burned".into())
            }
        }
        let dir = std::env::temp_dir().join(format!("vh-batch9-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_task(&dir, "a", "---\ntitle: A\n---\nBody A.\n");
        write_task(&dir, "b", "---\ntitle: B\n---\nBody B.\n");
        let store = crate::session::SessionStore::new_in_memory().unwrap();
        let exec = BurningExec { store: &store };
        let mut opts = batch_opts(&dir);
        opts.token_budget = 1000; // first task burns ~2k → second is skipped
        let summary = run_batch(&opts, &store, &exec).await.unwrap();
        // No git repo here so task "a" fails at commit — but its ~2k burned
        // tokens still count, and task "b" is skipped on budget.
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.skipped_budget.len(), 1);
        assert!(summary.tokens_spent >= 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
