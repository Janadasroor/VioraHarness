use super::popups::rewind_checkpoints;
use super::*;

impl App {
    pub(crate) fn submit_text(&mut self, prompt: String) -> anyhow::Result<()> {
        // Subagent views own no input: refuse before echoing anything.
        if self.viewing_subagent() {
            self.messages.push(Msg::new(
                "system",
                "subagent view is read-only — Esc back to main to chat".to_string(),
            ));
            self.status = "read-only: subagent view (Esc back to main)".into();
            self.input.text.clear();
            self.input.cursor = 0;
            return Ok(());
        }
        if self.model.trim().is_empty() {
            self.messages.push(Msg::new(
                "system",
                "no model selected — pick one with /model (or /providers to add keys first)",
            ));
            self.model_filter.clear();
            self.model_cursor = 0;
            self.popup = Popup::ModelPicker;
            self.input.text.clear();
            self.input.cursor = 0;
            return Ok(());
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
        self.messages.push(Msg::new("user", send.clone()));
        self.input.push_history(history_text);
        self.input.text.clear();
        self.input.cursor = 0;
        self.start_turn(
            send,
            pending_img.map(|img| (img.mime.to_string(), img.b64)),
            "user",
        );
        Ok(())
    }

    /// `$` instant prompt: bypasses the queue and lands in the live turn at
    /// its next turn boundary. Echoed immediately (unlike queued prompts).
    /// No slash dispatch after `$` — the remainder is always prompt text.
    /// Falls back to the normal queue when no live turn for this session can
    /// take it (turn just ended, session switched) or when an image is
    /// attached (injection is text-only in v1).
    pub(crate) fn submit_instant(&mut self, prompt: String) {
        if self.viewing_subagent() {
            self.status = "read-only: subagent view (Esc back to main)".into();
            self.input.text.clear();
            self.input.cursor = 0;
            return;
        }
        let stripped = prompt
            .strip_prefix('$')
            .unwrap_or(&prompt)
            .trim()
            .to_string();
        if stripped.is_empty() {
            self.status = "empty $ prompt — ignored".into();
            self.input.text.clear();
            self.input.cursor = 0;
            return;
        }
        let live = match (&self.instant_injector, &self.turn_session) {
            (Some(inj), Some(ts)) if ts == &self.session_id && self.pending_image.is_none() => {
                Some(inj.clone())
            }
            _ => None,
        };
        let Some(inj) = live else {
            if self.pending_image.is_some() {
                self.queue_prompt(stripped);
                self.status = "image can't ride ⚡ — queued for next turn".into();
            } else {
                self.queue_prompt(stripped);
            }
            return;
        };
        let history_text = stripped.clone();
        let send = expand_paste_chips(&stripped, &self.pending_texts);
        self.pending_texts.clear();
        self.messages.push(Msg::new("user", send.clone()));
        self.input.push_history(history_text);
        self.input.text.clear();
        self.input.cursor = 0;
        inj.push(send);
        self.status = "⚡ sent to current turn".into();
    }

    pub(crate) fn start_turn(
        &mut self,
        send: String,
        image: Option<(String, String)>,
        prompt_role: &'static str,
    ) {
        // A subagent transcript never takes turns — user or system:
        // only the main agent owns the input box, and follow-up wakes
        // (task/subagent completions) must wait for the return to main.
        // Starting one here would persist main-chat content into the sub
        // session and mix both transcripts on screen. Holds after finish.
        if self.viewing_subagent() {
            self.messages.push(Msg::new(
                "system",
                "subagent view is read-only — Esc back to main to chat".to_string(),
            ));
            self.status = "read-only: subagent view (Esc back to main)".into();
            self.scroll = 0;
            return;
        }
        self.scroll = 0;
        self.selection = None;
        self.dragging = false;
        self.busy = true;
        self.status = if image.is_some() {
            "thinking… (with image)".into()
        } else {
            "thinking…".into()
        };
        self.streaming_buf.clear();
        self.thinking_buf.clear();
        let model = self.model.clone();
        let session_id = self.session_id.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        self.stream_rx = Some(rx);
        // The loop is built here (not inside the task) so its `$`
        // instant-prompt handle can be shared with the UI while it runs.
        // Mode-scoped: the model only sees the active mode's tools.
        let loop_ = vioraharness_core::loop_mod::AgentLoop::with_mode(&self.agent_mode);
        self.instant_injector = Some(loop_.injector.clone());
        self.turn_session = Some(session_id.clone());
        let handle = tokio::spawn(async move {
            std::env::set_var("VIORAHARNESS_TUI", "1");
            let res = if prompt_role == "user" {
                if let Some((mime, b64)) = image {
                    loop_
                        .run_streaming_with_image(
                            &send,
                            &model,
                            Some(session_id),
                            tx,
                            Some((mime, b64)),
                        )
                        .await
                } else {
                    loop_
                        .run_streaming(&send, &model, Some(session_id), tx)
                        .await
                }
            } else {
                // System-originated turn (task wake): stored/sent as
                // system, never as the user's own words.
                loop_
                    .run_streaming_system(&send, &model, Some(session_id), tx)
                    .await
            };
            std::env::remove_var("VIORAHARNESS_TUI");
            res
        });
        self.pending = Some(handle);
    }

    pub(crate) fn reload_display_from_store(
        &mut self,
        store: &vioraharness_core::session::SessionStore,
        sid: &str,
    ) -> Option<usize> {
        let msgs = store.get_messages_detailed(sid).ok()?;
        let tool_map = store.get_tool_calls_grouped(sid).unwrap_or_default();
        self.messages.clear();
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
                            let ok = serde_json::from_str::<serde_json::Value>(res_str)
                                .ok()
                                .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
                                .unwrap_or(!res_str.contains("\"ok\":false"));
                            items.push(Content::ToolResult {
                                id: id.clone(),
                                content: res_str.clone(),
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
        Some(self.messages.len())
    }

    pub(crate) fn parse_compact_freed(msg: &str) -> usize {
        if let Some(rest) = msg.strip_prefix("Compacted context:") {
            if let Some(i) = rest.find("freed ~") {
                let num: String = rest[i + 7..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(k) = num.parse::<usize>() {
                    return k * 1000;
                }
            }
        }
        0
    }

    /// Record a turn/command error: chat message + registry entry (header
    /// badge + /errors dialog). Returns the registry id.
    pub(crate) fn report_error(&mut self, source: &str, msg: String) -> String {
        let id = vioraharness_core::observe::push_error(source, &msg);
        self.messages.push(Msg::new("system", msg));
        id
    }

    /// Restore the session to `target_seq`: files back to checkpoint state,
    /// later conversation rows dropped, display reloaded from the store.
    pub(crate) fn do_rewind_to_seq(&mut self, target_seq: i64) {
        if self.busy {
            self.messages.push(Msg::new(
                "system",
                "busy — rewind after the current turn finishes",
            ));
            return;
        }
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        let sid = self.session_id.clone();
        let msg: String = (|| -> Result<String, String> {
            let store = vioraharness_core::session::SessionStore::new(&db)
                .map_err(|e| format!("rewind: cannot open session db: {e}"))?;
            let rep = vioraharness_core::session::snapshot::rewind_to_seq(&store, &sid, target_seq)
                .map_err(|e| format!("rewind failed: {e}"))?;
            let _ = self.reload_display_from_store(&store, &sid);
            self.scroll = 0;
            let checkpoint: String = rewind_checkpoints(&sid)
                .into_iter()
                .find(|p| p.target_seq == target_seq)
                .map(|p| format!("#{}", p.user_seq))
                .unwrap_or_else(|| format!("message #{target_seq}"));
            let mut parts = vec![format!(
                "rewound to checkpoint {checkpoint} — dropped {} message{}",
                rep.dropped_messages,
                if rep.dropped_messages == 1 { "" } else { "s" }
            )];
            if rep.dropped_tool_calls > 0 {
                parts.push(format!(
                    "{} tool call{}",
                    rep.dropped_tool_calls,
                    if rep.dropped_tool_calls == 1 { "" } else { "s" }
                ));
            }
            if !rep.restored_files.is_empty() {
                parts.push(format!(
                    "restored {} file{}: {}",
                    rep.restored_files.len(),
                    if rep.restored_files.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    rep.restored_files.join(", ")
                ));
            }
            if !rep.earliest_state_files.is_empty() {
                parts.push(format!(
                    "earliest-known state (created after checkpoint, kept): {}",
                    rep.earliest_state_files.join(", ")
                ));
            }
            if rep.dropped_messages == 0 && rep.restored_files.is_empty() {
                parts.push("already at latest — nothing changed".to_string());
            }
            Ok(parts.join("; "))
        })()
        .unwrap_or_else(|e| e);
        if msg.starts_with("rewind failed") {
            self.report_error("rewind", msg);
        } else {
            self.messages.push(Msg::new("system", msg));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;
    #[test]
    fn report_error_posts_chat_and_registry() {
        use vioraharness_core::observe;
        let _reg_guard = ERR_REG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = test_app();
        let id = app.report_error("test-turn-xyz", "model blew up: boom-xyz".to_string());
        assert!(id.starts_with("err_"));
        assert!(
            app.messages.iter().any(|m| m.content.contains("boom-xyz")),
            "chat shows the error"
        );
        assert!(
            observe::list_errors().iter().any(|e| e.id == id),
            "registry holds it"
        );
    }

    #[test]
    fn compact_notice_keeps_header_gauge_honest() {
        assert_eq!(App::parse_compact_freed("waiting 4s for rate limit"), 0);
        assert_eq!(
            App::parse_compact_freed("Compacted context: 130→20 msgs, freed ~42k tokens"),
            42_000
        );
        assert_eq!(
            App::parse_compact_freed("Compacted context: 40→20 msgs, freed ~3k tokens"),
            3_000
        );

        let mut app = test_app();
        app.ctx_freed_tokens +=
            App::parse_compact_freed("Compacted context: 40→20 msgs, freed ~3k tokens");
        assert_eq!(app.ctx_freed_tokens, 3_000);
    }
}
