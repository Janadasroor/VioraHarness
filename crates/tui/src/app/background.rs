// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl App {
    pub(crate) fn poll_task_completions(&mut self) -> Vec<String> {
        use vioraharness_core::tools::tasks::BgStatus;
        // Same read-only rule as subagent completions: a transcript peek
        // must not drain the global task queue (draining marks surfaced)
        // nor persist notices into the sub session. Everything waits for
        // the return to the main chat.
        if self.viewing_subagent() {
            return Vec::new();
        }
        let mut waked = Vec::new();
        for done in vioraharness_core::tools::tasks::take_completions() {
            let (mark, detail) = match done.status {
                BgStatus::Done => ("✔", "exit 0".to_string()),
                BgStatus::Error => (
                    "✖",
                    done.exit_code
                        .map(|c| format!("exit {c}"))
                        .unwrap_or_else(|| "failed".to_string()),
                ),
                BgStatus::Killed => ("○", "killed".to_string()),
                BgStatus::Running => continue,
            };
            self.messages.push(Msg::new(
                "system",
                format!(
                    "{mark} task {} {} ({detail}) — /tasks to view log",
                    short_task_id(&done.id),
                    done.status.as_str()
                ),
            ));
            self.notify(
                &format!(
                    "{} task {} {} ({detail})",
                    mark,
                    short_task_id(&done.id),
                    done.status.as_str()
                ),
                done.status == BgStatus::Error,
            );
            self.note_task_completion(&done);
            if !self.wake_on_tasks || self.model.trim().is_empty() {
                continue;
            }
            if !self.busy && self.pending.is_none() {
                self.wake_for_task(&done);
                waked.push(done.id.clone());
            } else {
                // Turn running: notice only, never a queued wake prompt.
                // Queued wakes pile up behind live turns and read like the
                // chat talking to itself; the notice above is already
                // persisted, so the next turn still sees the outcome.
                waked.push(done.id.clone());
            }
        }
        waked
    }

    /// Follow-up prompt for a finished background task (log tail included).
    pub(crate) fn task_wake_prompt(done: &vioraharness_core::tools::tasks::BgTask) -> String {
        use vioraharness_core::tools::tasks::BgStatus;
        let tail = vioraharness_core::tools::tasks::read_task_log(&done.id).unwrap_or_default();
        let tail: String = tail
            .chars()
            .rev()
            .take(1500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let outcome = match done.status {
            BgStatus::Done => "done (exit 0)".to_string(),
            BgStatus::Error => done
                .exit_code
                .map(|c| format!("failed (exit {c})"))
                .unwrap_or_else(|| "failed".to_string()),
            BgStatus::Killed => "killed".to_string(),
            BgStatus::Running => return String::new(),
        };
        format!(
            "[background task finished] {} (`{}`) {outcome}.\nLog tail:\n{tail}\nContinue from where you left off; do not restart the finished command. If the work is complete, summarize briefly.",
            short_task_id(&done.id),
            done.command,
        )
    }

    pub(crate) fn note_task_completion(&mut self, done: &vioraharness_core::tools::tasks::BgTask) {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
            let _ = store.append_message(
                &self.session_id,
                "system",
                &format!(
                    "background task {} (`{}`) {} — log: {}",
                    short_task_id(&done.id),
                    done.command.chars().take(120).collect::<String>(),
                    done.status.as_str(),
                    done.log_path,
                ),
            );
        }
    }

    pub(crate) fn wake_for_task(&mut self, done: &vioraharness_core::tools::tasks::BgTask) {
        use vioraharness_core::tools::tasks::BgStatus;
        if done.status == BgStatus::Running {
            return;
        }
        self.status = format!("working on finished task {}…", short_task_id(&done.id));
        self.start_turn(Self::task_wake_prompt(done), None, "system");
    }

    pub(crate) fn is_busy_safe_slash(text: &str) -> bool {
        matches!(
            text.split_whitespace().next().unwrap_or(""),
            "/help"
                | "/h"
                | "/sessions"
                | "/chats"
                | "/history"
                | "/conversations"
                | "/ls"
                | "/convs"
                | "/providers"
                | "/provider"
                | "/auth"
                | "/keys"
                | "/model"
                | "/mode"
                | "/modes"
                | "/skills"
                | "/skill"
                | "/theme"
                | "/settings"
                | "/setting"
                | "/config"
                | "/options"
                | "/preferences"
                | "/thinking"
                | "/permissions"
                | "/perms"
                | "/verbosity"
                | "/verbose"
                | "/cards"
                | "/diff"
                | "/output"
                | "/view"
                | "/tool"
                | "/out"
                | "/tasks"
                | "/task"
                | "/bg"
                | "/jobs"
                | "/agents"
                | "/agent"
                | "/errors"
                | "/error"
                | "/err"
                | "/export"
                | "/compact"
                | "/quit"
                | "/q"
                | "/exit"
        )
    }

    pub(crate) fn queue_prompt(&mut self, prompt: String) {
        if self.viewing_subagent() {
            self.status = "read-only: subagent view (Esc back to main)".into();
            self.input.text.clear();
            self.input.cursor = 0;
            return;
        }
        let history_text = prompt.clone();
        let mut send = expand_paste_chips(&prompt, &self.pending_texts);
        self.pending_texts.clear();
        let mut pending_img = self.pending_image.take();
        if let Some(img) = &pending_img {
            let chip = image_chip(&img.label);
            if send.contains(&chip) {
                send = send.replace(&chip, &format!("[image {} attached]", img.label));
            } else {
                pending_img = None;
            }
        }
        // No chat echo here: the prompt appears when its turn starts (see
        // drain_queue), so display order always matches execution order.
        self.input.push_history(history_text);
        self.input.text.clear();
        self.input.cursor = 0;
        self.queued_prompts.push(QueuedPrompt {
            session_id: self.session_id.clone(),
            send,
            image: pending_img.map(|img| (img.mime.to_string(), img.b64)),
            slash: false,
            echo: true,
        });
        let n = self.queued_prompts.len();
        self.status = if n == 1 {
            "queued — sends when the turn finishes".into()
        } else {
            format!("queued ({n} waiting) — send in order when idle")
        };
    }

    /// Move unpicked `$` instant prompts to the queue head (no re-echo —
    /// they already echoed when typed). Called when a turn ends or is
    /// cancelled so displayed prompts are never orphaned. Returns how many
    /// were promoted.
    pub(crate) fn promote_instant_leftovers(&mut self) -> usize {
        let Some(inj) = self.instant_injector.take() else {
            return 0;
        };
        // Leftovers belong to the finished turn's session, not whatever
        // view is open now (a mid-turn peek must not re-tag them).
        let owner = self
            .turn_session
            .take()
            .unwrap_or_else(|| self.session_id.clone());
        let leftovers = inj.drain();
        let n = leftovers.len();
        // Reverse-insert so the oldest leftover ends up at the head.
        for send in leftovers.into_iter().rev() {
            self.queued_prompts.insert(
                0,
                QueuedPrompt {
                    session_id: owner.clone(),
                    send,
                    image: None,
                    slash: false,
                    echo: false,
                },
            );
        }
        n
    }

    /// Send queued prompts in order, but only once the agent is fully
    /// idle — never injected into a running turn. Each prompt is echoed
    /// into chat as its turn starts (never at queue time), so display
    /// order always matches execution order. Deferred slash commands
    /// run through the command handler; the first model prompt starts a
    /// turn and the rest wait for the next idle drain.
    pub(crate) fn drain_queue(&mut self) {
        if self.busy || self.pending.is_some() || self.model.trim().is_empty() {
            return;
        }
        // Read-only peek: queued prompts belong to the main chat. Draining
        // here would echo main-chat prompts into the transcript view and
        // start main turns under the sub session. Wait for Esc.
        if self.viewing_subagent() {
            return;
        }
        loop {
            while let Some(head) = self.queued_prompts.first() {
                if head.session_id != self.session_id {
                    self.queued_prompts.remove(0);
                    self.messages.push(Msg::new(
                        "system",
                        "dropped queued prompt (session changed)",
                    ));
                    continue;
                }
                break;
            }
            let Some(head) = self.queued_prompts.first() else {
                return;
            };
            if head.slash {
                let q = self.queued_prompts.remove(0);
                self.handle_slash(&q.send);
                if self.should_quit || self.busy || self.pending.is_some() {
                    // Quitting, or the command started a turn: the rest
                    // waits for the next idle drain.
                    return;
                }
                continue;
            }
            let q = self.queued_prompts.remove(0);
            if !self.queued_prompts.is_empty() {
                self.status = format!(
                    "sending queued prompt ({} more waiting)…",
                    self.queued_prompts.len()
                );
            }
            if q.echo {
                self.messages.push(Msg::new("user", q.send.clone()));
            }
            self.start_turn(q.send, q.image, "user");
            return;
        }
    }

    pub(crate) fn poll_compact(&mut self) {
        let Some((sid, mut rx)) = self.compact_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(rep)) => {
                if rep.compacted {
                    if sid == self.session_id {
                        let db = std::env::var("VIORAHARNESS_DB")
                            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                        if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                            let n = self.reload_display_from_store(&store, &sid).unwrap_or(0);
                            self.ctx_freed_tokens = 0;
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "✂ {} ({n} msgs shown)",
                                    vioraharness_core::context::compaction::compact_notice(&rep)
                                ),
                            ));
                        }
                        self.scroll = 0;
                    } else {
                        self.messages.push(Msg::new(
                            "system",
                            format!("compacted {sid} (session switched — use /resume to see it)"),
                        ));
                    }
                    self.status = "ready".into();
                } else {
                    self.messages.push(Msg::new("system", rep.note));
                    self.status = "ready".into();
                }
            }
            Ok(Err(e)) => {
                self.report_error("compact", format!("compact failed: {e}"));
                self.status = "ready".into();
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.compact_rx = Some((sid, rx));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.messages.push(Msg::new(
                    "system",
                    "compact task ended without a result".to_string(),
                ));
                self.status = "ready".into();
            }
        }
    }

    pub(crate) fn is_known_slash(text: &str) -> bool {
        matches!(
            text.split_whitespace().next().unwrap_or(""),
            "/help"
                | "/h"
                | "/clear"
                | "/sessions"
                | "/chats"
                | "/history"
                | "/conversations"
                | "/ls"
                | "/convs"
                | "/providers"
                | "/provider"
                | "/auth"
                | "/keys"
                | "/new"
                | "/resume"
                | "/r"
                | "/open"
                | "/restore"
                | "/fork"
                | "/rename"
                | "/archive"
                | "/delete"
                | "/export"
                | "/model"
                | "/mode"
                | "/modes"
                | "/skills"
                | "/skill"
                | "/skill-new"
                | "/new-skill"
                | "/skill-create"
                | "/theme"
                | "/settings"
                | "/setting"
                | "/config"
                | "/options"
                | "/preferences"
                | "/thinking"
                | "/permissions"
                | "/perms"
                | "/tasks"
                | "/task"
                | "/bg"
                | "/jobs"
                | "/agents"
                | "/agent"
                | "/errors"
                | "/error"
                | "/err"
                | "/verbosity"
                | "/verbose"
                | "/cards"
                | "/diff"
                | "/output"
                | "/view"
                | "/tool"
                | "/out"
                | "/undo"
                | "/rewind"
                | "/compact"
                | "/quit"
                | "/q"
                | "/exit"
        )
    }
}

#[cfg(test)]
// Env/global-registry tests serialize on process-global locks held across
// awaits by design; the deadlock risk the lint guards against does not
// apply to these test-only guards.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::app::testkit::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    #[test]
    fn compact_refuses_while_busy() {
        let mut app = test_app();
        app.busy = true;
        app.handle_slash("/compact");
        assert!(app.compact_rx.is_none(), "no task while busy");
        let last = app.messages.last().expect("notice");
        assert!(
            last.content.contains("busy"),
            "told to wait: {}",
            last.content
        );
    }

    #[tokio::test]
    async fn compact_poll_reports_not_needed_on_small_session() {
        let _env_guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let db = std::env::temp_dir().join(format!("vh_compact_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let prev = std::env::var("VIORAHARNESS_DB").ok();
        std::env::set_var("VIORAHARNESS_DB", &db);

        let mut app = test_app();
        app.handle_slash("/compact");
        assert!(app.compact_rx.is_some(), "task spawned");
        for _ in 0..200 {
            app.poll_compact();
            if app.compact_rx.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(app.compact_rx.is_none(), "task completed");
        let last = app.messages.last().expect("report");
        assert!(
            last.content.contains("not needed"),
            "planner note shown: {}",
            last.content
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_file(&db);
    }

    #[tokio::test]
    async fn task_completion_wakes_idle_chat() {
        use vioraharness_core::tools::tasks;
        let (db, prev, _env_guard) = with_temp_db("wake");
        let mut app = test_app();
        vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .create_session(&app.session_id, "m", None)
            .expect("session");
        let workdir = test_workdir();
        let t = tasks::spawn_task("echo wake-probe-xyz", &workdir);
        let t2 = tasks::spawn_task("echo wake-probe-second", &workdir);
        wait_task_done(&t.id).await;
        wait_task_done(&t2.id).await;
        let waked = app.poll_task_completions();
        for probe in [&t.id, &t2.id] {
            assert!(waked.contains(probe), "every completion tracked: {waked:?}");
        }
        assert!(app.busy, "follow-up turn started");
        // First completion starts the turn now; the second arrives while
        // busy, so it stays a persisted notice instead of queueing behind.
        assert!(
            app.queued_prompts.is_empty(),
            "no wake queued behind the live turn"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t.id)) && m.content.contains("done")),
            "notice names the finished task"
        );

        let rows = vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .get_messages_detailed(&app.session_id)
            .expect("rows");
        assert!(
            rows.iter()
                .any(|m| m.content.contains(&short_task_id(&t.id))),
            "notice row stored"
        );

        if let Some(h) = app.pending.take() {
            h.abort();
        }
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // _env_guard pins process-global VIORAHARNESS_DB
    async fn wake_turn_persists_prompt_as_system() {
        use vioraharness_core::tools::tasks;
        let (db, prev, _env_guard) = with_temp_db("wake-role");
        let mut app = test_app();
        vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .create_session(&app.session_id, "m", None)
            .expect("session");
        let workdir = test_workdir();
        let t = tasks::spawn_task("echo role-probe", &workdir);
        wait_task_done(&t.id).await;
        let _ = app.poll_task_completions();
        assert!(app.busy, "wake turn started");
        // The wake prompt row lands before the (doomed) provider call.
        let mut role = String::new();
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 10 {
            if let Ok(store) =
                vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            {
                if let Ok(rows) = store.get_messages(&app.session_id) {
                    if let Some(r) = rows
                        .iter()
                        .find(|m| m.2.contains("[background task finished]"))
                    {
                        role = r.1.clone();
                        break;
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(role, "system", "wake prompt never impersonates the user");
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // _env_guard pins process-global VIORAHARNESS_DB
    async fn user_turn_persists_prompt_as_user() {
        let (db, prev, _env_guard) = with_temp_db("user-role");
        let mut app = test_app();
        vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .create_session(&app.session_id, "m", None)
            .expect("session");
        app.start_turn("hello-role-probe".into(), None, "user");
        assert!(app.busy, "turn started");
        let mut role = String::new();
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < 10 {
            if let Ok(store) =
                vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            {
                if let Ok(rows) = store.get_messages(&app.session_id) {
                    if let Some(r) = rows.iter().find(|m| m.2.contains("hello-role-probe")) {
                        role = r.1.clone();
                        break;
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(role, "user", "typed prompt keeps user role");
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn task_completion_stays_quiet_when_busy_or_off() {
        use vioraharness_core::tools::tasks;
        let (db, prev, _env_guard) = with_temp_db("quiet");

        let mut app = test_app();
        let workdir = test_workdir();
        let t = tasks::spawn_task("echo quiet-probe-xyz", &workdir);
        wait_task_done(&t.id).await;
        app.busy = true;
        let waked = app.poll_task_completions();
        assert!(
            waked.contains(&t.id),
            "busy completion tracked for later: {waked:?}"
        );
        assert!(app.pending.is_none(), "no turn started while busy");
        assert!(
            !app.queued_prompts
                .iter()
                .any(|q| q.send.contains(&short_task_id(&t.id))),
            "no wake queued behind the live turn"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t.id))),
            "notice still posted"
        );

        let workdir = test_workdir();
        let t2 = tasks::spawn_task("echo quiet-probe-abc", &workdir);
        wait_task_done(&t2.id).await;
        app.busy = false;
        app.wake_on_tasks = false;
        let waked = app.poll_task_completions();
        assert!(waked.is_empty(), "no wake when disabled");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t2.id))),
            "notice still posted"
        );
        assert!(!app.busy, "stays idle");
        restore_db_env(prev, &db);
    }

    #[test]
    fn busy_safe_slash_lists() {
        for safe in [
            "/help",
            "/sessions",
            "/providers",
            "/model",
            "/skills",
            "/theme",
            "/settings",
            "/thinking",
            "/permissions",
            "/verbosity",
            "/diff",
            "/output",
            "/tasks",
            "/export",
            "/compact",
            "/quit",
        ] {
            assert!(App::is_busy_safe_slash(safe), "{safe} runs while busy");
        }
        for unsafe_ in [
            "/clear",
            "/new",
            "/resume",
            "/r",
            "/fork",
            "/rename",
            "/archive",
            "/delete",
            "/undo",
            "/rewind",
            "/skill-new",
        ] {
            assert!(
                !App::is_busy_safe_slash(unsafe_),
                "{unsafe_} waits for idle"
            );
        }
        assert!(
            !App::is_busy_safe_slash("/home/x/y.png"),
            "pasted path not slash"
        );
        assert!(!App::is_busy_safe_slash("hello"), "plain text not slash");
    }

    #[tokio::test]
    async fn typing_and_queue_work_while_busy() {
        let (db, prev, _env_guard) = with_temp_db("queue-echo");
        let mut app = test_app();
        app.busy = true;

        app.handle_key(KeyCode::Char('h')).await.unwrap();
        app.handle_key(KeyCode::Char('i')).await.unwrap();
        assert_eq!(app.input.text, "hi");

        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.input.text, "", "input cleared on queue");
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(app.queued_prompts[0].send, "hi");
        assert_eq!(app.queued_prompts[0].session_id, app.session_id);
        assert!(app.busy, "running turn untouched");
        assert!(
            !app.messages.iter().any(|m| m.role == "user"),
            "no echo until the queued turn starts"
        );

        app.input.text = "second".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 2);

        // Turn ends: first queued prompt echoes as its turn starts.
        app.busy = false;
        app.drain_queue();
        assert!(app.busy, "queued turn started");
        assert_eq!(app.queued_prompts.len(), 1, "one slot consumed");
        let users: Vec<_> = app
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.clone())
            .collect();
        assert_eq!(
            users,
            vec!["hi".to_string()],
            "echo at turn start: {users:?}"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn busy_enter_routes_slash_by_safety() {
        let mut app = test_app();
        app.busy = true;

        app.input.text = "/tasks".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.popup, Popup::Tasks, "/tasks opens while busy");
        app.popup = Popup::None;

        // Non-safe commands queue as deferred work instead of dropping.
        let sid = app.session_id.clone();
        app.input.text = "/new".into();
        app.input.cursor = 4;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.session_id, sid, "session untouched while busy");
        assert_eq!(app.input.text, "", "input cleared on queue");
        assert_eq!(app.queued_prompts.len(), 1);
        assert!(app.queued_prompts[0].slash, "marked deferred");
        assert_eq!(app.queued_prompts[0].send, "/new");
        assert!(
            app.status.contains("queued"),
            "queue notice: {}",
            app.status
        );

        // Once the turn finishes, the deferred command runs as if typed idle.
        app.busy = false;
        app.drain_queue();
        assert_ne!(app.session_id, sid, "deferred /new ran on drain");
        assert!(!app.busy, "no model turn started");
        assert!(
            app.messages.iter().any(|m| m.content.contains("New chat")),
            "command output shown"
        );
    }

    #[tokio::test]
    async fn queue_drains_on_idle_and_drops_on_mismatch() {
        let (db, prev, _env_guard) = with_temp_db("queue");
        let mut app = test_app();

        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "drained-next".into(),
            image: None,
            slash: false,
            echo: true,
        });
        app.drain_queue();
        assert!(app.busy, "turn started");
        assert!(app.queued_prompts.is_empty(), "slot consumed");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "drained-next"),
            "echoed at turn start"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;

        app.queued_prompts.push(QueuedPrompt {
            session_id: "other-session".into(),
            send: "stale".into(),
            image: None,
            slash: false,
            echo: false,
        });
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "fresh".into(),
            image: None,
            slash: false,
            echo: true,
        });
        app.drain_queue();
        assert!(app.busy, "fresh slot fired after dropping stale");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "fresh"),
            "survivor echoed at turn start"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("session changed")),
            "drop notice shown"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn drain_runs_deferred_slash_in_order_after_prompt() {
        let (db, prev, _env_guard) = with_temp_db("drain-order");
        let mut app = test_app();
        app.busy = true;

        // Matrix typed while busy: prompt first, command second.
        app.input.text = "first".into();
        app.input.cursor = 5;
        app.handle_key(KeyCode::Enter).await.unwrap();
        app.input.text = "/clear".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 2, "both queued");
        assert!(!app.queued_prompts[0].slash);
        assert!(app.queued_prompts[1].slash, "command deferred second");
        assert!(
            !app.messages.iter().any(|m| m.role == "user"),
            "nothing echoed while busy"
        );

        // Turn ends: the prompt echoes and fires first, the command waits.
        app.busy = false;
        app.drain_queue();
        assert!(app.busy, "prompt turn started");
        assert_eq!(app.queued_prompts.len(), 1, "slash still waiting");
        assert!(app.queued_prompts[0].slash);
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "first"),
            "prompt echoed at turn start"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;

        // Prompt turn ends: the deferred command runs, nothing injected.
        app.drain_queue();
        assert!(!app.busy, "slash starts no model turn");
        assert!(app.queued_prompts.is_empty(), "queue fully drained");
        assert!(
            app.messages.iter().any(|m| m.content == "cleared"),
            "deferred /clear ran last"
        );
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn esc_cancel_drops_queue() {
        let mut app = test_app();
        app.busy = true;
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "doomed".into(),
            image: None,
            slash: false,
            echo: true,
        });
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.busy, "first Esc only arms the cancel");
        assert!(app.cancel_armed());
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.busy, "second Esc cancels the turn");
        assert!(app.queued_prompts.is_empty(), "queue dropped");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("dropped 1 queued")),
            "drop notice shown"
        );
    }

    #[test]
    fn busy_input_renders_typeahead_hint() {
        let mut app = test_app();
        app.busy = true;
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Enter queues"), "title advertises queueing");
        assert!(text.contains("Type ahead"), "placeholder invites typing");
    }

    #[test]
    fn footer_shows_task_and_queue_counts() {
        let mut app = test_app();
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "later".into(),
            image: None,
            slash: false,
            echo: true,
        });
        let text = render_text(&mut app, 100, 30);
        let footer = text.lines().last().unwrap_or("").to_string();
        assert!(footer.contains("1 queued"), "queued count: {footer:?}");
    }

    #[tokio::test]
    async fn footer_shows_running_tasks() {
        use vioraharness_core::tools::tasks;
        let workdir = test_workdir();
        let live = tasks::spawn_task("sleep 30", &workdir);
        let mut app = test_app();
        let text = render_text(&mut app, 120, 30);
        let footer = text.lines().last().unwrap_or("").to_string();
        assert!(
            footer.contains("running") && footer.contains("task"),
            "running count: {footer:?}"
        );
        assert!(footer.contains("/tasks"), "panel hint: {footer:?}");
        assert!(tasks::kill_task(&live.id), "cleanup");
    }

    #[test]
    fn tasks_panel_renders_title_with_or_without_tasks() {
        let mut app = test_app();
        app.handle_slash("/tasks");
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Background Tasks"), "panel title");
        assert!(
            text.contains("No background tasks") || text.contains("STATUS"),
            "empty state or column list"
        );
    }

    #[tokio::test]
    async fn tasks_panel_lists_and_kills() {
        use vioraharness_core::tools::tasks;

        let workdir = test_workdir();
        let t = tasks::spawn_task("echo panel-probe-xyz", &workdir);
        let start = std::time::Instant::now();
        loop {
            if let Some(cur) = tasks::get_task(&t.id) {
                if cur.status != tasks::BgStatus::Running {
                    break;
                }
            }
            assert!(start.elapsed().as_secs() < 10, "task finished");
            tokio::task::yield_now().await;
        }
        let mut app = test_app();
        app.handle_slash("/tasks");

        let text = render_text(&mut app, 160, 40);
        assert!(text.contains("Background Tasks"), "panel title");
        assert!(text.contains(&short_task_id(&t.id)), "task id shown");
        assert!(text.contains("panel-probe-xyz"), "command shown");

        let narrow = render_text(&mut app, 100, 30);
        for (i, line) in narrow.lines().enumerate() {
            assert!(line.chars().count() <= 100, "row {i} fits: {line:?}");
        }

        let workdir = test_workdir();
        let live = tasks::spawn_task("sleep 30", &workdir);
        let mut app = test_app();
        app.handle_slash("/tasks");

        app.task_cursor = vioraharness_core::tools::tasks::list_tasks()
            .iter()
            .position(|t| t.id == live.id)
            .expect("own task listed");

        app.handle_popup_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()));
        assert_eq!(
            vioraharness_core::tools::tasks::get_task(&live.id)
                .expect("still tracked")
                .status,
            vioraharness_core::tools::tasks::BgStatus::Killed,
            "panel K kills the highlighted task"
        );
    }

    #[test]
    fn slash_routing_sends_unknown_slash_as_prompt() {
        for known in [
            "/help",
            "/h",
            "/clear",
            "/sessions",
            "/ls",
            "/providers",
            "/new",
            "/resume",
            "/r",
            "/fork",
            "/rename",
            "/archive",
            "/delete",
            "/export",
            "/model",
            "/mode",
            "/skills",
            "/skill-new",
            "/theme",
            "/settings",
            "/thinking",
            "/permissions",
            "/perms",
            "/verbosity",
            "/diff",
            "/output",
            "/view",
            "/undo",
            "/rewind",
            "/compact",
            "/quit",
            "/q",
            "/exit",
        ] {
            assert!(App::is_known_slash(known), "{known} routes to dispatcher");
            assert!(
                App::is_known_slash(&format!("{known} some args")),
                "{known} with args routes too"
            );
        }
        for prompt in [
            "/home/jnd/Pictures/shot.png",
            "/usr/bin/python3 --version",
            "/",
            "/viewx",
            "/undo2",
            "run /tmp/x.cir",
            "",
        ] {
            assert!(!App::is_known_slash(prompt), "{prompt:?} sends as prompt");
        }
    }

    #[test]
    fn mode_command_switches_lists_and_rejects() {
        let mut app = test_app();
        app.agent_mode = "eda".to_string();
        app.handle_slash("/mode web");
        assert_eq!(app.agent_mode, "web");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("mode → web")),
            "switch confirmed in chat"
        );
        app.handle_slash("/mode web");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("already in web")),
            "idempotent re-select"
        );
        app.handle_slash("/mode nope");
        assert_eq!(app.agent_mode, "web", "unknown mode rejected");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("unknown mode")),
            "rejection explained"
        );
        app.handle_slash("/mode");
        assert_eq!(
            app.popup,
            crate::app::Popup::ModePicker,
            "bare /mode opens picker dialog"
        );
        // Picker starts on the current mode; Enter keeps it (idempotent).
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.popup, crate::app::Popup::None, "picker closes on Enter");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("already in web")),
            "picker Enter on current mode is idempotent"
        );
        assert!(
            App::is_busy_safe_slash("/mode web"),
            "/mode runs while busy"
        );
    }

    #[test]
    fn mode_picker_navigates_and_switches() {
        let mut app = test_app();
        app.agent_mode = "eda".to_string();
        app.handle_slash("/mode");
        assert_eq!(app.popup, Popup::ModePicker, "bare /mode opens picker");
        assert_eq!(app.mode_cursor, 0, "cursor starts on current mode");
        app.handle_popup_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(app.mode_cursor, 1, "Down moves to web");
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.popup, Popup::None, "picker closes after switch");
        assert_eq!(app.agent_mode, "web", "picker Enter switches mode");
        // Esc closes without switching.
        app.handle_slash("/mode");
        assert_eq!(app.popup, Popup::ModePicker);
        app.handle_popup_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert_eq!(app.popup, Popup::None);
        assert_eq!(app.agent_mode, "web", "Esc keeps current mode");
    }

    #[test]
    fn mode_switch_persists_to_session_row() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env_guard) = with_temp_db("mode-persist");
        let mut app = test_app();
        app.agent_mode = "eda".to_string();
        SessionStore::new(db.to_str().unwrap())
            .unwrap()
            .create_session(&app.session_id, "m", None)
            .unwrap();
        app.handle_slash("/mode web");
        assert_eq!(app.agent_mode, "web");
        let stored = SessionStore::new(db.to_str().unwrap())
            .unwrap()
            .get_session(&app.session_id)
            .unwrap()
            .expect("session row");
        assert_eq!(
            stored.mode.as_deref(),
            Some("web"),
            "per-chat mode written on switch, not on next turn"
        );
        restore_db_env(prev, &db);
    }

    #[test]
    fn startup_prefers_last_opened_chat_mode() {
        use vioraharness_core::session::SessionStore;
        let (db, prev_db, _env_guard) = with_temp_db("mode-startup");
        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let xdg = std::env::temp_dir().join(format!("vh_xdg_mode_{}", std::process::id()));
        let _ = std::fs::create_dir_all(xdg.join("vioraharness"));
        std::env::set_var("XDG_DATA_HOME", &xdg);
        SessionStore::new(db.to_str().unwrap())
            .unwrap()
            .create_session("chat-web-1", "m", None)
            .unwrap();
        SessionStore::new(db.to_str().unwrap())
            .unwrap()
            .set_session_mode("chat-web-1", "web")
            .unwrap();
        std::fs::write(xdg.join("vioraharness/last_session"), "chat-web-1").unwrap();
        assert_eq!(
            App::last_opened_chat_mode().as_deref(),
            Some("web"),
            "last opened chat's mode wins over the global default"
        );
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&xdg);
        restore_db_env(prev_db, &db);
    }

    fn rewind_test_session(
        tag: &str,
    ) -> (
        std::path::PathBuf,
        Option<String>,
        std::sync::MutexGuard<'static, ()>,
    ) {
        use vioraharness_core::session::SessionStore;
        let (db, prev, guard) = with_temp_db(tag);
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        store.create_session("sess-rewind", "m", None).unwrap();
        store
            .append_message("sess-rewind", "user", "first message here")
            .unwrap();
        store
            .append_message("sess-rewind", "assistant", "second reply here")
            .unwrap();
        store
            .append_message("sess-rewind", "user", "third message here")
            .unwrap();
        (db, prev, guard)
    }

    #[test]
    fn rewind_opens_dialog_at_latest_checkpoint() {
        let (db, prev, _env) = rewind_test_session("rewind-open");
        let mut app = test_app();
        app.session_id = "sess-rewind".into();
        app.handle_slash("/rewind");
        assert_eq!(app.popup, Popup::Rewind);
        assert_eq!(
            app.rewind_cursor, 1,
            "starts at latest checkpoint (2 turns)"
        );
        assert_eq!(app.rewind_armed, None);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Rewind"), "dialog title");
        assert!(text.contains("first message here"), "checkpoints listed");
        assert!(text.contains("third message here"), "latest listed");
        let rows: Vec<&str> = text.lines().collect();
        let hint_at = rows
            .iter()
            .position(|l| l.contains("Cancel"))
            .unwrap_or(usize::MAX);
        let bottom = rows.get(hint_at).copied().unwrap_or("");
        assert!(
            bottom.contains("Navigate") && bottom.contains("Select"),
            "hint bar shows Navigate/Select/Cancel: {bottom:?}"
        );
        let last_checkpoint = rows
            .iter()
            .rposition(|l| l.contains("third message here"))
            .unwrap_or(0);
        let explainer = rows
            .iter()
            .position(|l| l.contains("later messages are dropped"));
        assert!(
            hint_at > last_checkpoint && explainer.map(|e| hint_at > e).unwrap_or(true),
            "hint bar sits below checkpoints and explainer (bottom of dialog)"
        );
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_empty_session_shows_notice() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env) = with_temp_db("rewind-empty");
        SessionStore::new(db.to_str().unwrap())
            .unwrap()
            .create_session("sess-empty", "m", None)
            .unwrap();
        let mut app = test_app();
        app.session_id = "sess-empty".into();
        app.handle_slash("/rewind");
        assert_eq!(app.popup, Popup::Rewind);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("nothing to rewind"), "empty notice");
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_refuses_while_busy() {
        let mut app = test_app();
        app.busy = true;
        app.handle_slash("/rewind");
        assert_eq!(app.popup, Popup::None);
        assert!(
            app.messages.iter().any(|m| m.content.contains("busy")),
            "busy notice pushed"
        );
    }

    #[test]
    fn rewind_arm_confirm_restores_and_truncates() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env) = rewind_test_session("rewind-flow");
        let dir = std::env::temp_dir().join("vh_rewind_tui_flow");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("deck.cir").to_string_lossy().to_string();
        {
            let store = SessionStore::new(db.to_str().unwrap()).unwrap();
            store
                .insert_snapshot("sess-rewind", 1, &f, "s", b"checkpoint-state")
                .unwrap();
        }
        std::fs::write(&f, b"later-state").unwrap();

        let mut app = test_app();
        app.session_id = "sess-rewind".into();
        app.handle_slash("/rewind");
        // Move to first checkpoint, arm, nav away disarms, re-arm, confirm.
        app.handle_popup_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        assert_eq!(app.rewind_cursor, 0);
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.rewind_armed, Some(2), "armed at turn-1 target seq");
        let armed_text = render_text(&mut app, 100, 30);
        assert!(armed_text.contains("ARMED"), "armed banner shown");
        assert!(
            armed_text.contains("drop 1 message"),
            "banner previews consequences: {armed_text:?}"
        );
        app.handle_popup_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(app.rewind_armed, None, "navigation disarms");
        app.handle_popup_key(KeyEvent::new(KeyCode::Up, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.popup, Popup::None, "dialog closes after restore");

        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(store.get_messages("sess-rewind").unwrap().len(), 2);
        assert_eq!(std::fs::read(&f).unwrap(), b"checkpoint-state");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("rewound to checkpoint #1")),
            "report message shown"
        );
        let _ = std::fs::remove_dir_all(&dir);
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_esc_cancels_without_changes() {
        let (db, prev, _env) = rewind_test_session("rewind-esc");
        let mut app = test_app();
        app.session_id = "sess-rewind".into();
        app.handle_slash("/rewind");
        app.handle_popup_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(app.rewind_armed.is_some(), "non-latest checkpoint arms");
        app.handle_popup_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()));
        assert_eq!(app.popup, Popup::None);
        assert_eq!(app.rewind_armed, None);
        use vioraharness_core::session::SessionStore;
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(
            store.get_messages("sess-rewind").unwrap().len(),
            3,
            "untouched"
        );
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_latest_checkpoint_refuses_as_noop() {
        let (db, prev, _guard) = rewind_test_session("rewind-noop");
        let mut app = test_app();
        app.session_id = "sess-rewind".into();
        app.handle_slash("/rewind");
        // Cursor starts at the latest checkpoint: arming must refuse.
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.rewind_armed, None, "latest checkpoint never arms");
        assert!(
            app.status.contains("already latest"),
            "refusal explained: {}",
            app.status
        );
        assert_eq!(app.popup, Popup::Rewind, "dialog stays open");
        use vioraharness_core::session::SessionStore;
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(
            store.get_messages("sess-rewind").unwrap().len(),
            3,
            "untouched"
        );
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_groups_one_user_turn_into_one_checkpoint() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env) = with_temp_db("rewind-group");
        {
            let store = SessionStore::new(db.to_str().unwrap()).unwrap();
            store.create_session("sess-group", "m", None).unwrap();
            // One user message followed by 9 model rows (assistant + tools).
            store
                .append_message("sess-group", "user", "only prompt")
                .unwrap();
            for i in 0..5 {
                store
                    .append_message("sess-group", "assistant", &format!("step {i}"))
                    .unwrap();
                let seq = store
                    .append_message("sess-group", "tool", &format!("result {i}"))
                    .unwrap();
                store
                    .record_tool_call(
                        &format!("g{i}"),
                        "sess-group",
                        seq,
                        "read",
                        &serde_json::json!({}),
                    )
                    .unwrap();
            }
            store
                .insert_snapshot("sess-group", 4, "/tmp/g.txt", "s", b"x")
                .unwrap();
            store
                .insert_snapshot("sess-group", 9, "/tmp/g.txt", "s", b"y")
                .unwrap();
        }
        let mut app = test_app();
        app.session_id = "sess-group".into();
        app.handle_slash("/rewind");
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("1 checkpoint"), "single turn, single row");
        assert!(!text.contains("step 3"), "model rows hidden");
        assert!(text.contains("◆2"), "both snapshots counted in the turn");
        // Rewinding to the only checkpoint drops nothing.
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(store.get_messages("sess-group").unwrap().len(), 11);
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_skips_compaction_summaries_as_turns() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env) = with_temp_db("rewind-compact");
        {
            let store = SessionStore::new(db.to_str().unwrap()).unwrap();
            store.create_session("sess-compact", "m", None).unwrap();
            store
                .append_message("sess-compact", "user", "old q")
                .unwrap();
            store
                .append_message("sess-compact", "assistant", "old a")
                .unwrap();
            store
                .compact_replace("sess-compact", 2, "summary of old")
                .unwrap();
            store
                .append_message("sess-compact", "user", "new q")
                .unwrap();
        }
        let mut app = test_app();
        app.session_id = "sess-compact".into();
        app.handle_slash("/rewind");
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("2 checkpoints"), "history stub + new turn");
        assert!(text.contains("new q"), "real user turn listed");
        assert!(
            !text.contains("summary of old"),
            "compaction summary is not a checkpoint"
        );
        restore_db_env(prev, &db);
    }

    #[test]
    fn rewind_badges_continue_and_wiring() {
        use vioraharness_core::session::SessionStore;
        let (db, prev, _env) = rewind_test_session("rewind-badges");
        let dir = std::env::temp_dir().join("vh_rewind_tui_badges");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.cir").to_string_lossy().to_string();
        {
            let store = SessionStore::new(db.to_str().unwrap()).unwrap();
            store
                .insert_snapshot("sess-rewind", 2, &f, "s", b"v")
                .unwrap();
            store
                .record_tool_call("cbadge", "sess-rewind", 3, "write", &serde_json::json!({}))
                .unwrap();
        }
        let mut app = test_app();
        app.session_id = "sess-rewind".into();
        app.handle_slash("/rewind");
        let text = render_text(&mut app, 120, 30);
        assert!(text.contains("◆1"), "snapshot badge on message #2");

        // Rewind to checkpoint 0 (turn 1, target #2), then continue: seq continues.
        app.handle_popup_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        let store = SessionStore::new(db.to_str().unwrap()).unwrap();
        assert_eq!(store.get_messages("sess-rewind").unwrap().len(), 2);
        let next = store
            .append_message("sess-rewind", "user", "continued")
            .unwrap();
        assert_eq!(next, 3, "seq continues after rewind, no clash");
        // Tool call at the dropped message is gone after reload.
        app.reload_display_from_store(&store, "sess-rewind")
            .unwrap_or(0);
        assert!(
            !store
                .get_tool_calls_grouped_simple("sess-rewind")
                .unwrap()
                .contains_key(&3),
            "dropped tool call purged"
        );
        assert!(
            !app.messages
                .iter()
                .any(|m| m.content.contains("third message")),
            "dropped message gone after reload"
        );

        // Wiring: completions offer /rewind, help documents it.
        app.input.text = "/rew".into();
        let completions = app.input.slash_completions();
        assert!(
            completions.iter().any(|(c, _)| *c == "/rewind"),
            "tab-completes /rewind"
        );
        app.popup = Popup::Help;
        let help = render_text(&mut app, 120, 100);
        assert!(help.contains("/rewind"), "help documents /rewind");
        assert!(
            help.contains("Shift+"),
            "help documents input selection keys"
        );
        let _ = std::fs::remove_dir_all(&dir);
        restore_db_env(prev, &db);
    }

    fn live_turn_app() -> App {
        // Simulate a running turn: busy with a live injector, no real task.
        let mut app = test_app();
        app.busy = true;
        let inj = vioraharness_core::loop_mod::Injector::default();
        app.turn_session = Some(app.session_id.clone());
        app.instant_injector = Some(inj);
        app
    }

    #[tokio::test]
    async fn instant_prompt_echoes_and_injects_while_busy() {
        let mut app = live_turn_app();
        app.input.text = "$stop that".into();
        app.input.cursor = 10;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.input.text, "", "input cleared");
        assert!(app.queued_prompts.is_empty(), "queue bypassed");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "stop that"),
            "echoed immediately, sigil stripped"
        );
        assert!(
            !app.messages.iter().any(|m| m.content.contains('$')),
            "no $ leaks into chat"
        );
        let inj = app.instant_injector.as_ref().expect("turn still live");
        assert_eq!(inj.drain(), vec!["stop that".to_string()]);
        assert!(app.status.contains('⚡'), "status: {}", app.status);
    }

    #[tokio::test]
    async fn instant_falls_back_to_queue_without_live_turn() {
        let mut app = test_app();
        app.busy = true; // busy, but the turn handle is already gone
        app.input.text = "$later please".into();
        app.input.cursor = 13;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(app.queued_prompts[0].send, "later please");
        assert!(app.queued_prompts[0].echo, "normal echo at turn start");
        assert!(
            !app.messages.iter().any(|m| m.role == "user"),
            "no immediate echo on fallback"
        );
    }

    #[tokio::test]
    async fn instant_session_mismatch_queues() {
        let mut app = live_turn_app();
        app.turn_session = Some("other-session".into());
        app.input.text = "$not yours".into();
        app.input.cursor = 10;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 1, "queued, not injected");
        assert_eq!(app.queued_prompts[0].send, "not yours");
        assert!(
            app.instant_injector
                .as_ref()
                .map(|i| i.is_empty())
                .unwrap_or(false),
            "live turn untouched"
        );
    }

    #[tokio::test]
    async fn instant_slash_stays_literal() {
        let mut app = live_turn_app();
        app.input.text = "$/model x".into();
        app.input.cursor = 9;
        app.handle_key(KeyCode::Enter).await.unwrap();
        let inj = app.instant_injector.as_ref().expect("turn still live");
        assert_eq!(inj.drain(), vec!["/model x".to_string()]);
        assert_eq!(app.popup, Popup::None, "no slash dispatched");
    }

    #[tokio::test]
    async fn instant_empty_ignored() {
        let mut app = live_turn_app();
        app.input.text = "$  ".into();
        app.input.cursor = 3;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert!(app.queued_prompts.is_empty());
        assert!(app
            .instant_injector
            .as_ref()
            .map(|i| i.is_empty())
            .unwrap_or(false));
        assert!(app.status.contains("ignored"), "status: {}", app.status);
    }

    #[tokio::test]
    async fn instant_idle_submits_normally() {
        let (db, prev, _env_guard) = with_temp_db("instant-idle");
        let mut app = test_app();
        app.input.text = "$go fast".into();
        app.input.cursor = 8;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert!(app.busy, "turn started");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "go fast"),
            "stripped and echoed"
        );
        assert!(app.instant_injector.is_some(), "live handle stashed");
        assert_eq!(app.turn_session.as_deref(), Some(app.session_id.as_str()));
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;
        restore_db_env(prev, &db);
    }

    #[test]
    fn promote_instant_leftovers_heads_queue_without_echo() {
        let mut app = live_turn_app();
        let inj = app.instant_injector.as_ref().unwrap().clone();
        inj.push("i1".into());
        inj.push("i2".into());
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "normal".into(),
            image: None,
            slash: false,
            echo: true,
        });
        let n = app.promote_instant_leftovers();
        assert_eq!(n, 2);
        assert!(app.instant_injector.is_none(), "handle released");
        assert!(app.turn_session.is_none());
        let sends: Vec<_> = app.queued_prompts.iter().map(|q| q.send.as_str()).collect();
        assert_eq!(sends, vec!["i1", "i2", "normal"], "oldest first: {sends:?}");
        assert!(
            !app.queued_prompts[0].echo && !app.queued_prompts[1].echo,
            "no re-echo for already-displayed prompts"
        );
        assert!(app.queued_prompts[2].echo, "normal keeps its echo");
    }

    #[tokio::test]
    async fn esc_cancel_promotes_instant_leftovers() {
        let mut app = live_turn_app();
        app.instant_injector.as_ref().unwrap().push("urgent".into());
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "doomed".into(),
            image: None,
            slash: false,
            echo: true,
        });
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.busy, "first Esc only arms the cancel");
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.busy, "second Esc cancels the turn");
        assert_eq!(app.queued_prompts.len(), 1, "normal dropped, ⚡ kept");
        assert_eq!(app.queued_prompts[0].send, "urgent");
        assert!(!app.queued_prompts[0].echo, "already echoed");
        assert!(
            app.messages.iter().any(|m| m.content.contains("kept 1 ⚡")),
            "kept notice shown"
        );
    }

    #[test]
    fn footer_shows_instant_count() {
        let mut app = live_turn_app();
        app.instant_injector.as_ref().unwrap().push("a".into());
        app.instant_injector.as_ref().unwrap().push("b".into());
        let text = render_text(&mut app, 100, 30);
        let footer = text.lines().last().unwrap_or("").to_string();
        assert!(footer.contains("»2 instant"), "instant count: {footer:?}");
    }
}
