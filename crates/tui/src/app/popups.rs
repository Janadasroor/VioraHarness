// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl App {
    pub(crate) fn handle_popup_key(&mut self, key: KeyEvent) {
        match self.popup {
            Popup::ThemePicker => match key.code {
                KeyCode::Up => {
                    if self.theme_cursor > 0 {
                        self.theme_cursor -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.theme_cursor + 1 < self.available_themes.len() {
                        self.theme_cursor += 1;
                    }
                }
                KeyCode::Enter => {
                    let name = self.available_themes[self.theme_cursor].clone();
                    super::settings::persist_theme_to_config(&name);
                    self.messages.push(Msg::new(
                        "system",
                        format!("Theme → {name} (saved, applied)"),
                    ));
                    self.popup = Popup::None;
                }
                KeyCode::Esc => self.popup = Popup::None,
                _ => {}
            },
            Popup::Settings => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.settings_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.settings_move(1),
                KeyCode::Home => self.settings_cursor = 0,
                KeyCode::End => self.settings_cursor = Self::settings_len() - 1,
                KeyCode::PageUp => self.settings_move(-4),
                KeyCode::PageDown => self.settings_move(4),
                KeyCode::Left | KeyCode::Char('h') => self.settings_cycle(-1),
                KeyCode::Right | KeyCode::Char('l') => self.settings_cycle(1),
                KeyCode::Enter | KeyCode::Char(' ') => self.settings_activate(),
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.popup = Popup::None,
                _ => {}
            },
            Popup::ModePicker => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    let len = vioraharness_core::mode::builtin_modes().len();
                    if self.mode_cursor > 0 {
                        self.mode_cursor -= 1;
                    } else if len > 0 {
                        self.mode_cursor = len - 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let len = vioraharness_core::mode::builtin_modes().len();
                    if len > 0 && self.mode_cursor + 1 < len {
                        self.mode_cursor += 1;
                    } else {
                        self.mode_cursor = 0;
                    }
                }
                KeyCode::Enter => {
                    let modes = vioraharness_core::mode::builtin_modes();
                    if let Some(m) = modes.get(self.mode_cursor) {
                        let name = m.name.to_string();
                        self.popup = Popup::None;
                        self.apply_agent_mode(&name);
                    } else {
                        self.popup = Popup::None;
                    }
                }
                KeyCode::Esc => self.popup = Popup::None,
                _ => {}
            },
            Popup::ModelPicker => match key.code {
                KeyCode::Up => {
                    let filtered_len = self.model_filtered_len();
                    if self.model_cursor > 0 {
                        self.model_cursor -= 1;
                    } else if filtered_len > 0 {
                        self.model_cursor = filtered_len - 1;
                    }
                }
                KeyCode::Down => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len > 0 && self.model_cursor + 1 < filtered_len {
                        self.model_cursor += 1;
                    } else {
                        self.model_cursor = 0;
                    }
                }
                KeyCode::PageUp => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len == 0 {
                        return;
                    }
                    let page = popup_list_visible(7);
                    if self.model_cursor >= page {
                        self.model_cursor -= page;
                    } else {
                        self.model_cursor = 0;
                    }
                }
                KeyCode::PageDown => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len == 0 {
                        return;
                    }
                    let page = popup_list_visible(7);
                    if self.model_cursor + page < filtered_len {
                        self.model_cursor += page;
                    } else {
                        self.model_cursor = filtered_len - 1;
                    }
                }
                KeyCode::Home => {
                    self.model_cursor = 0;
                }
                KeyCode::End => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len > 0 {
                        self.model_cursor = filtered_len - 1;
                    }
                }
                KeyCode::Left => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len == 0 {
                        return;
                    }
                    let page = popup_list_visible(7);
                    if self.model_cursor >= page {
                        self.model_cursor -= page;
                    } else {
                        self.model_cursor = 0;
                    }
                }
                KeyCode::Right => {
                    let filtered_len = self.model_filtered_len();
                    if filtered_len == 0 {
                        return;
                    }
                    let page = popup_list_visible(7);
                    if self.model_cursor + page < filtered_len {
                        self.model_cursor += page;
                    } else {
                        self.model_cursor = filtered_len - 1;
                    }
                }
                KeyCode::Enter => {
                    let filter = self.model_filter.to_lowercase();
                    let filtered: Vec<String> = self
                        .available_models
                        .iter()
                        .filter(|m| filter.is_empty() || m.to_lowercase().contains(&filter))
                        .cloned()
                        .collect();
                    if !filtered.is_empty() {
                        let idx = self.model_cursor.min(filtered.len().saturating_sub(1));
                        self.model = filtered[idx].clone();
                        Self::save_tui_state(serde_json::json!({"last_model": self.model}));
                        self.messages.push(Msg::new(
                            "system",
                            format!("model → {} (saved)", self.model),
                        ));
                    } else if !self.model_filter.trim().is_empty() {
                        let custom = self.model_filter.trim().to_string();
                        self.model = custom.clone();
                        Self::save_tui_state(serde_json::json!({"last_model": self.model}));
                        self.messages.push(Msg::new(
                            "system",
                            format!("model → {} (custom, saved)", custom),
                        ));
                    } else {
                        self.messages.push(Msg::new("system", "No model selected"));
                    }
                    self.model_filter.clear();
                    self.model_cursor = 0;
                    self.popup = Popup::None;
                }
                KeyCode::Esc => {
                    if !self.model_filter.is_empty() {
                        self.model_filter.clear();
                        self.model_cursor = 0;
                    } else {
                        self.popup = Popup::None;
                    }
                }
                KeyCode::Backspace => {
                    self.model_filter.pop();
                    self.model_cursor = 0;
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.model_filter.push(c);
                    self.model_cursor = 0;
                }
                _ => {}
            },
            Popup::Tasks => match key.code {
                KeyCode::Up => {
                    let len = vioraharness_core::tools::tasks::list_tasks().len();
                    if self.task_cursor > 0 {
                        self.task_cursor -= 1;
                    } else if len > 0 {
                        self.task_cursor = len.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    let len = vioraharness_core::tools::tasks::list_tasks().len();
                    if len > 0 && self.task_cursor + 1 < len {
                        self.task_cursor += 1;
                    } else {
                        self.task_cursor = 0;
                    }
                }
                KeyCode::PageUp => {
                    let page = popup_list_visible(6);
                    self.task_cursor = self.task_cursor.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    let len = vioraharness_core::tools::tasks::list_tasks().len();
                    let page = popup_list_visible(6);
                    if len > 0 {
                        self.task_cursor = (self.task_cursor + page).min(len - 1);
                    }
                }
                KeyCode::Home => {
                    self.task_cursor = 0;
                }
                KeyCode::End => {
                    let len = vioraharness_core::tools::tasks::list_tasks().len();
                    if len > 0 {
                        self.task_cursor = len - 1;
                    }
                }
                KeyCode::Enter => {
                    let all = vioraharness_core::tools::tasks::list_tasks();
                    if let Some(t) = all.get(self.task_cursor) {
                        let log = vioraharness_core::tools::tasks::read_task_log(&t.id)
                            .unwrap_or_else(|| "(log unavailable)".to_string());
                        let ok = t.status == vioraharness_core::tools::tasks::BgStatus::Done;
                        self.last_tool_output = Some((
                            t.id.clone(),
                            format!("task {}", short_task_id(&t.id)),
                            log,
                            ok,
                        ));
                        self.tool_output_scroll = 0;
                        self.popup = Popup::ToolOutput;
                    }
                }
                KeyCode::Char('k') | KeyCode::Char('K') => {
                    let all = vioraharness_core::tools::tasks::list_tasks();
                    if let Some(t) = all.get(self.task_cursor) {
                        if vioraharness_core::tools::tasks::kill_task(&t.id) {
                            self.status = format!("killed {}", short_task_id(&t.id));
                        } else {
                            self.status = format!("{} not running", short_task_id(&t.id));
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.popup = Popup::None;
                }
                _ => {}
            },
            Popup::Agents => match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    let len = super::agents::agent_rows(self).len();
                    if self.agent_cursor > 0 {
                        self.agent_cursor -= 1;
                    } else if len > 0 {
                        self.agent_cursor = len.saturating_sub(1);
                    }
                    self.anchor_agent_cursor();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let len = super::agents::agent_rows(self).len();
                    if len > 0 && self.agent_cursor + 1 < len {
                        self.agent_cursor += 1;
                    } else {
                        self.agent_cursor = 0;
                    }
                    self.anchor_agent_cursor();
                }
                KeyCode::PageUp => {
                    let page = popup_list_visible(6);
                    self.agent_cursor = self.agent_cursor.saturating_sub(page);
                    self.anchor_agent_cursor();
                }
                KeyCode::PageDown => {
                    let len = super::agents::agent_rows(self).len();
                    let page = popup_list_visible(6);
                    if len > 0 {
                        self.agent_cursor = (self.agent_cursor + page).min(len - 1);
                    }
                    self.anchor_agent_cursor();
                }
                KeyCode::Home => {
                    self.agent_cursor = 0;
                    self.anchor_agent_cursor();
                }
                KeyCode::End => {
                    let len = super::agents::agent_rows(self).len();
                    if len > 0 {
                        self.agent_cursor = len - 1;
                    }
                    self.anchor_agent_cursor();
                }
                KeyCode::Enter => {
                    self.agents_activate();
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    self.agents_kill();
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.popup = Popup::None;
                }
                _ => {}
            },
            Popup::Errors => match key.code {
                KeyCode::Up => {
                    let len = vioraharness_core::observe::list_errors().len();
                    if self.error_cursor > 0 {
                        self.error_cursor -= 1;
                    } else if len > 0 {
                        self.error_cursor = len.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    let len = vioraharness_core::observe::list_errors().len();
                    if len > 0 && self.error_cursor + 1 < len {
                        self.error_cursor += 1;
                    } else {
                        self.error_cursor = 0;
                    }
                }
                KeyCode::PageUp => {
                    let page = popup_list_visible(6);
                    self.error_cursor = self.error_cursor.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    let len = vioraharness_core::observe::list_errors().len();
                    let page = popup_list_visible(6);
                    if len > 0 {
                        self.error_cursor = (self.error_cursor + page).min(len - 1);
                    }
                }
                KeyCode::Home => {
                    self.error_cursor = 0;
                }
                KeyCode::End => {
                    let len = vioraharness_core::observe::list_errors().len();
                    if len > 0 {
                        self.error_cursor = len - 1;
                    }
                }
                KeyCode::Enter => {
                    let all = vioraharness_core::observe::list_errors();
                    if let Some(e) = all.get(self.error_cursor) {
                        self.last_tool_output = Some((
                            e.id.clone(),
                            format!("error {}", short_err_id(&e.id)),
                            e.full.clone(),
                            false,
                        ));
                        self.tool_output_scroll = 0;
                        self.popup = Popup::ToolOutput;
                    }
                }
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    vioraharness_core::observe::clear_errors();
                    self.error_cursor = 0;
                    self.status = "errors cleared".into();
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.popup = Popup::None;
                }
                _ => {}
            },
            Popup::Rewind => match key.code {
                KeyCode::Up => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    if self.rewind_cursor > 0 {
                        self.rewind_cursor -= 1;
                    }
                }
                KeyCode::Down => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    let len = rewind_checkpoints(&self.session_id).len();
                    if len > 0 && self.rewind_cursor + 1 < len {
                        self.rewind_cursor += 1;
                    }
                }
                KeyCode::PageUp => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    let page = popup_list_visible(6);
                    self.rewind_cursor = self.rewind_cursor.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    let len = rewind_checkpoints(&self.session_id).len();
                    let page = popup_list_visible(6);
                    if len > 0 {
                        self.rewind_cursor = (self.rewind_cursor + page).min(len - 1);
                    }
                }
                KeyCode::Home => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    self.rewind_cursor = 0;
                }
                KeyCode::End => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    let len = rewind_checkpoints(&self.session_id).len();
                    if len > 0 {
                        self.rewind_cursor = len - 1;
                    }
                }
                KeyCode::Enter => {
                    let points = rewind_checkpoints(&self.session_id);
                    if let Some(p) = points.get(self.rewind_cursor).cloned() {
                        if self.rewind_armed == Some(p.target_seq) {
                            self.rewind_armed = None;
                            self.rewind_armed_note = None;
                            self.popup = Popup::None;
                            self.do_rewind_to_seq(p.target_seq);
                        } else {
                            self.arm_rewind_checkpoint(&p);
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                    self.rewind_armed = None;
                    self.rewind_armed_note = None;
                    self.popup = Popup::None;
                }
                _ => {}
            },
            Popup::Sessions => match key.code {
                KeyCode::Up => {
                    let len = self.session_filtered_len();
                    if self.session_cursor > 0 {
                        self.session_cursor -= 1;
                    } else if len > 0 {
                        self.session_cursor = len.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    let len = self.session_filtered_len();
                    if len > 0 && self.session_cursor + 1 < len {
                        self.session_cursor += 1;
                    } else {
                        self.session_cursor = 0;
                    }
                }
                KeyCode::PageUp => {
                    let len = self.session_filtered_len();
                    if len == 0 {
                        return;
                    }
                    let page = popup_list_visible(6);
                    if self.session_cursor >= page {
                        self.session_cursor -= page;
                    } else {
                        self.session_cursor = 0;
                    }
                }
                KeyCode::PageDown => {
                    let len = self.session_filtered_len();
                    if len == 0 {
                        return;
                    }
                    let page = popup_list_visible(6);
                    if self.session_cursor + page < len {
                        self.session_cursor += page;
                    } else {
                        self.session_cursor = len - 1;
                    }
                }
                KeyCode::Home => {
                    self.session_cursor = 0;
                }
                KeyCode::End => {
                    let len = self.session_filtered_len();
                    if len > 0 {
                        self.session_cursor = len - 1;
                    }
                }
                KeyCode::Left => {
                    let len = self.session_filtered_len();
                    if len == 0 {
                        return;
                    }
                    let page = popup_list_visible(6);
                    if self.session_cursor >= page {
                        self.session_cursor -= page;
                    } else {
                        self.session_cursor = 0;
                    }
                }
                KeyCode::Right => {
                    let len = self.session_filtered_len();
                    if len == 0 {
                        return;
                    }
                    let page = popup_list_visible(6);
                    if self.session_cursor + page < len {
                        self.session_cursor += page;
                    } else {
                        self.session_cursor = len - 1;
                    }
                }
                KeyCode::Backspace => {
                    if !self.session_filter.is_empty() {
                        self.session_filter.pop();
                        self.session_cursor = 0;
                    }
                }
                KeyCode::Esc => {
                    if !self.session_filter.is_empty() {
                        self.session_filter.clear();
                        self.session_cursor = 0;
                    } else {
                        self.popup = Popup::None;
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    let is_cmd = matches!(
                        c,
                        'n' | 'N' | 'f' | 'F' | 'a' | 'A' | 'd' | 'D' | 'r' | 'R' | 'p' | 'P'
                    ) && self.session_filter.is_empty();
                    if !is_cmd {
                        self.session_filter.push(c);
                        self.session_cursor = 0;
                    } else {
                        match c {
                            'n' | 'N' => {
                                let new_id = new_session_id();
                                self.session_id = new_id.clone();
                                self.messages.clear();
                                self.messages.push(Msg::new(
                                    "system",
                                    format!("New chat {new_id} — previous chats saved in SQLite"),
                                ));
                                self.scroll = 0;
                                self.status = "ready".into();
                                self.session_cursor = 0;
                                self.popup = Popup::None;
                            }
                            'f' | 'F' => {
                                let db = std::env::var("VIORAHARNESS_DB").unwrap_or_else(|_| {
                                    "~/.local/share/vioraharness/sessions.db".into()
                                });
                                if let Ok(store) =
                                    vioraharness_core::session::SessionStore::new(&db)
                                {
                                    let parent_id = {
                                        let all = store
                                            .list_sessions_filtered(
                                                self.session_scope_filter().as_deref(),
                                                None,
                                                true,
                                                50,
                                                0,
                                            )
                                            .unwrap_or_default();
                                        let f = self.session_filter.to_lowercase();
                                        let filtered: Vec<_> = all
                                            .into_iter()
                                            .filter(|s| {
                                                f.is_empty()
                                                    || s.id.to_lowercase().contains(&f)
                                                    || s.title
                                                        .as_ref()
                                                        .map(|t| t.to_lowercase().contains(&f))
                                                        .unwrap_or(false)
                                            })
                                            .collect();
                                        filtered
                                            .get(self.session_cursor)
                                            .map(|s| s.id.clone())
                                            .unwrap_or_else(|| self.session_id.clone())
                                    };
                                    let new_id = new_session_id();
                                    match store.fork_session(&parent_id, &new_id, None) {
                                        Ok(_) => {
                                            self.session_id = new_id.clone();
                                            if let Ok(msgs) = store.get_messages_detailed(&new_id) {
                                                self.messages.clear();
                                                let tool_map = store
                                                    .get_tool_calls_grouped(&new_id)
                                                    .unwrap_or_default();
                                                for m in msgs {
                                                    if m.role == "tool" {
                                                        continue;
                                                    }
                                                    let ts =
                                                        m.timestamp.clone().unwrap_or_else(|| {
                                                            format!(
                                                                "{:02}:{:02}",
                                                                (m.created_at % 86400 / 3600) % 24,
                                                                (m.created_at % 3600) / 60
                                                            )
                                                        });
                                                    if m.role == "assistant" {
                                                        if let Some(calls) = tool_map.get(&m.seq) {
                                                            let mut items = Vec::new();
                                                            for (cid, name, args, _sig, result) in
                                                                calls
                                                            {
                                                                items.push(Content::ToolCall {
                                                                    id: cid.clone(),
                                                                    name: name.clone(),
                                                                    args: args.clone(),
                                                                    status: ToolStatus::Done,
                                                                });
                                                                if let Some(res_str) = result {
                                                                    let ok =
                                                                        serde_json::from_str::<
                                                                            serde_json::Value,
                                                                        >(
                                                                            res_str
                                                                        )
                                                                        .ok()
                                                                        .and_then(|v| {
                                                                            v.get("ok").and_then(
                                                                                |x| x.as_bool(),
                                                                            )
                                                                        })
                                                                        .unwrap_or(
                                                                            !res_str.contains(
                                                                                "\"ok\":false",
                                                                            ),
                                                                        );
                                                                    let content = res_str.clone();
                                                                    items.push(
                                                                        Content::ToolResult {
                                                                            id: cid.clone(),
                                                                            content,
                                                                            ok,
                                                                        },
                                                                    );
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
                                                        "⑂ Forked {} → {} ({} msgs)",
                                                        &parent_id[..8.min(parent_id.len())],
                                                        &new_id[..8.min(new_id.len())],
                                                        self.messages.len()
                                                    ),
                                                ));
                                            } else {
                                                self.messages.push(Msg::new(
                                                    "system",
                                                    format!(
                                                        "⑂ Forked {} → {} (full history copied)",
                                                        &parent_id[..8.min(parent_id.len())],
                                                        &new_id[..8.min(new_id.len())]
                                                    ),
                                                ));
                                            }
                                        }
                                        Err(e) => {
                                            self.messages.push(Msg::new(
                                                "system",
                                                format!("fork failed: {e}"),
                                            ));
                                        }
                                    }
                                }
                                self.popup = Popup::None;
                            }
                            'a' | 'A' => {
                                let db = std::env::var("VIORAHARNESS_DB").unwrap_or_else(|_| {
                                    "~/.local/share/vioraharness/sessions.db".into()
                                });
                                if let Ok(store) =
                                    vioraharness_core::session::SessionStore::new(&db)
                                {
                                    let all = store
                                        .list_sessions_filtered(
                                            self.session_scope_filter().as_deref(),
                                            None,
                                            true,
                                            50,
                                            0,
                                        )
                                        .unwrap_or_default();
                                    let f = self.session_filter.to_lowercase();
                                    let filtered: Vec<_> = all
                                        .into_iter()
                                        .filter(|s| {
                                            f.is_empty()
                                                || s.id.to_lowercase().contains(&f)
                                                || s.title
                                                    .as_ref()
                                                    .map(|t| t.to_lowercase().contains(&f))
                                                    .unwrap_or(false)
                                        })
                                        .collect();
                                    if let Some(sess) = filtered.get(self.session_cursor) {
                                        let _ = store.archive_session(&sess.id);
                                        self.messages.push(Msg::new(
                                            "system",
                                            format!(
                                                "Archived {}",
                                                &sess.id[..8.min(sess.id.len())]
                                            ),
                                        ));
                                        if sess.id == self.session_id {
                                            let new_id = new_session_id();
                                            self.session_id = new_id;
                                            self.messages.clear();
                                        }
                                        if self.session_cursor > 0 {
                                            self.session_cursor -= 1;
                                        }
                                    }
                                }
                            }
                            'd' | 'D' => {
                                let db = std::env::var("VIORAHARNESS_DB").unwrap_or_else(|_| {
                                    "~/.local/share/vioraharness/sessions.db".into()
                                });
                                if let Ok(store) =
                                    vioraharness_core::session::SessionStore::new(&db)
                                {
                                    let all = store
                                        .list_sessions_filtered(
                                            self.session_scope_filter().as_deref(),
                                            None,
                                            true,
                                            50,
                                            0,
                                        )
                                        .unwrap_or_default();
                                    let f = self.session_filter.to_lowercase();
                                    let filtered: Vec<_> = all
                                        .into_iter()
                                        .filter(|s| {
                                            f.is_empty()
                                                || s.id.to_lowercase().contains(&f)
                                                || s.title
                                                    .as_ref()
                                                    .map(|t| t.to_lowercase().contains(&f))
                                                    .unwrap_or(false)
                                        })
                                        .collect();
                                    if let Some(sess) = filtered.get(self.session_cursor) {
                                        let id = sess.id.clone();
                                        let _ = store.delete_session(&id);
                                        self.messages.push(Msg::new(
                                            "system",
                                            format!("Deleted {} — CASCADE", &id[..8.min(id.len())]),
                                        ));
                                        if id == self.session_id {
                                            let new_id = new_session_id();
                                            self.session_id = new_id;
                                            self.messages.clear();
                                        }
                                        if self.session_cursor > 0 {
                                            self.session_cursor -= 1;
                                        }
                                    }
                                }
                            }
                            'r' | 'R' => {
                                self.messages.push(Msg::new(
                                    "system",
                                    "Rename: use /rename <title> in input (or /sessions then r)",
                                ));
                            }
                            'p' | 'P' => {
                                self.show_all_sessions = !self.show_all_sessions;
                                self.session_cursor = 0;
                            }
                            _ => {
                                self.session_filter.push(c);
                                self.session_cursor = 0;
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        let all = store
                            .list_sessions_filtered(
                                self.session_scope_filter().as_deref(),
                                None,
                                true,
                                50,
                                0,
                            )
                            .unwrap_or_default();
                        let f = self.session_filter.to_lowercase();
                        let filtered: Vec<_> = all
                            .into_iter()
                            .filter(|s| {
                                f.is_empty()
                                    || s.id.to_lowercase().contains(&f)
                                    || s.title
                                        .as_ref()
                                        .map(|t| t.to_lowercase().contains(&f))
                                        .unwrap_or(false)
                                    || s.model.to_lowercase().contains(&f)
                            })
                            .collect();
                        if let Some(sess) = filtered.get(self.session_cursor) {
                            let id = sess.id.clone();
                            let model = sess.model.clone();
                            let theme = sess.theme.clone();
                            let stored_mode = sess.mode.clone();

                            match store.get_messages_detailed(&id) {
                                Ok(msgs) => {
                                    self.session_id = id.clone();
                                    self.model = model.clone();
                                    if let Some(th) = theme {
                                        Self::save_tui_state(serde_json::json!({"last_theme": th}));
                                    }
                                    if let Some(m) = stored_mode {
                                        let norm = vioraharness_core::mode::normalize_mode_name(&m);
                                        if vioraharness_core::mode::is_known_mode(&norm) {
                                            self.agent_mode = norm;
                                        }
                                    }
                                    Self::save_tui_state(
                                        serde_json::json!({"last_model": self.model}),
                                    );
                                    self.messages.clear();
                                    self.scroll = 0;
                                    let tool_map =
                                        store.get_tool_calls_grouped(&id).unwrap_or_default();
                                    for m in msgs {
                                        if m.role == "tool" {
                                            continue;
                                        }
                                        let ts = m.timestamp.clone().unwrap_or_else(|| {
                                            let secs = m.created_at % 86400;
                                            format!(
                                                "{:02}:{:02}",
                                                (secs / 3600) % 24,
                                                (secs % 3600) / 60
                                            )
                                        });
                                        if m.role == "assistant" {
                                            if let Some(calls) = tool_map.get(&m.seq) {
                                                let mut items = Vec::new();
                                                for (cid, name, args, _sig, result) in calls {
                                                    items.push(Content::ToolCall {
                                                        id: cid.clone(),
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
                                                        .unwrap_or(
                                                            !res_str.contains("\"ok\":false"),
                                                        );
                                                        let content = res_str.clone();
                                                        items.push(Content::ToolResult {
                                                            id: cid.clone(),
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
                                            "↩︎ Resumed chat {} ({} msgs) — model {}",
                                            &id[..8.min(id.len())],
                                            self.messages.len(),
                                            model
                                        ),
                                    ));
                                    self.status = "ready".into();
                                }
                                Err(e) => {
                                    self.messages
                                        .push(Msg::new("system", format!("resume failed: {e}")));
                                }
                            }
                        }
                    }
                    self.session_filter.clear();
                    self.popup = Popup::None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    let new_id = new_session_id();
                    self.session_id = new_id.clone();
                    self.messages.clear();
                    self.messages.push(Msg::new(
                        "system",
                        format!("New chat {new_id} — previous chats saved in SQLite"),
                    ));
                    self.scroll = 0;
                    self.status = "ready".into();
                    self.session_cursor = 0;
                    self.popup = Popup::None;
                }
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        let parent_id = {
                            if let Ok(sessions) = store.list_sessions_filtered(
                                self.session_scope_filter().as_deref(),
                                None,
                                false,
                                50,
                                0,
                            ) {
                                sessions
                                    .get(self.session_cursor)
                                    .map(|s| s.id.clone())
                                    .unwrap_or_else(|| self.session_id.clone())
                            } else {
                                self.session_id.clone()
                            }
                        };
                        let new_id = new_session_id();
                        match store.fork_session(&parent_id, &new_id, None) {
                            Ok(_) => {
                                self.session_id = new_id.clone();
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "⑂ Forked {} → {} (full history copied)",
                                        &parent_id[..8.min(parent_id.len())],
                                        &new_id[..8.min(new_id.len())]
                                    ),
                                ));

                                if let Ok(msgs) = store.get_messages_detailed(&new_id) {
                                    self.messages.clear();
                                    for m in msgs {
                                        let ts = {
                                            let secs = m.created_at % 86400;
                                            format!(
                                                "{:02}:{:02}",
                                                (secs / 3600) % 24,
                                                (secs % 3600) / 60
                                            )
                                        };
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
                                        format!("⑂ Fork ready — {} msgs", self.messages.len()),
                                    ));
                                }
                            }
                            Err(e) => {
                                self.messages
                                    .push(Msg::new("system", format!("fork failed: {e}")));
                            }
                        }
                    }
                    self.popup = Popup::None;
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        if let Ok(sessions) = store.list_sessions_filtered(
                            self.session_scope_filter().as_deref(),
                            None,
                            false,
                            50,
                            0,
                        ) {
                            if let Some(sess) = sessions.get(self.session_cursor) {
                                let _ = store.archive_session(&sess.id);
                                self.messages.push(Msg::new(
                                    "system",
                                    format!("Archived {}", &sess.id[..8.min(sess.id.len())]),
                                ));
                                if sess.id == self.session_id {
                                    let new_id = new_session_id();
                                    self.session_id = new_id;
                                    self.messages.clear();
                                }
                                if self.session_cursor > 0 {
                                    self.session_cursor -= 1;
                                }
                            }
                        }
                    }
                }
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        if let Ok(sessions) = store.list_sessions_filtered(
                            self.session_scope_filter().as_deref(),
                            None,
                            false,
                            50,
                            0,
                        ) {
                            if let Some(sess) = sessions.get(self.session_cursor) {
                                let id = sess.id.clone();
                                let _ = store.delete_session(&id);
                                self.messages.push(Msg::new("system", format!("Deleted {} — SQLite CASCADE removed messages/tool_calls/events", &id[..8.min(id.len())])));
                                if id == self.session_id {
                                    let new_id = new_session_id();
                                    self.session_id = new_id;
                                    self.messages.clear();
                                }
                                if self.session_cursor > 0 {
                                    self.session_cursor -= 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Popup::Providers => {
                let catalog = provider_catalog();
                if self.provider_input_active {
                    match key.code {
                        KeyCode::Esc => {
                            self.provider_input_active = false;
                            self.provider_key_input.clear();
                            self.provider_selected = None;
                            self.provider_msg = None;
                        }
                        KeyCode::Enter => {
                            let key_val = self.provider_key_input.trim().to_string();
                            if key_val.len() < 8 {
                                self.provider_msg = Some(("Key too short".into(), false));
                            } else if let Some(pid) = self.provider_selected.clone() {
                                let env_name = catalog
                                    .iter()
                                    .find(|(id, _, _, _)| *id == pid)
                                    .map(|(_, _, env, _)| *env)
                                    .unwrap_or("API_KEY");
                                match persist_provider_key(env_name, &key_val) {
                                    Ok(path) => {
                                        self.available_models =
                                            Self::fetch_models_from_openrouter();
                                        let local_cnt = self.available_models.len();

                                        let (tx, rx) = tokio::sync::mpsc::channel(1);
                                        self.model_fetch_rx = Some(rx);
                                        tokio::spawn(async move {
                                            let live = Self::fetch_models_live().await;
                                            let _ = tx.send(live).await;
                                        });
                                        self.status = "key saved — fetching models…".into();

                                        let pid_clone = pid.clone();
                                        let key_clone = key_val.clone();
                                        let path_clone = path.clone();
                                        tokio::spawn(async move {
                                            match validate_provider_key(&pid_clone, &key_clone)
                                                .await
                                            {
                                                Ok(cnt) => {
                                                    tracing::info!(
                                                        "provider {} validated: {} models",
                                                        pid_clone,
                                                        cnt
                                                    );
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "provider {} validation failed: {}",
                                                        pid_clone,
                                                        e
                                                    );
                                                }
                                            }
                                        });
                                        self.provider_msg = Some((format!("Saved {} → {} (0600) — {} models available instantly, live fetch in progress… (open /model to see)", env_name, path_clone.display(), local_cnt), true));
                                    }
                                    Err(e) => {
                                        self.provider_msg =
                                            Some((format!("Save failed: {e}"), false))
                                    }
                                }
                                self.provider_input_active = false;
                                self.provider_key_input.clear();
                            }
                        }
                        KeyCode::Backspace => {
                            self.provider_key_input.pop();
                        }
                        KeyCode::Char(c)
                            if !key.modifiers.contains(KeyModifiers::CONTROL)
                                && !key.modifiers.contains(KeyModifiers::ALT) =>
                        {
                            self.provider_key_input.push(c);
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up => {
                            if self.provider_cursor > 0 {
                                self.provider_cursor -= 1;
                            } else {
                                self.provider_cursor = catalog.len().saturating_sub(1);
                            }
                        }
                        KeyCode::Down => {
                            if self.provider_cursor + 1 < catalog.len() {
                                self.provider_cursor += 1;
                            } else {
                                self.provider_cursor = 0;
                            }
                        }
                        KeyCode::Enter => {
                            if let Some((pid, _, env_name, _)) = catalog.get(self.provider_cursor) {
                                self.provider_selected = Some(pid.to_string());
                                self.provider_input_active = true;
                                self.provider_key_input.clear();
                                self.provider_msg = Some((
                                    format!("Paste key for {env_name} (masked, Enter to save)"),
                                    true,
                                ));
                                self.provider_validating = false;
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            if let Some((_, _, env_name, _)) = catalog.get(self.provider_cursor) {
                                std::env::remove_var(env_name);

                                let path = providers_env_path();
                                if path.exists() {
                                    if let Ok(content) = std::fs::read_to_string(&path) {
                                        let filtered: Vec<String> = content
                                            .lines()
                                            .filter(|l| {
                                                !l.trim_start().starts_with(&format!("{env_name}="))
                                            })
                                            .map(|s| s.to_string())
                                            .collect();
                                        let _ = std::fs::write(
                                            &path,
                                            filtered.join("\n")
                                                + if filtered.is_empty() { "" } else { "\n" },
                                        );
                                    }
                                }
                                self.provider_msg = Some((format!("Cleared {env_name}"), true));
                            }
                        }
                        KeyCode::Esc => self.popup = Popup::None,
                        _ => {}
                    }
                }
            }
            Popup::Diff => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = Popup::None,
                _ => self.popup = Popup::None,
            },
            Popup::PermissionAsk => match key.code {
                KeyCode::Up => {
                    if self.perm_cursor > 0 {
                        self.perm_cursor -= 1;
                    } else {
                        self.perm_cursor = 2;
                    }
                }
                KeyCode::Down => {
                    self.perm_cursor = (self.perm_cursor + 1) % 3;
                }
                KeyCode::Char('a') => {
                    self.resolve_perm(
                        vioraharness_core::permissions::InteractiveDecision::AllowOnce,
                    );
                }
                KeyCode::Char('A') => {
                    self.resolve_perm(
                        vioraharness_core::permissions::InteractiveDecision::AllowAlways,
                    );
                }
                KeyCode::Char('d')
                | KeyCode::Char('D')
                | KeyCode::Char('n')
                | KeyCode::Char('N') => {
                    self.resolve_perm(vioraharness_core::permissions::InteractiveDecision::Deny);
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.resolve_perm(
                        vioraharness_core::permissions::InteractiveDecision::AllowOnce,
                    );
                }
                KeyCode::Enter => {
                    let dec = match self.perm_cursor {
                        0 => vioraharness_core::permissions::InteractiveDecision::AllowOnce,
                        1 => vioraharness_core::permissions::InteractiveDecision::AllowAlways,
                        _ => vioraharness_core::permissions::InteractiveDecision::Deny,
                    };
                    self.resolve_perm(dec);
                }
                KeyCode::Esc => {
                    self.resolve_perm(vioraharness_core::permissions::InteractiveDecision::Deny);
                }
                _ => {}
            },
            Popup::Question => match key.code {
                _ if self
                    .pending_q
                    .as_ref()
                    .is_some_and(|r| r.questions.is_empty()) =>
                {
                    self.resolve_question(
                        vioraharness_core::permissions::QuestionResult::Cancelled,
                    );
                }
                KeyCode::Up => {
                    if self.q_custom_active {
                        return;
                    }
                    if let Some(req) = &self.pending_q {
                        let qi = self
                            .q_answered
                            .len()
                            .min(req.questions.len().saturating_sub(1));
                        let rows = req.questions[qi].options.len() + 1;
                        if self.q_cursor > 0 {
                            self.q_cursor -= 1;
                        } else {
                            self.q_cursor = rows.saturating_sub(1);
                        }
                    }
                }
                KeyCode::Down => {
                    if self.q_custom_active {
                        return;
                    }
                    if let Some(req) = &self.pending_q {
                        let qi = self
                            .q_answered
                            .len()
                            .min(req.questions.len().saturating_sub(1));
                        let rows = req.questions[qi].options.len() + 1;
                        if rows > 0 {
                            self.q_cursor = (self.q_cursor + 1) % rows;
                        }
                    }
                }
                KeyCode::Home => {
                    if !self.q_custom_active {
                        self.q_cursor = 0;
                    }
                }
                KeyCode::End => {
                    if !self.q_custom_active {
                        if let Some(req) = &self.pending_q {
                            let qi = self
                                .q_answered
                                .len()
                                .min(req.questions.len().saturating_sub(1));
                            self.q_cursor = req.questions[qi].options.len();
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    if self.q_custom_active {
                        self.q_custom.push(' ');
                        return;
                    }
                    self.answer_current(false);
                }
                KeyCode::Enter => {
                    if self.q_custom_active {
                        self.submit_custom();
                    } else {
                        self.answer_current(true);
                    }
                }
                KeyCode::Backspace => {
                    if self.q_custom_active {
                        self.q_custom.pop();
                    } else {
                        self.q_cursor = 0;
                    }
                }
                KeyCode::Esc => {
                    if self.q_custom_active {
                        self.q_custom_active = false;
                        self.q_custom.clear();
                    } else {
                        self.resolve_question(
                            vioraharness_core::permissions::QuestionResult::Cancelled,
                        );
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(req) = &self.pending_q {
                        if req.questions.is_empty() {
                            return;
                        }
                        let qi = self
                            .q_answered
                            .len()
                            .min(req.questions.len().saturating_sub(1));
                        self.q_cursor = req.questions[qi].options.len();
                    }
                    self.q_custom_active = true;
                    self.q_custom.push(c);
                }
                _ => {}
            },
            Popup::ToolOutput => match key.code {
                KeyCode::Up => {
                    self.tool_output_scroll = self.tool_output_scroll.saturating_sub(1);
                }
                KeyCode::Down => {
                    if let Some((_, _, out, _)) = &self.last_tool_output {
                        let total = out.lines().count();
                        if self.tool_output_scroll + 1 < total {
                            self.tool_output_scroll += 1;
                        }
                    }
                }
                KeyCode::PageUp => {
                    let page = popup_list_visible(5);
                    self.tool_output_scroll = self.tool_output_scroll.saturating_sub(page);
                }
                KeyCode::PageDown => {
                    if let Some((_, _, out, _)) = &self.last_tool_output {
                        let total = out.lines().count();
                        let page = popup_list_visible(5);
                        self.tool_output_scroll =
                            (self.tool_output_scroll + page).min(total.saturating_sub(1));
                    }
                }
                KeyCode::Home => self.tool_output_scroll = 0,
                KeyCode::End => {
                    if let Some((_, _, out, _)) = &self.last_tool_output {
                        let total = out.lines().count();
                        let page = popup_list_visible(5);

                        self.tool_output_scroll =
                            total.saturating_sub(page).min(total.saturating_sub(1));
                    }
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if let Some((id, name, out, _)) = &self.last_tool_output {
                        let path = format!(
                            "/tmp/vioraharness_tool_{}_{}.txt",
                            name,
                            &id[..8.min(id.len())]
                        );
                        let _ = std::fs::write(&path, out);
                        self.messages.push(Msg::new(
                            "system",
                            format!("Saved full output → {} ({} chars)", path, out.len()),
                        ));
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = Popup::None,
                _ => {}
            },
            _ => {
                self.popup = Popup::None;
            }
        }
    }

    pub(crate) fn resolve_perm(
        &mut self,
        decision: vioraharness_core::permissions::InteractiveDecision,
    ) {
        if let Some(req) = self.pending_perm.take() {
            let tool = req.tool.clone();
            let args = req.args.clone();
            let _ = req.tx.send(decision.clone());
            let msg = match decision {
                vioraharness_core::permissions::InteractiveDecision::AllowOnce => {
                    format!(
                        "✓ Allowed once: {tool} — {}",
                        pretty_tool_args(&tool, &args)
                    )
                }
                vioraharness_core::permissions::InteractiveDecision::AllowAlways => {
                    format!("✓ Allowed always: {tool} — saved to vioraharness.json")
                }
                vioraharness_core::permissions::InteractiveDecision::Deny => {
                    format!("✗ Denied: {tool} — {}", pretty_tool_args(&tool, &args))
                }
            };
            self.messages.push(Msg::new("system", msg));
            self.status = "ready".into();
            self.popup = Popup::None;
            self.perm_cursor = 0;
        } else {
            self.popup = Popup::None;
        }
    }

    pub(crate) fn current_question(&self) -> Option<vioraharness_core::permissions::QuestionItem> {
        let req = self.pending_q.as_ref()?;
        if req.questions.is_empty() {
            return None;
        }
        let qi = self.q_answered.len().min(req.questions.len() - 1);
        Some(req.questions[qi].clone())
    }

    pub(crate) fn push_answer(&mut self, answer: vioraharness_core::permissions::QuestionAnswer) {
        self.q_answered.push(answer);
        self.q_cursor = 0;
        self.q_toggled.clear();
        self.q_custom.clear();
        self.q_custom_active = false;
        let done = self
            .pending_q
            .as_ref()
            .map(|r| self.q_answered.len() >= r.questions.len())
            .unwrap_or(true);
        if done {
            let answers = std::mem::take(&mut self.q_answered);
            self.resolve_question(vioraharness_core::permissions::QuestionResult::Answered(
                answers,
            ));
        }
    }

    pub(crate) fn answer_current(&mut self, confirm: bool) {
        let q = match self.current_question() {
            Some(q) => q,
            None => return,
        };
        if self.q_cursor >= q.options.len() {
            self.q_custom_active = true;
            if !confirm {
                self.q_custom.push(' ');
            }
            return;
        }
        if q.multi_select {
            if !confirm {
                if self.q_toggled.contains(&self.q_cursor) {
                    self.q_toggled.retain(|&i| i != self.q_cursor);
                } else {
                    self.q_toggled.push(self.q_cursor);
                }
            } else if !self.q_toggled.is_empty() {
                let mut idx = self.q_toggled.clone();
                idx.sort_unstable();
                let selected = idx
                    .iter()
                    .map(|&i| q.options[i].label.clone())
                    .collect::<Vec<_>>();
                self.push_answer(vioraharness_core::permissions::QuestionAnswer {
                    question: q.question.clone(),
                    selected,
                    custom: None,
                });
            }
        } else {
            let label = q.options[self.q_cursor].label.clone();
            self.push_answer(vioraharness_core::permissions::QuestionAnswer {
                question: q.question.clone(),
                selected: vec![label],
                custom: None,
            });
        }
    }

    pub(crate) fn submit_custom(&mut self) {
        let q = match self.current_question() {
            Some(q) => q,
            None => return,
        };
        let text = self.q_custom.trim().to_string();
        if text.is_empty() {
            return;
        }
        let mut selected: Vec<String> = {
            let mut idx = self.q_toggled.clone();
            idx.sort_unstable();
            idx.iter().map(|&i| q.options[i].label.clone()).collect()
        };

        if q.multi_select {
            selected.push(format!("custom: {text}"));
        }
        self.push_answer(vioraharness_core::permissions::QuestionAnswer {
            question: q.question.clone(),
            selected,
            custom: Some(text),
        });
    }

    pub(crate) fn resolve_question(
        &mut self,
        result: vioraharness_core::permissions::QuestionResult,
    ) {
        if let Some(req) = self.pending_q.take() {
            let n = req.questions.len();
            let _ = req.tx.send(result.clone());
            let msg = match &result {
                vioraharness_core::permissions::QuestionResult::Answered(answers) => {
                    let parts: Vec<String> = answers
                        .iter()
                        .map(|a| {
                            let mut picks = a.selected.join(", ");
                            if let Some(c) = &a.custom {
                                if picks.is_empty() {
                                    picks = format!("“{c}”");
                                } else {
                                    picks = format!("{picks} + “{c}”");
                                }
                            }
                            format!(
                                "{} → {picks}",
                                a.question.chars().take(60).collect::<String>()
                            )
                        })
                        .collect();
                    format!("✓ Answered {}/{}: {}", answers.len(), n, parts.join(" · "))
                }
                vioraharness_core::permissions::QuestionResult::Cancelled => {
                    "✗ Question cancelled — model proceeds on best judgment".to_string()
                }
            };
            self.messages.push(Msg::new("system", msg));
            self.status = "ready".into();
        }
        self.popup = Popup::None;
        self.q_cursor = 0;
        self.q_answered.clear();
        self.q_toggled.clear();
        self.q_custom.clear();
        self.q_custom_active = false;
    }
}
/// One rewind checkpoint = one user turn: the user's message plus every
/// model/tool row that followed it, up to (not including) the next user
/// message. `target_seq` is the seq to rewind to (drops everything after).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RewindCheckpoint {
    pub user_seq: i64,
    pub target_seq: i64,
    pub preview: String,
    pub snap_count: usize,
}

/// Rewind checkpoints for a session, oldest first. Read fresh from the
/// store on every call so the dialog never shows a stale conversation.
pub(crate) fn rewind_checkpoints(session_id: &str) -> Vec<RewindCheckpoint> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = match vioraharness_core::session::SessionStore::new(&db) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = store.get_messages_detailed(session_id).unwrap_or_default();
    let mut snap_counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    if let Ok(snaps) = store.get_snapshots(session_id) {
        for (seq, _, _) in snaps {
            *snap_counts.entry(seq).or_default() += 1;
        }
    }
    let mut points: Vec<RewindCheckpoint> = Vec::new();
    for m in &rows {
        let is_turn_start = m.role == "user" && !m.is_compaction;
        if is_turn_start {
            if let Some(prev) = points.last_mut() {
                prev.target_seq = m.seq - 1;
            }
            let first = m.content.lines().next().unwrap_or("").trim();
            let preview = if first.chars().count() > 64 {
                format!("{}…", first.chars().take(64).collect::<String>())
            } else if first.is_empty() {
                "(empty)".to_string()
            } else {
                first.to_string()
            };
            points.push(RewindCheckpoint {
                user_seq: m.seq,
                target_seq: m.seq,
                preview,
                snap_count: 0,
            });
        } else if let Some(cur) = points.last_mut() {
            cur.target_seq = m.seq;
        } else {
            // Leading non-user rows (e.g. a compaction summary with no
            // user message yet): anchor one checkpoint on them.
            points.push(RewindCheckpoint {
                user_seq: m.seq,
                target_seq: m.seq,
                preview: "(history)".to_string(),
                snap_count: 0,
            });
        }
    }
    let mut prev_target = 0;
    for p in &mut points {
        p.snap_count = snap_counts
            .iter()
            .filter(|(seq, _)| **seq > prev_target && **seq <= p.target_seq)
            .map(|(_, n)| n)
            .sum();
        prev_target = p.target_seq;
    }
    points
}

impl App {
    /// First Enter on a checkpoint: preview consequences and arm — or refuse
    /// outright when rewinding would change nothing (e.g. latest checkpoint).
    pub(crate) fn arm_rewind_checkpoint(&mut self, p: &RewindCheckpoint) {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        let preview = vioraharness_core::session::SessionStore::new(&db)
            .ok()
            .and_then(|store| {
                vioraharness_core::session::snapshot::preview_rewind(
                    &store,
                    &self.session_id,
                    p.target_seq,
                )
                .ok()
            });
        let note = preview.map(|r| {
            let mut parts = vec![format!(
                "drop {} message{}",
                r.drop_messages,
                if r.drop_messages == 1 { "" } else { "s" }
            )];
            if r.restore_files > 0 {
                parts.push(format!(
                    "restore {} file{}",
                    r.restore_files,
                    if r.restore_files == 1 { "" } else { "s" }
                ));
            }
            if r.prune_snapshots > 0 {
                parts.push(format!(
                    "prune {} snapshot{}",
                    r.prune_snapshots,
                    if r.prune_snapshots == 1 { "" } else { "s" }
                ));
            }
            (r.is_noop(), parts.join(", "))
        });
        match note {
            Some((true, _)) => {
                self.rewind_armed = None;
                self.rewind_armed_note = None;
                self.status = format!(
                    "checkpoint #{} is already latest — nothing to rewind",
                    p.user_seq
                );
            }
            Some((false, detail)) => {
                self.rewind_armed = Some(p.target_seq);
                self.rewind_armed_note = Some(detail.clone());
                self.status = format!(
                    "rewind armed at #{} ({detail}) — Enter again to restore, Esc cancels",
                    p.user_seq
                );
            }
            None => {
                self.rewind_armed = Some(p.target_seq);
                self.rewind_armed_note = None;
                self.status = format!(
                    "rewind armed at #{} — Enter again to restore, Esc cancels",
                    p.user_seq
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;
    #[test]
    fn picker_page_step_matches_visible_window() {
        let mut app = test_app();
        app.available_models = (0..50).map(|i| format!("openai/model-{i:02}")).collect();
        app.popup = Popup::ModelPicker;
        let page = popup_list_visible(7);
        assert!(page >= 3, "page step sane, got {page}");
        app.handle_popup_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()));
        assert_eq!(app.model_cursor, page);
        app.handle_popup_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()));
        assert_eq!(app.model_cursor, page * 2);
        app.handle_popup_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()));
        assert_eq!(app.model_cursor, page);
    }

    #[test]
    fn sessions_scope_toggle_flips_project_filter() {
        let mut app = test_app();
        app.popup = Popup::Sessions;
        assert!(!app.show_all_sessions);
        assert!(
            app.session_scope_filter().is_some(),
            "project-scoped by default"
        );
        app.handle_popup_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()));
        assert!(app.show_all_sessions, "p shows all folders");
        assert!(app.session_scope_filter().is_none());
        assert!(app.popup == Popup::Sessions, "dialog stays open");
        app.handle_popup_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::empty()));
        assert!(!app.show_all_sessions, "P scopes back to this folder");
    }

    #[test]
    fn sessions_popup_pages_and_clamps_cursor() {
        let mut app = test_app();
        app.popup = Popup::Sessions;

        app.available_models = vec![];
        app.handle_popup_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        app.handle_popup_key(KeyEvent::new(KeyCode::End, KeyModifiers::empty()));
        let text = render_text(&mut app, 80, 24);
        assert!(
            text.contains("Sessions") || text.contains("No chats"),
            "sessions popup renders"
        );
    }

    #[test]
    fn question_single_then_multi_answers() {
        let mut app = test_app();
        let (req, mut rx) = pending_question();
        app.pending_q = Some(req);
        app.popup = Popup::Question;

        app.handle_popup_key(key(KeyCode::Down));
        assert_eq!(app.q_cursor, 1);
        app.handle_popup_key(key(KeyCode::Enter));
        assert_eq!(app.q_answered.len(), 1);
        assert_eq!(app.q_answered[0].selected, vec!["B".to_string()]);
        assert_eq!(app.popup, Popup::Question);

        app.handle_popup_key(key(KeyCode::Char(' ')));
        app.handle_popup_key(key(KeyCode::Down));
        app.handle_popup_key(key(KeyCode::Char(' ')));
        app.handle_popup_key(key(KeyCode::Enter));
        assert_eq!(app.popup, Popup::None);
        match rx.try_recv() {
            Ok(vioraharness_core::permissions::QuestionResult::Answered(a)) => {
                assert_eq!(a.len(), 2);
                assert_eq!(a[1].selected, vec!["X".to_string(), "Y".to_string()]);
            }
            other => panic!("expected answers, got {other:?}"),
        }
        assert!(app
            .messages
            .iter()
            .any(|m| m.content.contains("Answered 2/2")));
    }

    #[test]
    fn question_custom_answer_and_cancel() {
        let mut app = test_app();
        let (req, mut rx) = pending_question();
        app.pending_q = Some(req);
        app.popup = Popup::Question;
        app.handle_popup_key(key(KeyCode::End));
        app.handle_popup_key(key(KeyCode::Enter));
        for c in "maybe".chars() {
            app.handle_popup_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()));
        }
        app.handle_popup_key(key(KeyCode::Enter));

        match rx.try_recv() {
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            other => panic!("expected no answer yet, got {other:?}"),
        }
        assert_eq!(app.q_answered.len(), 1);
        assert_eq!(app.q_answered[0].custom.as_deref(), Some("maybe"));

        app.handle_popup_key(key(KeyCode::Esc));
        assert_eq!(app.popup, Popup::None);

        let mut app2 = test_app();
        let (req2, mut rx2) = pending_question();
        app2.pending_q = Some(req2);
        app2.popup = Popup::Question;
        app2.handle_popup_key(key(KeyCode::Esc));
        match rx2.try_recv() {
            Ok(vioraharness_core::permissions::QuestionResult::Cancelled) => {}
            other => panic!("expected cancel, got {other:?}"),
        }
    }

    #[test]
    fn errors_panel_lists_and_opens_full_text() {
        use vioraharness_core::observe;
        let _reg_guard = ERR_REG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let id = observe::push_error("test-src-xyz", "boom detail line one\nline two here");
        let mut app = test_app();
        app.handle_slash("/errors");
        let text = render_text(&mut app, 120, 40);
        assert!(text.contains("Errors"), "panel title");
        assert!(text.contains("boom detail line one"), "summary shown");
        assert!(text.contains("test-src"), "source shown");
        assert!(text.contains("✖"), "header badge");
        // Cursor onto our entry, Enter loads the full text viewer.
        app.error_cursor = observe::list_errors()
            .iter()
            .position(|e| e.id == id)
            .expect("own error listed");
        app.handle_popup_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.popup, Popup::ToolOutput);
        let (_, name, out, ok) = app
            .last_tool_output
            .as_ref()
            .expect("viewer loaded")
            .clone();
        assert!(!ok, "error viewer marks failure");
        assert!(out.contains("line two here"), "full text, not just summary");
        assert!(name.contains(&short_err_id(&id)), "short id in title");
        // `c` clears via status (global count asserted only in core test).
        app.popup = Popup::Errors;
        app.handle_popup_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty()));
        assert_eq!(app.status, "errors cleared");
    }
}
