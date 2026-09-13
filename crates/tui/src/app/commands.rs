use super::popups::rewind_checkpoints;
use super::*;

impl App {
    pub(crate) fn handle_slash(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first().copied().unwrap_or("") {
            "/help" | "/h" => self.popup = Popup::Help,
            "/clear" => {
                self.messages.clear();

                let dropped = self.queued_prompts.len();
                self.queued_prompts.clear();
                self.messages.push(Msg::new(
                    "system",
                    if dropped > 0 {
                        format!(
                            "cleared (dropped {dropped} queued prompt{})",
                            if dropped == 1 { "" } else { "s" }
                        )
                    } else {
                        "cleared".to_string()
                    },
                ));
                self.scroll = 0;
            }
            "/sessions" | "/chats" | "/history" | "/conversations" | "/ls" | "/convs" => {
                self.session_filter.clear();
                self.session_cursor = 0;
                self.show_all_sessions = false;
                self.popup = Popup::Sessions;
            }
            "/providers" | "/provider" | "/auth" | "/keys" => {
                self.provider_cursor = 0;
                self.provider_key_input.clear();
                self.provider_input_active = false;
                self.provider_selected = None;
                self.provider_msg = None;
                self.popup = Popup::Providers;
            }
            "/model" => {
                if parts.len() > 1 {
                    let m = parts[1..].join(" ");
                    self.model = m.clone();
                    Self::save_tui_state(serde_json::json!({"last_model": self.model}));
                    self.messages
                        .push(Msg::new("system", format!("model → {m} (saved)")));
                } else {
                    self.model_filter.clear();
                    self.model_cursor = 0;
                    self.popup = Popup::ModelPicker;
                }
            }
            "/skills" | "/skill" => {
                self.popup = Popup::Skills;
            }
            "/skill-new" | "/new-skill" | "/skill-create" => {
                if self.busy {
                    self.messages.push(Msg::new(
                        "system",
                        "busy — wait for the current turn or Esc to cancel",
                    ));
                    return;
                }
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                match skill_new_request(&parts[1..], &cwd) {
                    Ok((description, target)) => {
                        let prompt = skill_creator_prompt(&description, &target);
                        self.input
                            .push_history(format!("/skill-new {}", parts[1..].join(" ")));
                        self.input.text.clear();
                        self.input.cursor = 0;
                        if let Err(e) = self.submit_text(prompt) {
                            self.messages.push(Msg::new(
                                "system",
                                format!("skill creator failed to start: {e:#}"),
                            ));
                        }
                    }
                    Err(usage) => {
                        self.messages.push(Msg::new("system", usage));
                    }
                }
            }
            "/theme" => {
                if parts.len() > 1 {
                    let name = parts[1].to_lowercase();

                    if self.available_themes.iter().any(|t| t == &name) {
                        Self::save_tui_state(serde_json::json!({"last_theme": name}));
                        self.messages.push(Msg::new(
                            "system",
                            format!("Theme → {} (saved, restart TUI to apply palette)", name),
                        ));
                    } else {
                        self.popup = Popup::ThemePicker;
                    }
                } else {
                    self.popup = Popup::ThemePicker;
                }
            }
            "/thinking" => {
                if parts.len() > 1 {
                    match parts[1].to_lowercase().as_str() {
                        "on" | "true" | "show" => {
                            self.show_thinking = true;
                            self.thinking_expanded = true;
                            Self::save_tui_state(serde_json::json!({"show_thinking": true}));
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Thinking ON ({} / {}) — saved",
                                    self.thinking_title, self.thinking_label
                                ),
                            ));
                        }
                        "off" | "false" | "hide" => {
                            self.show_thinking = false;
                            Self::save_tui_state(serde_json::json!({"show_thinking": false}));
                            self.messages
                                .push(Msg::new("system", "Thinking OFF — saved"));
                        }
                        _ => {
                            self.show_thinking = !self.show_thinking;
                            Self::save_tui_state(
                                serde_json::json!({"show_thinking": self.show_thinking}),
                            );
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Thinking {} — saved",
                                    if self.show_thinking { "ON" } else { "OFF" }
                                ),
                            ));
                        }
                    }
                } else {
                    self.show_thinking = !self.show_thinking;
                    self.thinking_expanded = self.show_thinking;
                    Self::save_tui_state(serde_json::json!({"show_thinking": self.show_thinking}));
                    self.messages.push(Msg::new(
                        "system",
                        format!(
                            "Thinking {} ({} / {}) — Ctrl+G to toggle expand — saved",
                            if self.show_thinking { "ON" } else { "OFF" },
                            self.thinking_title,
                            self.thinking_label
                        ),
                    ));
                }
            }
            "/permissions" | "/perms" => self.popup = Popup::Permissions,
            "/tasks" | "/task" | "/bg" | "/jobs" => {
                self.task_cursor = 0;
                self.popup = Popup::Tasks;
            }
            "/errors" | "/error" | "/err" => {
                self.error_cursor = 0;
                self.popup = Popup::Errors;
            }
            "/verbosity" | "/verbose" | "/cards" => {
                self.cmd_verbosity(&parts[1..]);
            }
            "/diff" => {
                self.popup = Popup::Diff;
            }
            "/output" | "/view" | "/tool" | "/out" => {
                if self.last_tool_output.is_some() {
                    self.popup = Popup::ToolOutput;
                    self.tool_output_scroll = 0;
                } else {
                    self.messages.push(Msg::new("system", "No tool output yet — run a bash/write/glob first, then /output or F9 to view"));
                }
            }
            "/undo" => {
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                let msg: String = (|| -> Result<String, String> {
                    let store = vioraharness_core::session::SessionStore::new(&db)
                        .map_err(|e| format!("undo: cannot open session db: {e}"))?;

                    match vioraharness_core::session::snapshot::restore_latest(
                        &store,
                        &self.session_id,
                    ) {
                        Ok(Some(path)) => Ok(format!(
                            "undo: restored {path} — repeat /undo to go further back"
                        )),
                        Ok(None) => Ok("undo: nothing to undo in this chat".to_string()),
                        Err(e) => Err(format!("undo failed: {e}")),
                    }
                })()
                .unwrap_or_else(|e| e);
                if msg.starts_with("undo failed") {
                    self.report_error("undo", msg);
                } else {
                    self.messages.push(Msg::new("system", msg));
                }
            }
            "/rewind" => {
                if self.busy {
                    self.messages.push(Msg::new(
                        "system",
                        "busy — rewind after the current turn finishes",
                    ));
                } else {
                    self.rewind_cursor = 0;
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    // Start at the latest message; ↑ walks back through checkpoints.
                    let len = rewind_checkpoints(&self.session_id).len();
                    if len > 0 {
                        self.rewind_cursor = len - 1;
                    }
                    self.popup = Popup::Rewind;
                }
            }
            "/compact" => {
                if self.busy {
                    self.messages.push(Msg::new(
                        "system",
                        "busy — /compact after the current turn finishes",
                    ));
                } else if self.compact_rx.is_some() {
                    self.messages
                        .push(Msg::new("system", "compaction already running…"));
                } else {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    let sid = self.session_id.clone();
                    let model = self.model.clone();
                    self.status = "Compacting context… (one summary call)".into();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    self.compact_rx = Some((sid.clone(), rx));
                    tokio::spawn(async move {
                        let out = async {
                            let store = vioraharness_core::session::SessionStore::new(&db)
                                .map_err(|e| format!("db: {e:#}"))?;
                            let provider = vioraharness_core::provider_for_model(&model);
                            let keep = vioraharness_core::context::compaction::compaction_config()
                                .keep_tail;
                            vioraharness_core::context::compaction::compact_session(
                                &store,
                                provider.as_ref(),
                                &sid,
                                &model,
                                keep,
                            )
                            .await
                            .map_err(|e| format!("{e:#}"))
                        }
                        .await;
                        let _ = tx.send(out);
                    });
                }
            }
            "/quit" | "/q" | "/exit" => self.should_quit = true,
            "/new" => {
                let new_id = new_session_id();
                self.session_id = new_id.clone();
                self.messages.clear();
                self.ctx_freed_tokens = 0;
                self.messages.push(Msg::new(
                    "system",
                    format!(
                        "New chat {new_id} — old chats kept in SQLite, use /sessions to resume"
                    ),
                ));
                self.scroll = 0;
                self.status = "ready".into();
            }
            "/resume" | "/r" | "/open" | "/restore" => {
                if parts.len() > 1 {
                    let id = parts[1].to_string();
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        let real_id = if let Ok(Some(_)) = store.get_session(&id) {
                            id.clone()
                        } else {
                            if let Ok(list) =
                                store.list_sessions_filtered(None, Some(&id), true, 10, 0)
                            {
                                list.into_iter()
                                    .find(|s| s.id.starts_with(&id))
                                    .map(|s| s.id)
                                    .unwrap_or(id.clone())
                            } else {
                                id.clone()
                            }
                        };
                        if let Ok(Some(sess)) = store.get_session(&real_id) {
                            self.session_id = real_id.clone();
                            self.model = sess.model.clone();
                            if let Some(theme) = sess.theme.clone() {
                                Self::save_tui_state(serde_json::json!({"last_theme": theme}));
                            }
                            match self.reload_display_from_store(&store, &real_id) {
                                Some(n) => {
                                    self.ctx_freed_tokens = 0;
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!(
                                            "↩︎ Resumed {} ({} msgs)",
                                            &real_id[..8.min(real_id.len())],
                                            n
                                        ),
                                    ));
                                }
                                None => {
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!("resume: no messages for {real_id}"),
                                    ));
                                }
                            }
                        } else {
                            self.messages.push(Msg::new(
                                "system",
                                format!("resume: session {real_id} not found"),
                            ));
                        }
                    } else {
                        self.report_error("resume", "resume: db error".to_string());
                    }
                } else {
                    self.session_cursor = 0;
                    self.show_all_sessions = false;
                    self.popup = Popup::Sessions;
                }
            }
            "/fork" => {
                let at_seq = parts.get(1).and_then(|s| s.parse::<i64>().ok());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    let parent = self.session_id.clone();
                    let new_id = new_session_id();
                    match store.fork_session(&parent, &new_id, at_seq) {
                        Ok(_) => {
                            self.session_id = new_id.clone();
                            self.ctx_freed_tokens = 0;
                            if let Ok(msgs) = store.get_messages_detailed(&new_id) {
                                self.messages.clear();
                                let tool_map =
                                    store.get_tool_calls_grouped(&new_id).unwrap_or_default();
                                for m in msgs {
                                    if m.role == "tool" {
                                        continue;
                                    }
                                    let ts = m.timestamp.clone().unwrap_or_else(|| {
                                        format!(
                                            "{:02}:{:02}",
                                            (m.created_at % 86400 / 3600) % 24,
                                            (m.created_at % 3600) / 60
                                        )
                                    });
                                    if m.role == "assistant" {
                                        if let Some(calls) = tool_map.get(&m.seq) {
                                            let mut items = Vec::new();
                                            for (id, name, args, _sig, result) in calls {
                                                items.push(Content::ToolCall {
                                                    id: id.clone(),
                                                    name: name.clone(),
                                                    args: args.clone(),
                                                    status: ToolStatus::Done,
                                                });
                                                if let Some(res_str) = result {
                                                    let ok = serde_json::from_str::<
                                                        serde_json::Value,
                                                    >(
                                                        res_str
                                                    )
                                                    .ok()
                                                    .and_then(|v| {
                                                        v.get("ok").and_then(|x| x.as_bool())
                                                    })
                                                    .unwrap_or(!res_str.contains("\"ok\":false"));
                                                    let content = res_str.clone();
                                                    items.push(Content::ToolResult {
                                                        id: id.clone(),
                                                        content,
                                                        ok,
                                                    });
                                                }
                                            }
                                            self.messages.push(Msg {
                                                role: m.role.clone(),
                                                content: m.content.clone(),
                                                items,
                                                timestamp: ts,
                                                reasoning: m.reasoning.clone(),
                                            });
                                            continue;
                                        }
                                    }
                                    self.messages.push(Msg {
                                        role: m.role.clone(),
                                        content: m.content.clone(),
                                        items: Vec::new(),
                                        timestamp: ts,
                                        reasoning: m.reasoning.clone(),
                                    });
                                }
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "⑂ Forked {} → {} at seq {:?}",
                                        &parent[..8.min(parent.len())],
                                        &new_id[..8.min(new_id.len())],
                                        at_seq
                                    ),
                                ));
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("fork failed: {e}")));
                        }
                    }
                }
            }
            "/rename" => {
                let title = parts[1..].join(" ");
                if title.is_empty() {
                    self.messages
                        .push(Msg::new("system", "usage: /rename <title>"));
                } else {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        match store.rename_session(&self.session_id, &title) {
                            Ok(_) => {
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "Renamed {} → '{}'",
                                        &self.session_id[..8.min(self.session_id.len())],
                                        title
                                    ),
                                ));
                            }
                            Err(e) => {
                                self.messages
                                    .push(Msg::new("system", format!("rename failed: {e}")));
                            }
                        }
                    }
                }
            }
            "/archive" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.archive_session(&id) {
                        Ok(_) => {
                            self.messages.push(Msg::new(
                                "system",
                                format!("Archived {}", &id[..8.min(id.len())]),
                            ));
                            if id == self.session_id {
                                let new_id = new_session_id();
                                self.session_id = new_id;
                                self.messages.clear();
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("archive failed: {e}")));
                        }
                    }
                }
            }
            "/delete" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.delete_session(&id) {
                        Ok(_) => {
                            self.messages.push(Msg::new(
                                "system",
                                format!("Deleted {} (CASCADE)", &id[..8.min(id.len())]),
                            ));
                            if id == self.session_id {
                                let new_id = new_session_id();
                                self.session_id = new_id;
                                self.messages.clear();
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("delete failed: {e}")));
                        }
                    }
                }
            }
            "/export" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.export_jsonl(&id) {
                        Ok(jsonl) => {
                            let path = format!(
                                "/tmp/vioraharness_export_{}.jsonl",
                                &id[..8.min(id.len())]
                            );
                            let _ = std::fs::write(&path, &jsonl);
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Exported {} ({} lines) → {} ({} bytes)",
                                    &id[..8.min(id.len())],
                                    jsonl.lines().count(),
                                    path,
                                    jsonl.len()
                                ),
                            ));
                        }
                        Err(e) => {
                            self.report_error("export", format!("export failed: {e}"));
                        }
                    }
                }
            }
            _ => {
                self.messages.push(Msg::new(
                    "system",
                    format!("unknown command: {cmd} (try /help)"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::testkit::*;
    #[test]
    fn skill_new_command_dispatch() {
        let mut app = test_app();
        let busy_before = app.busy;
        app.handle_slash("/skill-new");
        assert!(!busy_before && !app.busy, "usage path never submits");
        let last = app.messages.last().expect("usage message");
        assert!(
            last.content.contains("usage: /skill-new"),
            "{:?}",
            last.content
        );

        let mut app = test_app();
        app.busy = true;
        app.handle_slash("/skill-new plot things");
        let last = app.messages.last().expect("busy message");
        assert!(last.content.contains("busy"), "{:?}", last.content);
    }
}
