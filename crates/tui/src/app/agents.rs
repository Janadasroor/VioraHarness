//! `/agents` dialog: every active agent in one place with navigation.
//!
//! Rows are flat (one cursor across sections, like the Tasks panel):
//! the live turn first, then this chat's subagent runs (active first),
//! then background tasks (running first). Enter acts contextually:
//! live closes, a linked subagent opens its transcript session (its own
//! view, live or finished — running transcripts are read-only), task
//! jumps to the Tasks panel focused on it. Unlinked legacy rows fall
//! back to a detail post. Chats live in `/sessions` — this dialog stays
//! scoped to the current chat so old explore sessions never crowd it.
//! Selection is id-anchored (`AgentRow::row_id`): the list re-sorts on
//! every rebuild, so a bare index could land on another row by Enter
//! time. Main chat also announces each launch with its title
//! (`poll_subagent_starts`, every event-loop tick).

use super::*;
use vioraharness_core::subagent::tracker::{self, SubagentRun};
use vioraharness_core::tools::tasks::{self, BgStatus, BgTask};

#[derive(Debug, Clone)]
pub(crate) enum AgentRow {
    Live,
    Subagent(SubagentRun),
    Task(BgTask),
}

impl AgentRow {
    /// Stable identity surviving list rebuilds. The dialog re-sorts on
    /// every keypress (active-first), so a bare index can silently point
    /// at another row by Enter time — usually Live, which reads as "the
    /// subagent opened the main agent". Anchor by id instead.
    pub(crate) fn row_id(&self) -> String {
        match self {
            AgentRow::Live => "live".to_string(),
            AgentRow::Subagent(run) => format!("sa:{}", run.id),
            AgentRow::Task(t) => format!("task:{}", t.id),
        }
    }
}

/// Flattened selectable rows: live turn, this chat's subagents (active
/// first), background tasks (running first). The subagent section is
/// scoped to the current chat (or the originating chat while peeking at
/// a transcript) via each run's parent link — other chats' runs never
/// crowd this dialog. Recent chats are intentionally excluded (use
/// `/sessions`); that section is what made repeated explore sessions
/// look like agent spam.
pub(crate) fn agent_rows(app: &App) -> Vec<AgentRow> {
    let mut rows = vec![AgentRow::Live];
    let scope = app.agents_scope_session();
    let mut runs: Vec<_> = tracker::list_runs()
        .into_iter()
        .filter(|r| r.parent_session.as_deref() == Some(scope))
        .collect();
    runs.sort_by_key(|r| r.status != tracker::SubagentStatus::Running);
    rows.extend(runs.into_iter().map(AgentRow::Subagent));
    let mut bg = tasks::list_tasks();
    bg.sort_by_key(|t| t.status != BgStatus::Running);
    rows.extend(bg.into_iter().map(AgentRow::Task));
    rows
}

pub(crate) fn age_str(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

impl App {
    /// Drain finished background subagents (`task background:true`):
    /// chat notice + desktop alert always, follow-up turn with the
    /// result when idle (mirrors background bash tasks). Runs every
    /// event-loop iteration; each completion drains once. Returns the
    /// waked run ids.
    pub(crate) fn poll_subagent_completions(&mut self) -> Vec<String> {
        // Subagent transcript views are read-only peeks: never drain the
        // global completion queue here. Draining marks runs surfaced, so
        // the main chat would lose its finish notice + follow-up turn.
        // Completions wait until Esc returns to the main session.
        if self.viewing_subagent() {
            return Vec::new();
        }
        let mut waked = Vec::new();
        for done in tracker::take_completions() {
            let (mark, urgent) = match done.status {
                tracker::SubagentStatus::Done => ("⑂✔", false),
                tracker::SubagentStatus::Error => ("⑂✖", true),
                tracker::SubagentStatus::Killed => ("⑂○", false),
                tracker::SubagentStatus::Running => continue,
            };
            self.messages.push(Msg::new(
                "system",
                format!(
                    "{mark} subagent {} {} ({}) — Enter on its /agents row for detail",
                    done.kind,
                    short_task_id(&done.id),
                    done.status.as_str(),
                ),
            ));
            self.notify(
                &format!(
                    "{} subagent {} {} ({})",
                    mark,
                    done.kind,
                    short_task_id(&done.id),
                    done.status.as_str()
                ),
                urgent,
            );
            if !self.wake_on_tasks || self.model.trim().is_empty() {
                continue;
            }
            if !self.busy && self.pending.is_none() {
                self.wake_for_subagent(&done);
                waked.push(done.id.clone());
            }
            // Busy: notice only, like task wakes — the persisted chat
            // notice carries the outcome into the next turn.
        }
        waked
    }

    /// Follow-up turn carrying a finished background subagent's result.
    pub(crate) fn subagent_wake_prompt(done: &SubagentRun) -> String {
        let outcome = match done.status {
            tracker::SubagentStatus::Done => "done".to_string(),
            tracker::SubagentStatus::Error => "failed".to_string(),
            tracker::SubagentStatus::Killed => "killed".to_string(),
            tracker::SubagentStatus::Running => return String::new(),
        };
        let result = done
            .result_full
            .as_deref()
            .or(done.result_preview.as_deref())
            .unwrap_or("(no result)")
            .chars()
            .take(2000)
            .collect::<String>();
        let log_hint = if done.result_truncated {
            format!(
                "\n(Full output truncated — read {})",
                done.log_path.as_deref().unwrap_or("(log unavailable)")
            )
        } else {
            String::new()
        };
        format!(
            "[background subagent finished] {} ({}) {outcome}.\nTask was: {}\nResult:\n{result}{log_hint}\nContinue from where you left off; do not restart the finished work. If the work is complete, summarize briefly.",
            short_task_id(&done.id),
            done.kind,
            done.full_prompt.chars().take(500).collect::<String>(),
        )
    }

    pub(crate) fn wake_for_subagent(&mut self, done: &SubagentRun) {
        if done.status == tracker::SubagentStatus::Running {
            return;
        }
        self.status = format!("working on finished subagent {}…", short_task_id(&done.id));
        self.start_turn(Self::subagent_wake_prompt(done), None, "system");
    }

    /// Resume a chat by id (model/mode/display follow it). Shared by
    /// the Agents dialog; mirrors the Sessions dialog + `/resume`.
    pub(crate) fn resume_chat(&mut self, id: &str) {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        let Ok(store) = vioraharness_core::session::SessionStore::new(&db) else {
            self.report_error("resume", "resume: db error".to_string());
            return;
        };
        let Ok(Some(sess)) = store.get_session(id) else {
            self.messages.push(Msg::new(
                "system",
                format!("resume: session {id} not found"),
            ));
            return;
        };
        self.session_id = id.to_string();
        self.model = sess.model.clone();
        if let Some(m) = sess.mode.clone() {
            let norm = vioraharness_core::mode::normalize_mode_name(&m);
            if vioraharness_core::mode::is_known_mode(&norm) {
                self.agent_mode = norm;
            }
        }
        if let Some(theme) = sess.theme.clone() {
            Self::save_tui_state(serde_json::json!({"last_theme": theme}));
        }
        Self::save_tui_state(serde_json::json!({"last_model": self.model}));
        match self.reload_display_from_store(&store, id) {
            Some(n) => {
                self.ctx_freed_tokens = 0;
                self.messages.push(Msg::new(
                    "system",
                    format!("↩︎ Resumed {} ({n} msgs)", &id[..8.min(id.len())]),
                ));
            }
            None => {
                self.messages
                    .push(Msg::new("system", format!("resume: no messages for {id}")));
            }
        }
        self.scroll = 0;
        self.status = "ready".into();
    }

    /// Post a subagent run's full detail to chat (prompt + outcome).
    pub(crate) fn show_subagent_detail(&mut self, run: &SubagentRun) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(run.started_at);
        let mut detail = format!(
            "⑂ subagent {} · {} · depth {} · {} · {} · {}s",
            short_task_id(&run.id),
            run.kind,
            run.depth,
            run.mode,
            run.status.as_str(),
            run.elapsed_secs(now),
        );
        detail.push_str("\n\nTask:\n");
        detail.push_str(&run.full_prompt.chars().take(1500).collect::<String>());
        let result = run.result_full.as_deref().or(run.result_preview.as_deref());
        match result {
            Some(res) if !res.trim().is_empty() => {
                detail.push_str("\n\nResult:\n");
                detail.push_str(&res.chars().take(1500).collect::<String>());
                if run.result_truncated {
                    detail.push_str(&format!(
                        "\n\n(truncated to {} chars — full output at {})",
                        res.chars().count(),
                        run.log_path.as_deref().unwrap_or("(log unavailable)")
                    ));
                } else if let Some(p) = run.log_path.as_deref() {
                    detail.push_str(&format!("\n\n(full log: {p})"));
                }
            }
            _ => {
                if run.status == tracker::SubagentStatus::Running {
                    detail.push_str("\n\n(still running — detail refreshes on completion)");
                }
            }
        }
        self.messages.push(Msg::new("system", detail));
        self.scroll = 0;
    }

    /// Re-anchor the cursor id to the current index after any move.
    /// Moves are relative (±1/page) so the index is right at move time;
    /// the id is what keeps Enter honest when the list re-sorts later.
    pub(crate) fn anchor_agent_cursor(&mut self) {
        let rows = agent_rows(self);
        if rows.is_empty() {
            self.agent_cursor_id = None;
            return;
        }
        self.agent_cursor = self.agent_cursor.min(rows.len() - 1);
        self.agent_cursor_id = rows.get(self.agent_cursor).map(|r| r.row_id());
    }

    /// Resolve the highlighted row: prefer the anchored id against fresh
    /// rows, fall back to the clamped index when the row is gone
    /// (eviction/prune) or was never anchored (older flows, tests).
    pub(crate) fn resolve_agent_row(&mut self) -> Option<AgentRow> {
        let rows = agent_rows(self);
        if rows.is_empty() {
            return None;
        }
        if let Some(want) = self.agent_cursor_id.clone() {
            if let Some(pos) = rows.iter().position(|r| r.row_id() == want) {
                self.agent_cursor = pos;
                return rows.into_iter().nth(pos);
            }
        }
        self.agent_cursor = self.agent_cursor.min(rows.len() - 1);
        self.agent_cursor_id = rows.get(self.agent_cursor).map(|r| r.row_id());
        rows.into_iter().nth(self.agent_cursor)
    }

    /// Read-only highlight index for render: same id-first resolution
    /// without mutating (render takes `&self`).
    pub(crate) fn agent_highlight_index(&self, rows: &[AgentRow]) -> usize {
        if rows.is_empty() {
            return 0;
        }
        if let Some(want) = self.agent_cursor_id.as_deref() {
            if let Some(pos) = rows.iter().position(|r| r.row_id() == want) {
                return pos;
            }
        }
        self.agent_cursor.min(rows.len() - 1)
    }

    /// Announce newly-launched subagents in main chat with their title
    /// (prompt preview), so a spawn is visible without opening /agents.
    /// Runs every event-loop iteration. History hydrated from sqlite is
    /// never announced: only runs started after boot qualify, and each
    /// id announces once.
    pub(crate) fn poll_subagent_starts(&mut self) {
        // Read-only peek: launch announcements belong to the main chat.
        // Posting them here would pollute the transcript being viewed.
        if self.viewing_subagent() {
            return;
        }
        for run in tracker::list_runs() {
            if run.started_at < self.booted_at {
                continue;
            }
            if self.seen_subagents.insert(run.id.clone()) {
                let title: String = run.prompt_preview.chars().take(100).collect();
                self.messages.push(Msg::new(
                    "system",
                    format!(
                        "⑂ subagent {} {} launched — {title} (/agents to watch)",
                        run.kind,
                        short_task_id(&run.id),
                    ),
                ));
            }
        }
    }

    /// True while viewing any subagent transcript (`sub-` session): input
    /// stays hidden and all turns are refused — only the main agent owns
    /// the input box. Holds even after the run finishes; Esc returns.
    pub(crate) fn viewing_subagent(&self) -> bool {
        self.session_id.starts_with(tracker::SUB_SESSION_PREFIX)
    }

    /// Chat whose subagents the /agents dialog lists: the current chat,
    /// or the originating chat while peeking at a subagent transcript
    /// (so the dialog stays useful instead of going empty mid-peek).
    pub(crate) fn agents_scope_session(&self) -> &str {
        if self.viewing_subagent() {
            self.return_session.as_deref().unwrap_or(&self.session_id)
        } else {
            &self.session_id
        }
    }

    /// Live-refresh the open subagent transcript (called every event-loop
    /// tick). The view loads once on Enter, but a running subagent keeps
    /// appending to its session in the store — without this the view looks
    /// frozen until the user leaves and re-enters. Reloads at most once
    /// per second while the linked run is still Running, plus one final
    /// reload when it lands. Scroll position is preserved (draw_chat holds
    /// scrolled-up views steady); refuses nothing, mutates only this view.
    /// Returns true when the view changed and needs a redraw.
    pub(crate) fn poll_subview_refresh(&mut self) -> bool {
        if !self.viewing_subagent() {
            return false;
        }
        if self.subview_last_refresh.elapsed() < std::time::Duration::from_secs(1) {
            return false;
        }
        self.subview_last_refresh = std::time::Instant::now();
        let run = tracker::run_for_session(&self.session_id);
        let running = run
            .as_ref()
            .is_some_and(|r| r.status == tracker::SubagentStatus::Running);
        if !running {
            if self.subview_finalized {
                return false;
            }
            self.subview_finalized = true;
        }
        let finishing = !running;
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        let Ok(store) = vioraharness_core::session::SessionStore::new(&db) else {
            return false;
        };
        let sid = self.session_id.clone();
        let before = self.messages.len();
        self.reload_display_from_store(&store, &sid);
        if !running {
            let state = run.map(|r| r.status.as_str().to_string());
            self.status = match state.as_deref() {
                Some(s) => format!("subagent {s} — Esc back to main"),
                None => "subagent transcript — read-only, no input (Esc back to main)".into(),
            };
        }
        // Tool results can land under an existing message row (same count,
        // richer items) — redraw whenever the run is still live. The
        // finishing transition also redraws (status flips to done) even
        // with no new rows: the result already streamed in while live.
        running || finishing || self.messages.len() != before
    }

    /// Enter on the highlighted Agents row: contextual navigation.
    /// Id-anchored (see `resolve_agent_row`): entering a subagent can
    /// never land on Live just because the list re-sorted. A linked run
    /// opens its transcript session — the subagent's own view, always
    /// read-only with no input box (Esc returns). Rows from before the
    /// session link fall back to the detail post.
    pub(crate) fn agents_activate(&mut self) {
        let Some(row) = self.resolve_agent_row() else {
            return;
        };
        match row {
            AgentRow::Live => {
                self.popup = Popup::None;
                self.status = "already here — this is the live turn".into();
            }
            AgentRow::Subagent(run) => {
                if let Some(sid) = run.session_id.clone() {
                    if sid != self.session_id {
                        self.return_session = Some(self.session_id.clone());
                    }
                    self.resume_chat(&sid);
                    self.subview_finalized = false;
                    self.subview_last_refresh = std::time::Instant::now();
                    self.popup = Popup::None;
                    self.status =
                        "subagent transcript — read-only, no input (Esc back to main)".into();
                    return;
                }
                self.show_subagent_detail(&run);
                self.popup = Popup::None;
            }
            AgentRow::Task(t) => {
                // Jump to the Tasks panel focused on this task.
                let all = tasks::list_tasks();
                if let Some(idx) = all.iter().position(|x| x.id == t.id) {
                    self.task_cursor = idx;
                }
                self.popup = Popup::Tasks;
            }
        }
    }

    /// `x` on the highlighted row: kill a running background task or
    /// detached subagent. Id-anchored like activation.
    pub(crate) fn agents_kill(&mut self) {
        match self.resolve_agent_row() {
            Some(AgentRow::Task(t)) => {
                if tasks::kill_task(&t.id) {
                    self.status = format!("killed {}", short_task_id(&t.id));
                } else {
                    self.status = format!("{} not running", short_task_id(&t.id));
                }
            }
            Some(AgentRow::Subagent(run)) => {
                if tracker::cancel_run(&run.id) {
                    self.status = format!("cancelled subagent {}", short_task_id(&run.id));
                } else {
                    self.status = format!("subagent {} not running", short_task_id(&run.id));
                }
            }
            Some(_) => {
                self.status = "x only kills background tasks/subagents".into();
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::*;
    use super::*;

    /// Temp DB + temp XDG home under the process-global env lock.
    /// `resume_chat` touches both (`VIORAHARNESS_DB` sessions and
    /// `tui_state.json`), so tests must not leak into the real ones.
    struct AgentsEnv {
        prev_db: Option<String>,
        prev_xdg: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
        _dir: std::path::PathBuf,
    }

    fn agents_env(tag: &str) -> (AgentsEnv, std::path::PathBuf) {
        let guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vh_agents_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev_db = std::env::var("VIORAHARNESS_DB").ok();
        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let db = dir.join("sessions.db");
        std::env::set_var("VIORAHARNESS_DB", &db);
        std::env::set_var("XDG_DATA_HOME", &dir);
        (
            AgentsEnv {
                prev_db,
                prev_xdg,
                _guard: guard,
                _dir: dir.clone(),
            },
            db,
        )
    }

    impl Drop for AgentsEnv {
        fn drop(&mut self) {
            match &self.prev_db {
                Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
                None => std::env::remove_var("VIORAHARNESS_DB"),
            }
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    fn unique_prompt(tag: &str) -> String {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("agents-probe-{tag}-{n}-{}", std::process::id())
    }

    /// Completions drain globally (`take_completions` marks everything
    /// surfaced), so consumer tests serialize on this lock — otherwise
    /// a parallel test's poll can eat another's completion.
    fn completion_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
            std::sync::LazyLock::new(|| std::sync::Mutex::new(()));
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn agents_command_opens_dialog() {
        let mut app = test_app();
        app.handle_slash("/agents");
        assert_eq!(app.popup, Popup::Agents);
        app.handle_slash("/agent");
        assert_eq!(app.popup, Popup::Agents);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Agents"), "dialog title: {text:?}");
    }

    #[tokio::test]
    async fn agents_opens_while_busy() {
        use crossterm::event::KeyCode;
        let mut app = test_app();
        app.busy = true;
        app.input.text = "/agents".into();
        app.input.cursor = 7;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.popup, Popup::Agents, "read-only dialog is busy-safe");
    }

    #[test]
    fn rows_lead_with_live_then_active_subagents() {
        let _lock = completion_lock();
        let prompt = unique_prompt("rows");
        let (id, _) = tracker::track_start("explore", &prompt, "eda");
        let mut app = test_app();
        tracker::set_run_parent(&id, &app.session_id);
        let rows = agent_rows(&app);
        assert!(matches!(rows[0], AgentRow::Live), "live turn first");
        let pos = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            .expect("tracked run listed");
        assert!(pos >= 1, "after live");
        // Index 0 is structurally Live (never shifts): activate closes.
        app.popup = Popup::Agents;
        app.agent_cursor = 0;
        app.anchor_agent_cursor();
        app.agents_activate();
        assert_eq!(app.popup, Popup::None);
        tracker::track_finish(&id, true, "done");
        let _ = tracker::take_completions();
    }

    #[test]
    fn subagent_enter_posts_detail() {
        let _lock = completion_lock();
        let prompt = unique_prompt("detail");
        let (id, _) = tracker::track_start("reviewer", &prompt, "web");
        tracker::track_finish(&id, true, "looks good");
        let mut app = test_app();
        tracker::set_run_parent(&id, &app.session_id);
        app.popup = Popup::Agents;
        // Rows rebuild inside activate; a concurrent spawn from another
        // test can shift indices between snapshot and use — verify.
        let mut ok = false;
        for _ in 0..20 {
            let rows = agent_rows(&app);
            let Some(pos) = rows
                .iter()
                .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            else {
                continue;
            };
            app.agent_cursor = pos;
            app.anchor_agent_cursor();
            app.agents_activate();
            if app
                .messages
                .last()
                .is_some_and(|m| m.content.contains(&prompt[..24]))
            {
                ok = true;
                break;
            }
            app.popup = Popup::Agents;
        }
        assert!(ok, "detail posted for our run");
        let last = app.messages.last().expect("detail posted");
        assert!(last.content.contains("reviewer"), "{:?}", last.content);
        assert!(last.content.contains("looks good"), "result shown");
        let _ = tracker::take_completions();
    }

    #[tokio::test]
    async fn task_enter_jumps_to_tasks_focused() {
        let _lock = completion_lock();
        let live = tasks::spawn_task("sleep 30", "/tmp");
        let mut app = test_app();
        app.popup = Popup::Agents;
        // The task registry is shared with parallel tests: another
        // spawn landing between the cursor snapshot and activation can
        // shift indices, so resolve + verify in a short retry loop.
        let mut focused = false;
        for _ in 0..20 {
            let rows = agent_rows(&app);
            let Some(pos) = rows
                .iter()
                .position(|r| matches!(r, AgentRow::Task(t) if t.id == live.id))
            else {
                continue;
            };
            app.agent_cursor = pos;
            app.anchor_agent_cursor();
            app.agents_activate();
            if app.popup != Popup::Tasks {
                continue;
            }
            let all = tasks::list_tasks();
            if all.get(app.task_cursor).map(|t| &t.id) == Some(&live.id) {
                focused = true;
                break;
            }
            app.popup = Popup::Agents;
        }
        assert!(focused, "jumps to Tasks focused on the task");
        assert!(tasks::kill_task(&live.id), "cleanup");
        // x on a non-task row hints instead of killing.
        app.popup = Popup::Agents;
        app.agent_cursor = 0;
        app.anchor_agent_cursor();
        app.agents_kill();
        assert!(app.status.contains("x only kills"), "{}", app.status);
    }

    #[tokio::test]
    async fn enter_running_linked_run_opens_readonly_transcript() {
        // A subagent transcript is always read-only with no input box —
        // running or finished, only the main agent takes input.
        let (_env, db) = agents_env("sublive");
        let _lock = completion_lock();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session("sub-sa_live1", "model-s", Some("live transcript"))
            .unwrap();
        store
            .append_message("sub-sa_live1", "user", "live work xyz")
            .unwrap();
        let (id, _) = tracker::track_start("coder", &unique_prompt("live"), "eda");
        tracker::set_run_session(&id, "sub-sa_live1");
        let mut app = test_app();
        app.session_id = "sess-main".into();
        tracker::set_run_parent(&id, "sess-main");
        app.popup = Popup::Agents;
        let rows = agent_rows(&app);
        app.agent_cursor = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            .expect("run listed");
        app.anchor_agent_cursor();
        app.agents_activate();
        assert_eq!(app.session_id, "sub-sa_live1", "live transcript opened");
        assert_eq!(app.popup, Popup::None, "dialog closes into the view");
        assert!(app.status.contains("read-only"), "{}", app.status);
        // Typing there is refused while the run is live...
        app.start_turn("hijack attempt".into(), None, "user");
        assert!(!app.busy, "no turn started");
        assert!(
            app.messages.iter().any(|m| m.content.contains("read-only")),
            "refusal explained"
        );
        // ...and still refused after it finishes — only main owns input.
        tracker::track_finish(&id, true, "all done");
        app.start_turn("follow up".into(), None, "user");
        assert!(!app.busy, "still no turn in subagent view");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("Esc back to main")),
            "refusal points home"
        );
        let _ = tracker::take_completions();
    }

    #[tokio::test]
    async fn subagent_completion_wakes_idle_chat() {
        let _lock = completion_lock();
        // Stale undrained completions from sibling tests would wake first
        // (the first wake flips busy, the rest get notice-only in the same
        // poll) — clear the backlog so ours is the waker.
        let _ = tracker::take_completions();
        let (id, _) = tracker::track_start("coder", &unique_prompt("wake"), "eda");
        tracker::track_finish(&id, true, "the answer is 42");
        let mut app = test_app();
        app.wake_on_tasks = true;
        let waked = app.poll_subagent_completions();
        assert!(waked.contains(&id), "waked: {waked:?}");
        assert!(app.busy, "follow-up turn started");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&id))),
            "completion notice posted"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;
    }

    #[test]
    fn subagent_completion_stays_quiet_when_busy_or_off() {
        let _lock = completion_lock();
        let (id, _) = tracker::track_start("explore", &unique_prompt("quiet"), "eda");
        tracker::track_finish(&id, true, "quiet result");
        let mut app = test_app();
        app.busy = true;
        let waked = app.poll_subagent_completions();
        assert!(!waked.contains(&id), "no wake while busy");
        assert!(app.pending.is_none(), "no turn started while busy");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&id))),
            "notice still posted"
        );

        let (id2, _) = tracker::track_start("explore", &unique_prompt("quiet2"), "eda");
        tracker::track_finish(&id2, true, "quiet result 2");
        app.busy = false;
        app.wake_on_tasks = false;
        let waked = app.poll_subagent_completions();
        assert!(!waked.contains(&id2), "no wake when disabled");
        assert!(!app.busy, "stays idle");
    }

    #[test]
    fn agents_kill_finished_subagent_hints() {
        let _lock = completion_lock();
        let (id, _) = tracker::track_start("explore", &unique_prompt("killhint"), "eda");
        tracker::track_finish(&id, true, "done");
        let _ = tracker::take_completions();
        let mut app = test_app();
        tracker::set_run_parent(&id, &app.session_id);
        app.popup = Popup::Agents;
        let rows = agent_rows(&app);
        app.agent_cursor = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            .expect("finished run listed");
        app.anchor_agent_cursor();
        app.agents_kill();
        assert!(app.status.contains("not running"), "{}", app.status);
    }

    #[test]
    fn enter_hits_anchored_row_after_resort() {
        // Regression: the cursor is a bare index into a list that re-sorts
        // (active-first) on every rebuild. A stale index used to land on
        // Live — "the subagent opened the main agent". Id-anchoring must
        // survive the resort.
        let _lock = completion_lock();
        let prompt = unique_prompt("anchored");
        let (id, _) = tracker::track_start("explore", &prompt, "eda");
        let mut app = test_app();
        tracker::set_run_parent(&id, &app.session_id);
        app.popup = Popup::Agents;
        let rows = agent_rows(&app);
        app.agent_cursor = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            .expect("run listed");
        app.anchor_agent_cursor();
        // Simulate the resort happening between render and Enter: the
        // subagent is no longer at index 1, cursor clobbered to Live.
        app.agent_cursor = 0;
        app.agents_activate();
        assert!(
            app.messages
                .last()
                .is_some_and(|m| m.content.contains(&prompt[..24])),
            "anchored subagent detail posted, not Live"
        );
        assert_eq!(app.popup, Popup::None, "unlinked row posts and closes");
        tracker::track_finish(&id, true, "done");
        let _ = tracker::take_completions();
    }

    #[test]
    fn enter_finished_linked_run_opens_its_session() {
        // The subagent's own view: Enter on a finished linked run resumes
        // its transcript session instead of posting a summary in main chat.
        let (_env, db) = agents_env("subview");
        let _lock = completion_lock();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session("sub-sa_view1", "model-s", Some("sub transcript"))
            .unwrap();
        store
            .append_message("sub-sa_view1", "user", "sub hello xyz")
            .unwrap();
        let (id, _) = tracker::track_start("explore", &unique_prompt("view"), "eda");
        tracker::set_run_session(&id, "sub-sa_view1");
        tracker::track_finish(&id, true, "all done");
        let _ = tracker::take_completions();
        let mut app = test_app();
        app.session_id = "sess-main".into();
        tracker::set_run_parent(&id, "sess-main");
        app.popup = Popup::Agents;
        let rows = agent_rows(&app);
        app.agent_cursor = rows
            .iter()
            .position(|r| matches!(r, AgentRow::Subagent(run) if run.id == id))
            .expect("run listed");
        app.anchor_agent_cursor();
        app.agents_activate();
        assert_eq!(app.session_id, "sub-sa_view1", "switched to transcript");
        assert_eq!(app.popup, Popup::None);
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("sub hello xyz")),
            "transcript loaded, not a summary"
        );
    }

    #[test]
    fn launch_notice_posts_title_once() {
        let _lock = completion_lock();
        let prompt = unique_prompt("launched-title-xyz");
        let mut app = test_app();
        // Pre-boot history (hydrated from sqlite) must stay silent.
        app.booted_at = i64::MAX;
        let (id, _) = tracker::track_start("explore", &prompt, "eda");
        app.poll_subagent_starts();
        assert!(
            !app.messages.iter().any(|m| m.content.contains(&prompt)),
            "pre-boot run stays silent"
        );
        // A genuinely new launch announces kind + title, exactly once.
        app.booted_at = 0;
        app.poll_subagent_starts();
        let hits: Vec<_> = app
            .messages
            .iter()
            .filter(|m| m.content.contains(&prompt))
            .collect();
        assert_eq!(hits.len(), 1, "launch announced once");
        assert!(hits[0].content.contains("explore"), "{}", hits[0].content);
        assert!(hits[0].content.contains("launched"), "{}", hits[0].content);
        app.poll_subagent_starts();
        assert_eq!(
            app.messages
                .iter()
                .filter(|m| m.content.contains(&prompt))
                .count(),
            1,
            "no repeat announcements"
        );
        tracker::track_finish(&id, true, "done");
        let _ = tracker::take_completions();
    }

    #[tokio::test]
    async fn esc_returns_from_subview_to_main() {
        use crossterm::event::KeyCode;
        let (_env, db) = agents_env("escback");
        let _lock = completion_lock();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session("sess-home", "model-h", Some("Home"))
            .unwrap();
        store
            .append_message("sess-home", "user", "home sweet home")
            .unwrap();
        store
            .create_session("sub-sa_esc1", "model-s", Some("sub"))
            .unwrap();
        let mut app = test_app();
        // Sitting in a subagent view with a way back...
        app.session_id = "sub-sa_esc1".into();
        app.return_session = Some("sess-home".into());
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert_eq!(app.session_id, "sess-home", "Esc returns to main");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("home sweet home")),
            "main transcript restored"
        );
        // ...but Esc in a normal chat keeps its classic behavior.
        app.input.text = "draft".into();
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.input.text.is_empty(), "Esc still clears input");
        assert_eq!(app.session_id, "sess-home", "stays put");
    }

    #[tokio::test]
    async fn locked_subview_hides_input_and_swallows_edits() {
        use crossterm::event::KeyCode;
        let _lock = completion_lock();
        assert!(!test_app().viewing_subagent(), "main chat editable");
        let (id, _) = tracker::track_start("explore", &unique_prompt("lockview"), "eda");
        tracker::set_run_session(&id, "sub-sa_lock1");
        let mut app = test_app();
        app.session_id = "sub-sa_lock1".into();
        assert!(app.viewing_subagent(), "subagent view has no input");
        // No textbox rendered at all — not even a read-only bar.
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("Enter send"), "no input chrome");
        assert!(!text.contains("Input —"), "input box gone");
        assert!(
            app.input_area.width == 0 && app.input_area.height == 0,
            "layout collapsed"
        );
        // ...and keystrokes never reach it.
        app.handle_key(KeyCode::Char('x')).await.unwrap();
        assert!(app.input.text.is_empty(), "char swallowed");
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert!(!app.busy, "enter submits nothing");
        // Finished transcripts stay input-less too — only main owns input.
        tracker::track_finish(&id, true, "done");
        assert!(app.viewing_subagent(), "still no input after finish");
        app.handle_key(KeyCode::Char('y')).await.unwrap();
        assert!(app.input.text.is_empty(), "still swallowed");
        app.start_turn("follow up".into(), None, "user");
        assert!(!app.busy, "turns refused after finish too");
        let _ = tracker::take_completions();
    }

    #[test]
    fn subview_defers_completions_and_launches_to_main() {
        // Regression for the "messy texts" report: peeking at a subagent
        // transcript while a run finishes must not drain the global queues
        // into the peeked view (draining marks runs surfaced, so the main
        // chat would lose its notice + follow-up turn, and the notice text
        // would mix two transcripts on one screen).
        let _lock = completion_lock();
        let prompt = unique_prompt("defer");
        let (id, _) = tracker::track_start("explore", &prompt, "eda");
        tracker::track_finish(&id, true, "deferred result");
        let mut app = test_app();
        app.booted_at = 0;
        let before = app.messages.len();
        app.session_id = "sub-sa_defer1".into();
        assert!(app.viewing_subagent(), "peek is read-only");
        let waked = app.poll_subagent_completions();
        assert!(waked.is_empty(), "no wake from a transcript peek");
        assert_eq!(app.messages.len(), before, "no notice in sub view");
        app.poll_subagent_starts();
        assert_eq!(app.messages.len(), before, "no launch noise in sub view");
        // Still queued for the main chat — deferred, not lost.
        let pending = tracker::take_completions();
        assert!(
            pending.iter().any(|r| r.id == id),
            "completion survives the peek"
        );
    }

    #[test]
    fn subview_refuses_system_turns_too() {
        // Follow-up wakes (task/subagent completions) must wait for the
        // return to main: starting one here would persist main-chat
        // content into the sub session.
        let mut app = test_app();
        app.session_id = "sub-sa_sysref1".into();
        assert!(app.viewing_subagent());
        app.start_turn("wake".into(), None, "system");
        assert!(!app.busy, "no system turn in sub view");
        assert!(app.pending.is_none(), "nothing spawned");
        assert!(
            app.messages.iter().any(|m| m.content.contains("read-only")),
            "refusal explained"
        );
    }

    #[test]
    fn subview_refreshes_live_transcript_without_reenter() {
        // The open transcript must follow a running subagent: new store
        // rows appear via the periodic refresh, plus one final reload
        // when the run lands — then ticks stay quiet.
        let (_env, db) = agents_env("subrefresh");
        let _lock = completion_lock();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session("sub-sa_refr1", "model-s", Some("live"))
            .unwrap();
        store
            .append_message("sub-sa_refr1", "user", "first xyz")
            .unwrap();
        let (id, _) = tracker::track_start("explore", &unique_prompt("refresh"), "eda");
        tracker::set_run_session(&id, "sub-sa_refr1");
        let past = || std::time::Instant::now() - std::time::Duration::from_secs(5);
        let mut app = test_app();
        app.session_id = "sub-sa_refr1".into();
        app.subview_last_refresh = past();
        assert!(app.poll_subview_refresh(), "loads running transcript");
        assert!(
            app.messages.iter().any(|m| m.content.contains("first xyz")),
            "first row shown"
        );
        store
            .append_message("sub-sa_refr1", "assistant", "second xyz")
            .unwrap();
        app.subview_last_refresh = past();
        assert!(app.poll_subview_refresh(), "new rows stream in");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("second xyz")),
            "no re-enter needed"
        );
        tracker::track_finish(&id, true, "done");
        app.subview_last_refresh = past();
        assert!(app.poll_subview_refresh(), "final reload on finish");
        assert!(app.status.contains("Esc back to main"), "{}", app.status);
        app.subview_last_refresh = past();
        assert!(!app.poll_subview_refresh(), "quiet once finalized");
        let _ = tracker::take_completions();
    }

    #[test]
    fn dialog_lists_only_current_chat_subagents() {
        // /agents is per-chat: runs spawned by other chats (or from
        // before the parent link existed) never crowd the list. While
        // peeking at a transcript the scope is the originating chat.
        let _lock = completion_lock();
        let (mine, _) = tracker::track_start("explore", &unique_prompt("mine"), "eda");
        let (theirs, _) = tracker::track_start("coder", &unique_prompt("theirs"), "eda");
        let (legacy, _) = tracker::track_start("reviewer", &unique_prompt("legacy"), "eda");
        tracker::track_finish(&legacy, true, "old");
        let mut app = test_app();
        app.session_id = "sess-home-chat".into();
        tracker::set_run_parent(&mine, "sess-home-chat");
        tracker::set_run_parent(&theirs, "sess-other-chat");
        let rows = agent_rows(&app);
        assert!(
            rows.iter()
                .any(|r| matches!(r, AgentRow::Subagent(run) if run.id == mine)),
            "own run listed"
        );
        assert!(
            rows.iter()
                .all(|r| !matches!(r, AgentRow::Subagent(run) if run.id == theirs)),
            "other chat's run hidden"
        );
        assert!(
            rows.iter()
                .all(|r| !matches!(r, AgentRow::Subagent(run) if run.id == legacy)),
            "unattributed legacy run hidden"
        );
        // Peeking at a transcript scopes to the chat we came from.
        app.session_id = "sub-sa_peek1".into();
        app.return_session = Some("sess-home-chat".into());
        let rows = agent_rows(&app);
        assert!(
            rows.iter()
                .any(|r| matches!(r, AgentRow::Subagent(run) if run.id == mine)),
            "originating chat's runs visible mid-peek"
        );
        tracker::track_finish(&mine, true, "done");
        tracker::track_finish(&theirs, true, "done");
        let _ = tracker::take_completions();
    }

    #[test]
    fn dialog_excludes_recent_chats() {
        // Regression for "why i see all of these": repeated explore
        // sessions in the same project used to fill /agents with 8 chat
        // rows. Chats live in /sessions now — this dialog is live +
        // this chat's subagents + tasks only.
        let (_env, db) = agents_env("nochats");
        let _lock = completion_lock();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        for i in 0..3 {
            store
                .create_session(
                    &format!("sess-chat-{i}"),
                    "m",
                    Some("Running Subagent To Explore"),
                )
                .unwrap();
        }
        let mut app = test_app();
        app.session_id = "sess-chat-0".into();
        let rows = agent_rows(&app);
        assert!(matches!(rows[0], AgentRow::Live), "live first");
        assert!(
            rows.iter().all(|r| matches!(
                r,
                AgentRow::Live | AgentRow::Subagent(_) | AgentRow::Task(_)
            )),
            "no chat rows: {rows:?}"
        );
        app.popup = Popup::Agents;
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("chat ·"), "no chat rows rendered: {text:?}");
        assert!(
            text.contains("subagents •"),
            "header without chats section: {text:?}"
        );
    }
}
