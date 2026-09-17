// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

//! Live registry of subagent executions (`task` tool).
//!
//! Subagents run synchronously inside the turn that spawned them, so
//! without a registry they are invisible while working. The pool
//! records every spawn here: the TUI `/agents` dialog reads it to show
//! running subagents plus recent history. In-memory for speed, with a
//! best-effort sqlite write-through (`subagent_runs` in the sessions DB)
//! so history survives restarts; rows still marked `running` at load died
//! with their process and surface once as `error` ("interrupted").

use std::collections::VecDeque;
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Done,
    Error,
    Killed,
}

impl SubagentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubagentStatus::Running => "running",
            SubagentStatus::Done => "done",
            SubagentStatus::Error => "error",
            SubagentStatus::Killed => "killed",
        }
    }

    /// Fail-closed parse for hydrated rows: anything unrecognized is an
    /// error, never resurrected as running.
    fn parse(s: &str) -> Self {
        match s {
            "running" => SubagentStatus::Running,
            "done" => SubagentStatus::Done,
            "killed" => SubagentStatus::Killed,
            _ => SubagentStatus::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubagentRun {
    pub id: String,
    pub kind: String,
    pub prompt_preview: String,
    pub full_prompt: String,
    /// Parent agent mode at spawn (origin tag, like background tasks).
    pub mode: String,
    /// Nesting depth: 1 = spawned by the main turn, 2+ = nested.
    /// Computed from the parent turn's depth (`parent_depth + 1`), not
    /// from the number of concurrently-running agents, so parallel
    /// siblings share the same depth.
    pub depth: usize,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: SubagentStatus,
    pub result_preview: Option<String>,
    /// Full result text (capped) for detail views and follow-up turns.
    pub result_full: Option<String>,
    /// True when the result exceeded `RESULT_FULL_CHARS` and was cut.
    /// The complete output (up to `LOG_FULL_CHARS`) lives at `log_path`.
    pub result_truncated: bool,
    /// File holding the complete result, like background-task logs.
    /// Always written on finish (best-effort); survives restarts.
    pub log_path: Option<String>,
    /// Chat session holding this run's transcript (`sub-<run id>`).
    /// Enter on a finished run opens this session instead of a summary
    /// buried in main chat. `None` for rows from before the link existed.
    pub session_id: Option<String>,
    /// Chat session that spawned this run (the `task` tool caller's
    /// `session_id`). The /agents dialog lists only the current chat's
    /// runs; `None` for rows from before the link existed (hidden there).
    pub parent_session: Option<String>,
    /// Completion already surfaced (TUI notice / follow-up turn).
    pub notified: bool,
}

impl SubagentRun {
    pub fn elapsed_secs(&self, now: i64) -> i64 {
        self.finished_at
            .unwrap_or(now)
            .saturating_sub(self.started_at)
    }
}
const MAX_KEPT: usize = 30;
const PREVIEW_CHARS: usize = 120;
const FULL_PROMPT_CHARS: usize = 2000;
const RESULT_CHARS: usize = 500;
const RESULT_FULL_CHARS: usize = 12_000;
/// Cap for the on-disk log (keeps huge outputs from filling the disk).
const LOG_FULL_CHARS: usize = 200_000;

static RUNS: LazyLock<Mutex<VecDeque<SubagentRun>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
/// Abort handles of live runs, by run id (detached + blocking — both are
/// spawned so `cancel_run`/`x` in /agents can abort either).
static HANDLES: LazyLock<Mutex<std::collections::HashMap<String, tokio::task::AbortHandle>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn new_id() -> String {
    static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("sa_{:x}{:04x}", nanos & 0xffffffffffffff, c & 0xffff)
}

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Timeout for one subagent run, if any. `VIORAHARNESS_SUBAGENT_TIMEOUT_SECS`
/// is opt-in: positive bounds the run, anything else (unset included) means
/// unbounded like opencode/Claude Code. Backstops remain: the 20-turn loop
/// budget, per-tool timeouts, and `x` in /agents.
pub fn subagent_timeout_opt() -> Option<std::time::Duration> {
    let secs = std::env::var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)?;
    Some(std::time::Duration::from_secs(secs))
}

/// On-disk log for a run, mirroring background-task logs
/// (`~/.local/share/vioraharness/logs/<id>.log`). Best-effort: parent
/// dirs are created, write failures are ignored by the caller.
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

/// Rows kept in sqlite (wider than the in-memory window so a restart
/// recovers recent history, not just live state).
const MAX_DB_KEPT: i64 = 100;

/// Which DB path this process has hydrated from (`None` = not yet).
/// Re-hydrates when the path changes (persistence tests point at temp DBs).
static HYDRATED_FOR: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

/// Prefix for the chat sessions holding subagent transcripts.
/// `sub-` sessions are the subagent's own view (opened from /agents) and
/// are hidden from user chat lists (`list_sessions_filtered`,
/// `latest_for_cwd`, counts) so they never hijack `--continue`/pickers.
pub const SUB_SESSION_PREFIX: &str = "sub-";

/// Shared DDL for the runs table (tracker `open_db` + `SessionStore::migrate`).
pub(crate) const SUBAGENT_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS subagent_runs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    prompt_preview TEXT NOT NULL,
    full_prompt TEXT NOT NULL,
    mode TEXT NOT NULL DEFAULT '',
    depth INTEGER NOT NULL DEFAULT 1,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    status TEXT NOT NULL,
    result_preview TEXT,
    result_full TEXT,
    result_truncated INTEGER NOT NULL DEFAULT 0,
    log_path TEXT,
    notified INTEGER NOT NULL DEFAULT 0,
    session_id TEXT,
    parent_session TEXT
);";

/// ALTER for DBs created before the session link existed. Best-effort:
/// duplicate-column on re-run is ignored by the caller.
pub(crate) const SUBAGENT_COLUMN_SESSION: &str =
    "ALTER TABLE subagent_runs ADD COLUMN session_id TEXT";

/// ALTER for DBs created before the parent-chat link existed. Same
/// best-effort contract as the session column above.
pub(crate) const SUBAGENT_COLUMN_PARENT: &str =
    "ALTER TABLE subagent_runs ADD COLUMN parent_session TEXT";

/// Kill-switch: `VIORAHARNESS_SUBAGENT_PERSIST=0/off/false/no` disables all
/// sqlite I/O (in-memory only). Anything else (unset included) persists.
fn persist_disabled() -> bool {
    matches!(
        std::env::var("VIORAHARNESS_SUBAGENT_PERSIST")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "off" | "false" | "no"
    )
}

fn shellexpand_db(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

/// sqlite file for runs: explicit `VIORAHARNESS_SUBAGENT_DB` wins (tests),
/// else the sessions DB. `None` means persistence is off — kill-switch, or
/// unit tests without an explicit DB (keeps `cargo test` hermetic and off
/// the user's real DB; integration tests link the lib without `cfg(test)`
/// and persist normally).
fn persist_db_path() -> Option<String> {
    if persist_disabled() {
        return None;
    }
    if let Ok(p) = std::env::var("VIORAHARNESS_SUBAGENT_DB") {
        if !p.trim().is_empty() {
            return Some(shellexpand_db(&p));
        }
    }
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        Some(shellexpand_db(
            &std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into()),
        ))
    }
}

fn open_db(path: &str) -> Option<rusqlite::Connection> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let conn = rusqlite::Connection::open(path).ok()?;
    let _ = conn.execute_batch("PRAGMA busy_timeout=5000;");
    let _ = conn.execute_batch(SUBAGENT_TABLE_SQL);
    let _ = conn.execute(SUBAGENT_COLUMN_SESSION, []);
    let _ = conn.execute(SUBAGENT_COLUMN_PARENT, []);
    Some(conn)
}

/// Best-effort write-through of one run. Never panics and never fails the
/// caller — a bookkeeping path must not break turns.
fn persist_run(run: &SubagentRun) {
    let Some(db_path) = persist_db_path() else {
        return;
    };
    let Some(conn) = open_db(&db_path) else {
        return;
    };
    let _ = conn.execute(
        "INSERT OR REPLACE INTO subagent_runs
         (id, kind, prompt_preview, full_prompt, mode, depth, started_at,
          finished_at, status, result_preview, result_full, result_truncated,
          log_path, notified, session_id, parent_session)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        rusqlite::params![
            run.id,
            run.kind,
            run.prompt_preview,
            run.full_prompt,
            run.mode,
            run.depth as i64,
            run.started_at,
            run.finished_at,
            run.status.as_str(),
            run.result_preview,
            run.result_full,
            run.result_truncated as i32,
            run.log_path,
            run.notified as i32,
            run.session_id,
            run.parent_session,
        ],
    );
    let _ = conn.execute(
        "DELETE FROM subagent_runs WHERE id NOT IN
         (SELECT id FROM subagent_runs ORDER BY started_at DESC LIMIT ?1)",
        rusqlite::params![MAX_DB_KEPT],
    );
}

/// Snapshot the in-memory row for `id` into sqlite. Call after the RUNS
/// lock is released; no-op when persistence is off (fast path: checks the
/// path before locking).
fn persist_snapshot(id: &str) {
    if persist_db_path().is_none() {
        return;
    }
    let run = RUNS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|r| r.id == id)
        .cloned();
    if let Some(run) = run {
        persist_run(&run);
    }
}

/// Merge persisted rows into memory (once per DB path). Recovered rows are
/// SILENT history: `notified` is forced on so a restart never re-posts old
/// notices nor fires follow-up turns for a dead process. Rows still marked
/// `running` died with their process: flip to `error` ("interrupted by
/// restart") — inspectable in /agents, never announced.
fn hydrate_runs() {
    let Some(db_path) = persist_db_path() else {
        return;
    };
    {
        let done = HYDRATED_FOR.lock().unwrap_or_else(|e| e.into_inner());
        if done.as_deref() == Some(db_path.as_str()) {
            return;
        }
    }
    let Some(conn) = open_db(&db_path) else {
        return;
    };
    let rows: Vec<SubagentRun> = (|| {
        let mut stmt = conn
            .prepare(
                "SELECT id, kind, prompt_preview, full_prompt, mode, depth, started_at,
                    finished_at, status, result_preview, result_full,
                    result_truncated, log_path, notified, session_id, parent_session
             FROM subagent_runs ORDER BY started_at DESC LIMIT ?1",
            )
            .ok()?;
        let iter = stmt
            .query_map(rusqlite::params![MAX_DB_KEPT], |row| {
                Ok(SubagentRun {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    prompt_preview: row.get(2)?,
                    full_prompt: row.get(3)?,
                    mode: row.get(4)?,
                    depth: row.get::<_, i64>(5).unwrap_or(1).max(1) as usize,
                    started_at: row.get(6)?,
                    finished_at: row.get(7)?,
                    status: row
                        .get::<_, String>(8)
                        .map(|s| SubagentStatus::parse(&s))
                        .unwrap_or(SubagentStatus::Error),
                    result_preview: row.get(9)?,
                    result_full: row.get(10)?,
                    result_truncated: row.get::<_, i64>(11).unwrap_or(0) != 0,
                    log_path: row.get(12)?,
                    notified: row.get::<_, i64>(13).unwrap_or(0) != 0,
                    session_id: row.get(14).unwrap_or(None),
                    parent_session: row.get(15).unwrap_or(None),
                })
            })
            .ok()?;
        iter.collect::<Result<Vec<_>, _>>().ok()
    })()
    .unwrap_or_default();
    if rows.is_empty() {
        *HYDRATED_FOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(db_path);
        return;
    }
    let mut corrected = Vec::new();
    {
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        // Oldest first so push_back preserves newest-last order.
        for mut run in rows.into_iter().rev() {
            if runs.iter().any(|r| r.id == run.id) {
                continue;
            }
            if run.status == SubagentStatus::Running {
                run.status = SubagentStatus::Error;
                run.finished_at = Some(now_secs());
                run.result_preview = Some("interrupted by restart (process exited)".into());
                run.result_full = Some("interrupted by restart (process exited)".into());
                corrected.push(run.clone());
            }
            // Silent history: never notify/wake for a life that ended
            // before this process booted.
            run.notified = true;
            runs.push_back(run);
        }
        while runs.len() > MAX_KEPT {
            runs.pop_front();
        }
    }
    // The clone was taken before `notified` was forced on above — set it
    // here so the persisted flip stays silent (never drains after restart).
    for mut run in corrected {
        run.notified = true;
        persist_run(&run);
    }
    *HYDRATED_FOR.lock().unwrap_or_else(|e| e.into_inner()) = Some(db_path);
}

/// Record a spawn. Returns the run id. Depth is the number of
/// currently-running tracked subagents + 1, so nested `task` calls
/// inside a subagent naturally deepen.
pub fn track_start(kind: &str, prompt: &str, mode: &str) -> (String, usize) {
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let depth = runs
        .iter()
        .filter(|r| r.status == SubagentStatus::Running)
        .count()
        + 1;
    let out = track_start_inner(&mut runs, kind, prompt, mode, depth);
    drop(runs);
    persist_snapshot(&out.0);
    out
}

/// Record a spawn at an explicit depth (`parent_depth + 1` from the
/// spawning turn). Prefer this over `track_start` in spawn paths: parallel
/// siblings share the same depth instead of inflating it by start order.
pub fn track_start_at_depth(kind: &str, prompt: &str, mode: &str, depth: usize) -> (String, usize) {
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let depth = depth.max(1);
    let out = track_start_inner(&mut runs, kind, prompt, mode, depth);
    drop(runs);
    persist_snapshot(&out.0);
    out
}

fn track_start_inner(
    runs: &mut std::collections::VecDeque<SubagentRun>,
    kind: &str,
    prompt: &str,
    mode: &str,
    depth: usize,
) -> (String, usize) {
    let id = new_id();
    runs.push_back(SubagentRun {
        id: id.clone(),
        kind: kind.to_string(),
        prompt_preview: take_chars(prompt.trim(), PREVIEW_CHARS),
        full_prompt: take_chars(prompt, FULL_PROMPT_CHARS),
        mode: mode.to_string(),
        depth,
        started_at: now_secs(),
        finished_at: None,
        status: SubagentStatus::Running,
        result_preview: None,
        result_full: None,
        result_truncated: false,
        log_path: None,
        session_id: None,
        parent_session: None,
        notified: false,
    });
    while runs.len() > MAX_KEPT {
        runs.pop_front();
    }
    (id, depth)
}

/// Record completion. Unknown ids are ignored. The full result spills to
/// `log_path_for(id)` (best-effort, capped); `result_full` keeps the head
/// and `result_truncated` marks the cut so the UI can point at the log.
pub fn track_finish(id: &str, ok: bool, result: &str) {
    let truncated = result.chars().count() > RESULT_FULL_CHARS;
    let preview = take_chars(result, RESULT_CHARS);
    let full = take_chars(result, RESULT_FULL_CHARS);
    // Best-effort spill outside the lock (disk I/O must not block reads).
    let path = log_path_for(id);
    let logged = std::fs::write(&path, take_chars(result, LOG_FULL_CHARS)).is_ok();
    {
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = runs.iter_mut().rev().find(|r| r.id == id) {
            // Cancel-then-finish races resolve to cancelled: a finished run
            // is never overwritten.
            if run.status != SubagentStatus::Running {
                return;
            }
            run.status = if ok {
                SubagentStatus::Done
            } else {
                SubagentStatus::Error
            };
            run.finished_at = Some(now_secs());
            run.result_preview = Some(preview);
            run.result_full = Some(full);
            run.result_truncated = truncated;
            run.log_path = if logged { Some(path) } else { None };
        }
    }
    HANDLES.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    persist_snapshot(id);
}

/// All runs, newest first.
pub fn list_runs() -> Vec<SubagentRun> {
    hydrate_runs();
    RUNS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .cloned()
        .collect()
}

/// Currently-running subagents, newest first.
pub fn active_runs() -> Vec<SubagentRun> {
    list_runs()
        .into_iter()
        .filter(|r| r.status == SubagentStatus::Running)
        .collect()
}

/// Finished runs not yet surfaced (TUI notice / follow-up turn).
/// Each completion drains once, like `tasks::take_completions`.
/// Hydrates first so completions missed during downtime still surface.
pub fn take_completions() -> Vec<SubagentRun> {
    take_completions_for(None)
}

/// Scoped drain: only runs belonging to `scope` surface. A run belongs
/// when it has no parent link yet (legacy/test probes, spawn race) or its
/// parent is the scoped chat. Others stay un-notified until their own chat
/// polls — otherwise a completion would post into whatever chat is open.
pub fn take_completions_for(scope: Option<&str>) -> Vec<SubagentRun> {
    hydrate_runs();
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for run in runs.iter_mut() {
        if run.status != SubagentStatus::Running && !run.notified {
            let belongs = match (scope, run.parent_session.as_deref()) {
                (None, _) => true,
                (Some(_), None) => true,
                (Some(s), Some(p)) => p == s,
            };
            if !belongs {
                continue;
            }
            run.notified = true;
            out.push(run.clone());
        }
    }
    drop(runs);
    for run in &out {
        persist_snapshot(&run.id);
    }
    out.sort_by_key(|r| r.finished_at.unwrap_or(0));
    out
}

/// Stash the abort handle of a live run (pool internal, both blocking
/// and detached spawn paths so either can be cancelled).
pub fn register_handle(id: &str, handle: tokio::task::AbortHandle) {
    HANDLES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id.to_string(), handle);
}

/// Newest run linked to a transcript session, if any (TUI resolves the
/// read-only guard and /agents navigation through this).
pub fn run_for_session(session_id: &str) -> Option<SubagentRun> {
    hydrate_runs();
    RUNS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .rev()
        .find(|r| r.session_id.as_deref() == Some(session_id))
        .cloned()
}

/// Link a run to the chat session holding its transcript (pool internal,
/// right after spawn). The link persists with the row, so /agents can
/// open the subagent's own view after restarts too.
pub fn set_run_session(id: &str, session_id: &str) {
    {
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(run) = runs.iter_mut().rev().find(|r| r.id == id) else {
            return;
        };
        run.session_id = Some(session_id.to_string());
    }
    persist_snapshot(id);
}

/// Link a run to the chat session that spawned it (pool internal, right
/// after spawn, from the `task` tool's injected `session_id`). The
/// /agents dialog lists only the current chat's runs.
pub fn set_run_parent(id: &str, parent_session: &str) {
    {
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(run) = runs.iter_mut().rev().find(|r| r.id == id) else {
            return;
        };
        run.parent_session = Some(parent_session.to_string());
    }
    persist_snapshot(id);
}

/// Cancel a running subagent (detached or blocking): aborts its task
/// and marks it killed. Returns false when there is nothing running
/// under `id`.
pub fn cancel_run(id: &str) -> bool {
    let handle = HANDLES.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    let Some(handle) = handle else {
        return false;
    };
    if handle.is_finished() {
        return false;
    }
    handle.abort();
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(run) = runs.iter_mut().rev().find(|r| r.id == id) {
        if run.status == SubagentStatus::Running {
            run.status = SubagentStatus::Killed;
            run.finished_at = Some(now_secs());
            run.result_preview = Some("cancelled by user".to_string());
            run.result_full = Some("cancelled by user".to_string());
            run.result_truncated = false;
            drop(runs);
            persist_snapshot(id);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_prompt(tag: &str) -> String {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("tracker-probe-{tag}-{n}-{}", std::process::id())
    }

    #[test]
    fn start_lists_running_then_finish_marks_done() {
        // Serialized on ENV_LOCK: RUNS + the completion drain are global,
        // so parallel tracker tests would eat each other's completions.
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prompt = unique_prompt("lifecycle");
        let (id, depth) = track_start("explore", &prompt, "eda");
        assert!(depth >= 1);
        let running: Vec<_> = active_runs().into_iter().filter(|r| r.id == id).collect();
        assert_eq!(running.len(), 1, "visible while running");
        assert_eq!(running[0].prompt_preview, prompt);
        assert_eq!(running[0].mode, "eda");
        track_finish(&id, true, "found three call sites");
        assert!(
            active_runs().iter().all(|r| r.id != id),
            "gone from active after finish"
        );
        let done = list_runs().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(done.status, SubagentStatus::Done);
        assert_eq!(
            done.result_preview.as_deref(),
            Some("found three call sites")
        );
        assert!(done.finished_at.is_some());
        if let Some(p) = done.log_path.as_deref() {
            let _ = std::fs::remove_file(p);
        }
        let _ = take_completions();
    }

    #[test]
    fn error_status_and_nesting_depth() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let outer_p = unique_prompt("outer");
        let (outer, d1) = track_start("planner", &outer_p, "web");
        let inner_p = unique_prompt("inner");
        let (inner, d2) = track_start("explore", &inner_p, "web");
        assert!(d2 > d1, "nested spawn deepens: {d1} → {d2}");
        track_finish(&inner, false, "boom");
        let inner_run = list_runs().into_iter().find(|r| r.id == inner).unwrap();
        assert_eq!(inner_run.status, SubagentStatus::Error);
        track_finish(&outer, true, "ok");
    }

    #[test]
    fn unknown_finish_is_ignored_and_previews_capped() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        track_finish("sa_nonexistent", true, "x");
        let long = "y".repeat(5000);
        let prompt = format!("{}-{}", unique_prompt("cap"), long);
        let (id, _) = track_start("coder", &prompt, "eda");
        let run = list_runs().into_iter().find(|r| r.id == id).unwrap();
        assert!(run.prompt_preview.chars().count() <= PREVIEW_CHARS);
        assert!(run.full_prompt.chars().count() <= FULL_PROMPT_CHARS);
        track_finish(&id, true, &long);
        let run = list_runs().into_iter().find(|r| r.id == id).unwrap();
        assert!(run.result_preview.unwrap().chars().count() <= RESULT_CHARS);
        assert!(run.result_full.unwrap().chars().count() <= RESULT_FULL_CHARS);
    }

    #[test]
    fn completions_drain_once() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (id, _) = track_start("explore", &unique_prompt("drain"), "eda");
        track_finish(&id, true, "result here");
        let first: Vec<_> = take_completions()
            .into_iter()
            .filter(|r| r.id == id)
            .collect();
        assert_eq!(first.len(), 1, "surfaced once");
        assert_eq!(first[0].result_full.as_deref(), Some("result here"));
        assert!(
            take_completions().iter().all(|r| r.id != id),
            "second drain skips it"
        );
    }

    #[test]
    fn cancel_unknown_or_finished_is_false() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(!cancel_run("sa_nonexistent"), "nothing to cancel");
        let (id, _) = track_start("coder", &unique_prompt("nocancel"), "eda");
        track_finish(&id, true, "already done");
        assert!(!cancel_run(&id), "finished run has no handle");
    }

    #[tokio::test]
    async fn cancel_aborts_live_handle() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (id, _) = track_start("explore", &unique_prompt("abort"), "eda");
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        });
        register_handle(&id, handle.abort_handle());
        assert!(cancel_run(&id), "live run cancelled");
        let run = list_runs().into_iter().find(|r| r.id == id).unwrap();
        assert_eq!(run.status, SubagentStatus::Killed);
        assert!(!cancel_run(&id), "second cancel is a no-op");
        let done: Vec<_> = take_completions()
            .into_iter()
            .filter(|r| r.id == id)
            .collect();
        assert_eq!(done.len(), 1, "cancellation surfaces as a completion");
    }

    #[test]
    fn explicit_depth_siblings_share_depth() {
        // Parallel siblings must share parent+1, not inflate by start order
        // (the old running-count behavior gave 1,2,3 for three siblings).
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (a, da) = track_start_at_depth("explore", &unique_prompt("sib-a"), "eda", 1);
        let (b, db) = track_start_at_depth("explore", &unique_prompt("sib-b"), "eda", 1);
        assert_eq!((da, db), (1, 1), "siblings share depth");
        let (c, d_child) = track_start_at_depth("coder", &unique_prompt("child"), "eda", 2);
        assert_eq!(d_child, 2, "nested child deepens");
        for id in [a, b, c] {
            track_finish(&id, true, "sib done");
        }
        let _ = take_completions();
    }

    #[test]
    fn truncation_flags_and_spills_to_log() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let big = "z".repeat(RESULT_FULL_CHARS + 100);
        let (id, _) = track_start("explore", &unique_prompt("trunc"), "eda");
        track_finish(&id, true, &big);
        let run = list_runs().into_iter().find(|r| r.id == id).unwrap();
        assert!(run.result_truncated, "over-cap output marks truncated");
        assert_eq!(run.result_full.unwrap().chars().count(), RESULT_FULL_CHARS);
        let path = run.log_path.expect("spill path recorded");
        let logged = std::fs::read_to_string(&path).expect("log readable");
        assert!(logged.chars().count() > RESULT_FULL_CHARS, "log keeps more");
        let _ = std::fs::remove_file(&path);
        let _ = take_completions();
    }

    #[test]
    fn timeout_env_parses_and_defaults() {
        use crate::ENV_LOCK;
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS").ok();
        std::env::set_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS", "7");
        assert_eq!(
            subagent_timeout_opt(),
            Some(std::time::Duration::from_secs(7))
        );
        // No timeout by default — matches opencode/Claude Code behavior.
        std::env::remove_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS");
        assert_eq!(subagent_timeout_opt(), None);
        std::env::set_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS", "garbage");
        assert_eq!(subagent_timeout_opt(), None);
        std::env::set_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS", "0");
        assert_eq!(subagent_timeout_opt(), None);
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_TIMEOUT_SECS"),
        }
    }

    /// Temp `VIORAHARNESS_SUBAGENT_DB` under the process-global env lock.
    /// Unit tests default to persistence-off (`persist_db_path` returns
    /// `None` under `cfg(test)` without this), so only tests holding this
    /// guard touch sqlite — never the user's real DB.
    struct PersistEnv {
        prev_db: Option<String>,
        prev_off: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
        path: String,
    }

    fn persist_env(tag: &str) -> PersistEnv {
        let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("vh_subagent_{tag}_{}_{n}.db", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let prev_db = std::env::var("VIORAHARNESS_SUBAGENT_DB").ok();
        let prev_off = std::env::var("VIORAHARNESS_SUBAGENT_PERSIST").ok();
        std::env::set_var("VIORAHARNESS_SUBAGENT_DB", &path);
        std::env::remove_var("VIORAHARNESS_SUBAGENT_PERSIST");
        PersistEnv {
            prev_db,
            prev_off,
            _guard: guard,
            path,
        }
    }

    impl Drop for PersistEnv {
        fn drop(&mut self) {
            match &self.prev_db {
                Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_DB", v),
                None => std::env::remove_var("VIORAHARNESS_SUBAGENT_DB"),
            }
            match &self.prev_off {
                Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_PERSIST", v),
                None => std::env::remove_var("VIORAHARNESS_SUBAGENT_PERSIST"),
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn db_row(db: &str, id: &str) -> Option<(String, i64, i64)> {
        let conn = rusqlite::Connection::open(db).ok()?;
        conn.query_row(
            "SELECT status, notified, depth FROM subagent_runs WHERE id = ?1",
            rusqlite::params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    }

    #[test]
    fn persist_write_through_roundtrip() {
        let env = persist_env("roundtrip");
        let (id, _) = track_start_at_depth("explore", &unique_prompt("persist"), "eda", 2);
        let (status, notified, depth) =
            db_row(&env.path, &id).expect("start wrote the row through");
        assert_eq!(status, "running");
        assert_eq!((notified, depth), (0, 2));
        track_finish(&id, true, "persisted ok");
        let (status, notified, _) = db_row(&env.path, &id).expect("finish updated the row");
        assert_eq!((status.as_str(), notified), ("done", 0));
        // Draining marks notified in memory AND in sqlite (no re-surface
        // after a restart).
        let drained: Vec<_> = take_completions()
            .into_iter()
            .filter(|r| r.id == id)
            .collect();
        assert_eq!(drained.len(), 1, "surfaced once");
        let (_, notified, _) = db_row(&env.path, &id).expect("row still there");
        assert_eq!(notified, 1, "drain persisted");
        if let Some(p) = drained[0].log_path.as_deref() {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn restart_recovers_interrupted_and_honors_notified() {
        let env = persist_env("restart");
        // Fake a previous process's rows directly: one run still marked
        // running at shutdown, one already-surfaced completion.
        {
            let conn = open_db(&env.path).expect("temp db opens");
            for (id, status, notified) in [
                ("sa_restart_run", "running", 0),
                ("sa_restart_done", "done", 1),
            ] {
                conn.execute(
                    "INSERT INTO subagent_runs
                     (id, kind, prompt_preview, full_prompt, mode, depth,
                      started_at, status, notified)
                     VALUES (?1,'explore','p','p','eda',1,1,?2,?3)",
                    rusqlite::params![id, status, notified],
                )
                .unwrap();
            }
        }
        // Fresh path → hydrate runs on first read.
        hydrate_runs();
        let run = list_runs()
            .into_iter()
            .find(|r| r.id == "sa_restart_run")
            .expect("interrupted run recovered");
        assert_eq!(run.status, SubagentStatus::Error);
        assert_eq!(
            run.result_preview.as_deref(),
            Some("interrupted by restart (process exited)")
        );
        let done = list_runs()
            .into_iter()
            .find(|r| r.id == "sa_restart_done")
            .expect("surfaced row recovered");
        assert!(done.notified, "stays surfaced");
        // Recovered rows are SILENT history: no notices, no wake turns
        // for work from a dead process — inspectable in /agents only.
        assert!(
            take_completions()
                .iter()
                .all(|r| !r.id.starts_with("sa_restart_")),
            "hydrated rows never drain"
        );
        let (status, _, _) = db_row(&env.path, "sa_restart_run").expect("correction persisted");
        assert_eq!(status, "error");
    }

    #[test]
    fn session_link_persists_with_row() {
        let env = persist_env("session");
        let (id, _) = track_start_at_depth("coder", &unique_prompt("sess"), "web", 1);
        set_run_session(&id, "sub-sa_link9");
        let run = list_runs()
            .into_iter()
            .find(|r| r.id == id)
            .expect("run listed");
        assert_eq!(run.session_id.as_deref(), Some("sub-sa_link9"));
        let conn = rusqlite::Connection::open(&env.path).expect("temp db opens");
        let stored: Option<String> = conn
            .query_row(
                "SELECT session_id FROM subagent_runs WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .expect("link persisted");
        assert_eq!(stored.as_deref(), Some("sub-sa_link9"));
        // Unknown ids never panic.
        set_run_session("sa_nonexistent", "sub-nope");
        track_finish(&id, true, "done");
        let _ = take_completions();
    }

    #[test]
    fn parent_link_persists_with_row() {
        let env = persist_env("parent");
        let (id, _) = track_start_at_depth("explore", &unique_prompt("par"), "eda", 1);
        set_run_parent(&id, "sess-parent9");
        let run = list_runs()
            .into_iter()
            .find(|r| r.id == id)
            .expect("run listed");
        assert_eq!(run.parent_session.as_deref(), Some("sess-parent9"));
        let conn = rusqlite::Connection::open(&env.path).expect("temp db opens");
        let stored: Option<String> = conn
            .query_row(
                "SELECT parent_session FROM subagent_runs WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .expect("parent persisted");
        assert_eq!(stored.as_deref(), Some("sess-parent9"));
        // Unknown ids never panic.
        set_run_parent("sa_nonexistent", "sess-nope");
        track_finish(&id, true, "done");
        let _ = take_completions();
    }

    #[test]
    fn run_for_session_resolves_link() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert!(run_for_session("sub-nope-missing").is_none());
        let (id, _) = track_start("explore", &unique_prompt("resolve"), "eda");
        assert!(run_for_session("sub-resolve9").is_none(), "no link yet");
        set_run_session(&id, "sub-resolve9");
        assert_eq!(
            run_for_session("sub-resolve9").map(|r| r.id),
            Some(id.clone()),
            "linked run resolves"
        );
        track_finish(&id, true, "done");
        assert!(
            run_for_session("sub-resolve9").is_some(),
            "finished runs keep the link (own view stays openable)"
        );
        let _ = take_completions();
    }

    #[test]
    fn persist_kill_switch_disables_sqlite() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_db = std::env::var("VIORAHARNESS_SUBAGENT_DB").ok();
        let prev_off = std::env::var("VIORAHARNESS_SUBAGENT_PERSIST").ok();
        let path = std::env::temp_dir()
            .join(format!("vh_subagent_off_{}.db", std::process::id()))
            .to_string_lossy()
            .to_string();
        std::env::set_var("VIORAHARNESS_SUBAGENT_DB", &path);
        std::env::set_var("VIORAHARNESS_SUBAGENT_PERSIST", "0");
        let (id, _) = track_start("explore", &unique_prompt("off"), "eda");
        track_finish(&id, true, "no db");
        assert!(
            !std::path::Path::new(&path).exists(),
            "kill-switch wrote nothing"
        );
        let _ = take_completions();
        match prev_db {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_DB", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_DB"),
        }
        match prev_off {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_PERSIST", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_PERSIST"),
        }
    }
}
