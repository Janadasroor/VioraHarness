use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// Double-press confirmation slot: quitting the TUI vs cancelling the
/// running turn. One shared slot — arming one clears the other, so a
/// stale arm can never fire in the wrong context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfirmKind {
    Quit,
    CancelTurn,
}

impl App {
    /// Window for double-press confirms: first press arms (status hint),
    /// second press inside this window executes.
    const CONFIRM_SECS: u64 = 3;

    /// True while a quit is armed and the window hasn't lapsed.
    pub(crate) fn quit_armed(&self) -> bool {
        self.confirm_state(ConfirmKind::Quit)
    }

    /// True while a turn-cancel is armed and the window hasn't lapsed.
    pub(crate) fn cancel_armed(&self) -> bool {
        self.confirm_state(ConfirmKind::CancelTurn)
    }

    fn confirm_state(&self, kind: ConfirmKind) -> bool {
        matches!(self.confirm_armed, Some((k, t)) if k == kind && t.elapsed() < std::time::Duration::from_secs(Self::CONFIRM_SECS))
    }

    fn arm_confirm(&mut self, kind: ConfirmKind) {
        self.confirm_armed = Some((kind, std::time::Instant::now()));
        self.status = match kind {
            ConfirmKind::Quit => "press Esc or Ctrl+C again to quit".into(),
            ConfirmKind::CancelTurn => "press Esc again to cancel the turn".into(),
        };
    }

    pub(crate) fn disarm_quit(&mut self) {
        self.confirm_armed = None;
    }

    /// Shared double-press quit. First idle press arms (status hint),
    /// second press inside the window returns true (caller quits).
    /// Any other key disarms via the callers below.
    pub(crate) fn confirm_quit(&mut self) -> bool {
        if self.quit_armed() {
            self.confirm_armed = None;
            return true;
        }
        self.arm_confirm(ConfirmKind::Quit);
        false
    }

    /// OS-signal shutdown check, polled by the event loop. A real SIGINT
    /// bypasses the key handling above (raw mode turns ^C into a key
    /// event, but a signal from another session — or after raw mode is
    /// off — kills the process mid-alternate-screen by default). On the
    /// flag, mark a clean quit so the normal restore path runs. Returns
    /// true when the caller must break out now. Separated for tests.
    pub(crate) fn check_sigint(
        &mut self,
        sigint: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> bool {
        if sigint.load(std::sync::atomic::Ordering::SeqCst) {
            self.confirm_armed = None;
            self.status = "interrupted — shutting down".into();
            self.should_quit = true;
            return true;
        }
        false
    }

    /// Bare Ctrl+C (no Ctrl+Alt): copy selection when there is one,
    /// otherwise the double-press quit path. Returns true when the caller
    /// must quit now. Extracted so the event loop and tests share it.
    pub(crate) fn handle_ctrl_c(&mut self) -> bool {
        if self.input.selected_range().is_some() {
            self.copy_input_selection();
            self.disarm_quit();
            return false;
        }
        if self.selection.is_some() {
            self.copy_pending = true;
            self.disarm_quit();
            return false;
        }
        if self.confirm_quit() {
            self.selection = None;
            return true;
        }
        false
    }

    /// Test + compat entry: plain key without modifiers.
    #[cfg(test)]
    pub(crate) async fn handle_key(&mut self, code: KeyCode) -> anyhow::Result<()> {
        self.handle_key_event(KeyEvent::new(code, KeyModifiers::empty()))
            .await
    }

    pub(crate) async fn handle_key_event(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        let code = key.code;
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        // Any key but Esc cancels a pending double-press (quit or
        // cancel) — Esc reaches its own arm below, where only a fully
        // idle press counts toward quitting.
        if code != KeyCode::Esc {
            self.disarm_quit();
        }
        if self.busy
            && !matches!(
                code,
                KeyCode::Esc
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Char(_)
                    | KeyCode::Tab
                    | KeyCode::Backspace
                    | KeyCode::Delete
                    | KeyCode::Enter
            )
        {
            return Ok(());
        }
        match code {
            KeyCode::Char(c) => {
                // Subagent view owns no input box: swallow edits quietly
                // (submit is refused too) instead of filling it invisibly.
                // Holds after finish — only the main agent takes input.
                if self.viewing_subagent() {
                    self.status = "read-only: subagent view (Esc back to main)".into();
                    return Ok(());
                }
                if c == '\t' {
                    if self.input.text.starts_with('/') {
                        let comps = self.input.slash_completions();
                        if !comps.is_empty() {
                            let idx = self.input.completion_idx % comps.len();
                            let chosen = comps[idx].0;
                            self.input.apply_completion(chosen);
                            self.input.completion_idx = (idx + 1) % comps.len();
                        }
                        return Ok(());
                    }
                    let is_plan = self.mode == "plan";
                    self.mode = if is_plan {
                        "build".into()
                    } else {
                        "plan".into()
                    };
                    return Ok(());
                }
                self.input.insert(c);
                self.input.completion_idx = 0;
            }
            KeyCode::Tab => {
                if self.viewing_subagent() {
                    self.status = "read-only: subagent view (Esc back to main)".into();
                    return Ok(());
                }
                if self.input.text.starts_with('/') {
                    let comps = self.input.slash_completions();
                    if !comps.is_empty() {
                        let idx = self.input.completion_idx % comps.len();
                        let chosen = comps[idx].0;
                        self.input.apply_completion(chosen);
                        self.input.completion_idx = (idx + 1) % comps.len();
                    }
                } else {
                    let is_plan = self.mode == "plan";
                    self.mode = if is_plan {
                        "build".into()
                    } else {
                        "plan".into()
                    };
                }
            }
            KeyCode::Backspace => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.backspace();
                self.input.completion_idx = 0;
            }
            KeyCode::Delete => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.delete();
                self.input.completion_idx = 0;
            }
            KeyCode::Left => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.move_left(shift)
            }
            KeyCode::Right => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.move_right(shift)
            }
            KeyCode::Home => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.move_to_start(shift)
            }
            KeyCode::End => {
                if self.viewing_subagent() {
                    return Ok(());
                }
                self.input.move_to_end(shift)
            }
            KeyCode::Up => {
                // No input box in subagent view: arrows scroll the
                // transcript instead of touching history.
                if self.viewing_subagent() {
                    self.scroll = self.scroll.saturating_add(1);
                    return Ok(());
                }
                if self.busy {
                    self.scroll = self.scroll.saturating_add(1);
                } else {
                    let comps = self.input.slash_completions();
                    if !comps.is_empty() && self.input.text.starts_with('/') {
                        if self.input.completion_idx == 0 {
                            self.input.completion_idx = comps.len() - 1;
                        } else {
                            self.input.completion_idx -= 1;
                        }
                        let chosen = comps[self.input.completion_idx % comps.len()].0;
                        self.input.apply_completion(chosen);
                    } else {
                        self.input.hist_prev();
                    }
                }
            }
            KeyCode::Down => {
                if self.viewing_subagent() {
                    if self.scroll > 0 {
                        self.scroll -= 1;
                    }
                    return Ok(());
                }
                if self.busy {
                    if self.scroll > 0 {
                        self.scroll -= 1;
                    }
                } else {
                    let comps = self.input.slash_completions();
                    if !comps.is_empty() && self.input.text.starts_with('/') {
                        self.input.completion_idx = (self.input.completion_idx + 1) % comps.len();
                        let chosen = comps[self.input.completion_idx].0;
                        self.input.apply_completion(chosen);
                    } else {
                        self.input.hist_next();
                    }
                }
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_sub(10);
            }
            KeyCode::Esc => {
                self.dragging = false;
                self.input_drag = false;
                if self.input.selected_range().is_some() {
                    self.input.clear_selection();
                    return Ok(());
                }
                if self.selection.take().is_some() {
                    return Ok(());
                }
                // Esc from a subagent transcript returns to main chat.
                if self
                    .session_id
                    .starts_with(vioraharness_core::subagent::tracker::SUB_SESSION_PREFIX)
                {
                    if let Some(prev) = self.return_session.clone() {
                        self.return_session = None;
                        self.resume_chat(&prev);
                        return Ok(());
                    }
                    // No way back stored (e.g. landed here without /agents):
                    // open the sessions picker so there is always a way out.
                    self.popup = Popup::Sessions;
                    self.status = "subagent view is read-only — pick a main chat".into();
                    return Ok(());
                }
                if self.pending_image.is_some() || !self.pending_texts.is_empty() {
                    let chips: Vec<String> = self
                        .pending_texts
                        .iter()
                        .map(|(id, text)| paste_chip(*id, text.lines().count()))
                        .collect();
                    let img_chip = self
                        .pending_image
                        .as_ref()
                        .map(|img| image_chip(&img.label));
                    for chip in chips.iter().chain(img_chip.iter()) {
                        self.input.text = self.input.text.replace(chip, "");
                    }
                    self.input.text = self.input.text.replace("  ", " ");
                    self.input.cursor = self.input.cursor.min(self.input.text.len());
                    while !self.input.text.is_char_boundary(self.input.cursor)
                        && self.input.cursor > 0
                    {
                        self.input.cursor -= 1;
                    }
                    self.pending_image = None;
                    self.pending_texts.clear();
                    self.messages
                        .push(Msg::new("system", "Cleared pending attachments (Esc)"));
                } else if !self.input.text.is_empty() {
                    self.input.text.clear();
                    self.input.cursor = 0;
                } else if self.busy {
                    if !self.cancel_armed() {
                        // Busy Esc needs confirming too: first press arms,
                        // second cancels the turn (re-arming clears a stale
                        // quit arm — contexts never mix).
                        self.arm_confirm(ConfirmKind::CancelTurn);
                        return Ok(());
                    }
                    self.confirm_armed = None;
                    if let Some(h) = self.pending.take() {
                        h.abort();
                    }
                    self.busy = false;
                    self.status = "cancelled".into();
                    self.messages.push(Msg::new("system", "cancelled"));

                    if !self.queued_prompts.is_empty() {
                        let n = self.queued_prompts.len();
                        self.queued_prompts.clear();
                        self.messages.push(Msg::new(
                            "system",
                            format!(
                                "dropped {n} queued prompt{} (turn cancelled)",
                                if n == 1 { "" } else { "s" }
                            ),
                        ));
                    }
                    // `$` instant prompts already echoed into chat: never
                    // orphan them — promote leftovers to the queue head
                    // (no re-echo) instead of dropping.
                    let kept = self.promote_instant_leftovers();
                    if kept > 0 {
                        self.messages.push(Msg::new(
                            "system",
                            format!(
                                "kept {kept} ⚡ prompt{} for next turn (turn cancelled)",
                                if kept == 1 { "" } else { "s" }
                            ),
                        ));
                    }
                } else if self.popup != Popup::None {
                    self.popup = Popup::None;
                } else if self.confirm_quit() {
                    // Fully idle Esc: first press arms, second quits.
                    self.should_quit = true;
                }
            }
            KeyCode::Enter => {
                // No input box in subagent view: nothing submits from here.
                if self.viewing_subagent() {
                    self.status = "read-only: subagent view (Esc back to main)".into();
                    return Ok(());
                }
                if self.input.text.starts_with('/') {
                    let comps = self.input.slash_completions();
                    if !comps.is_empty() {
                        let base = self
                            .input
                            .text
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();

                        let exact = comps.iter().any(|(c, _)| *c == base);
                        if !exact && comps.len() == 1 {
                            self.input.apply_completion(comps[0].0);

                            if !self.busy || Self::is_busy_safe_slash(&self.input.text) {
                                let new_prompt = self.input.text.trim().to_string();
                                self.input.push_history(new_prompt.clone());
                                self.handle_slash(&new_prompt);
                                self.input.text.clear();
                                self.input.cursor = 0;
                                return Ok(());
                            }
                            return Ok(());
                        }

                        if !exact && comps.len() > 1 {
                            let idx = self.input.completion_idx % comps.len();
                            self.input.apply_completion(comps[idx].0);

                            return Ok(());
                        }
                    }
                }
                let prompt = self.input.text.trim().to_string();
                if prompt.is_empty() {
                    return Ok(());
                }

                // `$` instant prompt: bypasses the queue into the live turn.
                // No slash dispatch after `$` — the remainder is prompt text.
                if prompt.starts_with('$') {
                    if self.busy {
                        self.submit_instant(prompt);
                        return Ok(());
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
                        return Ok(());
                    }
                    return self.submit_text(stripped);
                }

                if prompt.starts_with('/') && Self::is_known_slash(&prompt) {
                    if self.busy && !Self::is_busy_safe_slash(&prompt) {
                        // Deferred: runs in order once the turn finishes,
                        // exactly as if typed while idle.
                        let base = prompt.split_whitespace().next().unwrap_or("").to_string();
                        self.input.push_history(prompt.clone());
                        self.queued_prompts.push(QueuedPrompt {
                            session_id: self.session_id.clone(),
                            send: prompt,
                            image: None,
                            slash: true,
                            echo: false,
                        });
                        self.input.text.clear();
                        self.input.cursor = 0;
                        let n = self.queued_prompts.len();
                        self.status = if n == 1 {
                            format!("{base} queued — runs when the turn finishes")
                        } else {
                            format!("{base} queued ({n} waiting) — runs in order when idle")
                        };
                        return Ok(());
                    }
                    self.input.push_history(prompt.clone());
                    self.handle_slash(&prompt);
                    self.input.text.clear();
                    self.input.cursor = 0;
                    return Ok(());
                }

                if self.busy {
                    self.queue_prompt(prompt);
                    return Ok(());
                }
                return self.submit_text(prompt);
            }
            KeyCode::F(1) => {
                self.popup = if self.popup == Popup::Help {
                    Popup::None
                } else {
                    Popup::Help
                };
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn session_filtered_len(&self) -> usize {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        vioraharness_core::session::SessionStore::new(&db)
            .and_then(|s| {
                s.list_sessions_filtered(self.session_scope_filter().as_deref(), None, true, 50, 0)
            })
            .map(|all| {
                let f = self.session_filter.to_lowercase();
                if f.is_empty() {
                    all.len()
                } else {
                    all.iter()
                        .filter(|s| {
                            s.id.to_lowercase().contains(&f)
                                || s.title
                                    .as_ref()
                                    .map(|t| t.to_lowercase().contains(&f))
                                    .unwrap_or(false)
                                || s.model.to_lowercase().contains(&f)
                        })
                        .count()
                }
            })
            .unwrap_or(0)
    }

    pub(crate) fn model_filtered_len(&self) -> usize {
        let filter = self.model_filter.to_lowercase();
        if filter.is_empty() {
            self.available_models.len()
        } else {
            self.available_models
                .iter()
                .filter(|m| m.to_lowercase().contains(&filter))
                .count()
        }
    }

    pub(crate) fn chat_inner(&self) -> Option<Rect> {
        if self.chat_area.width <= 2 || self.chat_area.height <= 2 {
            return None;
        }
        Some(Rect {
            x: self.chat_area.x + 1,
            y: self.chat_area.y + 1,
            width: self.chat_area.width - 2,
            height: self.chat_area.height - 2,
        })
    }

    pub(crate) fn screen_to_content(&self, mx: u16, my: u16) -> Option<SelPos> {
        let inner = self.chat_inner()?;
        if self.vis_rows.is_empty() {
            return None;
        }
        let rel_y = my.saturating_sub(inner.y) as usize;
        let visible = (inner.height as usize).max(1);
        let row_in_view = rel_y
            .min(visible.saturating_sub(1))
            .min(self.vis_rows.len().saturating_sub(1));
        let (line, col_start) = self.vis_rows[row_in_view];
        let col = col_start + (mx.saturating_sub(inner.x) as usize).min(10000);
        Some(SelPos { line, col })
    }

    pub(crate) fn in_input(&self, mx: u16, my: u16) -> bool {
        hit_rect(self.input_area, mx, my)
    }

    /// Map a mouse column to an input-text byte index (width-aware, clamped).
    pub(crate) fn input_col_to_idx(&self, mx: u16) -> usize {
        use unicode_width::UnicodeWidthChar;
        let x0 = self.input_area.x.saturating_add(1);
        let max_w = self.input_area.width.saturating_sub(2) as usize;
        let col = (mx.saturating_sub(x0) as usize).min(max_w);
        let mut w = 0;
        for (i, c) in self.input.text.char_indices() {
            let cw = c.width().unwrap_or(0);
            if w + cw > col {
                return i;
            }
            w += cw;
        }
        self.input.text.len()
    }

    pub(crate) fn copy_input_selection(&mut self) {
        if let Some(text) = self.input.selected_text() {
            match clipboard_copy_text(&text) {
                Ok((n, via)) => {
                    self.status =
                        format!("copied {n} chars via {via} — selection kept (Esc clears)");
                }
                Err(e) => {
                    self.status = format!("copy failed ({e}) — selection kept");
                }
            }
        } else {
            self.status = "nothing selected — Shift+←/→ or drag to select first".into();
        }
    }

    pub(crate) fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.popup != Popup::None {
                    return;
                }
                if self.in_input(m.column, m.row) {
                    let idx = self.input_col_to_idx(m.column);
                    self.input.cursor = idx;
                    self.input.sel_anchor = Some(idx);
                    self.input_drag = true;
                    self.dragging = false;
                    self.selection = None;
                    return;
                }
                match self.screen_to_content(m.column, m.row) {
                    Some(pos) => {
                        self.selection = Some(Selection {
                            anchor: pos,
                            cursor: pos,
                            session: self.session_id.clone(),
                            msg_len: self.messages.len(),
                        });
                        self.dragging = true;
                    }
                    None if !self.in_input(m.column, m.row) => {
                        self.selection = None;
                    }
                    None => {}
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.popup != Popup::None {
                    return;
                }
                if self.input_drag {
                    let idx = self.input_col_to_idx(m.column);
                    self.input.cursor = idx;
                    if self.input.sel_anchor == Some(idx) {
                        self.input.sel_anchor = None;
                    }
                    return;
                }
                if !self.dragging {
                    return;
                }

                if let Some(inner) = self.chat_inner() {
                    if m.row <= inner.y + 1 {
                        self.scroll = self.scroll.saturating_add(4);
                    } else if m.row + 2 >= inner.y + inner.height {
                        self.scroll = self.scroll.saturating_sub(4);
                    }
                }
                if let Some(pos) = self.screen_to_content(m.column, m.row) {
                    if let Some(sel) = self.selection.as_mut() {
                        sel.cursor = pos;
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.input_drag {
                    self.input_drag = false;
                    if self.input.selected_range().is_some() {
                        self.copy_input_selection();
                    } else {
                        self.input.clear_selection();
                    }
                    return;
                }
                if !self.dragging {
                    return;
                }
                self.dragging = false;

                match self.selection.take() {
                    Some(sel) if !sel.is_caret() => {
                        self.selection = Some(sel);
                        self.copy_pending = true;
                    }
                    _ => {}
                }
            }
            MouseEventKind::ScrollUp => {
                if self.popup != Popup::None {
                    self.popup_wheel(-3);
                } else {
                    self.scroll = self.scroll.saturating_add(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.popup != Popup::None {
                    self.popup_wheel(3);
                } else {
                    self.scroll = self.scroll.saturating_sub(3);
                }
            }

            MouseEventKind::Down(MouseButton::Middle)
                if self.popup == Popup::None && self.in_input(m.column, m.row) =>
            {
                if let Some(txt) = clipboard_paste_text().filter(|t| !t.trim().is_empty()) {
                    self.insert_pasted_text(&txt);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn popup_wheel(&mut self, dir: i32) {
        match self.popup {
            Popup::Settings => {
                self.settings_move(dir);
            }
            Popup::ModelPicker => {
                let len = self.model_filtered_len();
                if len == 0 {
                    return;
                }
                let cur = self.model_cursor as i32 + dir;
                self.model_cursor = cur.clamp(0, len as i32 - 1) as usize;
            }
            Popup::Sessions => {
                let len = self.session_filtered_len();
                if len == 0 {
                    return;
                }
                let cur = self.session_cursor as i32 + dir;
                self.session_cursor = cur.clamp(0, len as i32 - 1) as usize;
            }
            Popup::Agents => {
                let len = super::agents::agent_rows(self).len();
                if len == 0 {
                    return;
                }
                let cur = self.agent_cursor as i32 + dir;
                self.agent_cursor = cur.clamp(0, len as i32 - 1) as usize;
                self.anchor_agent_cursor();
            }
            Popup::ToolOutput => {
                if dir < 0 {
                    self.tool_output_scroll =
                        self.tool_output_scroll.saturating_sub((-dir) as usize);
                } else if let Some((_, _, out, _)) = &self.last_tool_output {
                    let total = out.lines().count();
                    self.tool_output_scroll =
                        (self.tool_output_scroll + dir as usize).min(total.saturating_sub(1));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;
    #[test]
    fn drag_selects_and_edge_autoscrolls() {
        let mut app = mouse_app();

        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 5, 5));
        assert!(app.dragging);
        assert!(app.selection.as_ref().is_some_and(|s| s.is_caret()));

        app.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 10, 8));
        assert!(app.selection.as_ref().is_some_and(|s| !s.is_caret()));

        let before = app.scroll;
        app.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 5, 0));
        assert!(app.scroll > before);

        app.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 5, 30));
        assert_eq!(app.scroll, before);

        app.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 5, 30));
        assert!(!app.dragging);
        assert!(app.copy_pending);
        assert!(app.selection.is_some());
    }

    #[test]
    fn click_clears_and_popup_ignores_mouse() {
        let mut app = mouse_app();
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 5, 5));

        app.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 5, 5));
        assert!(!app.dragging);
        assert!(app.selection.is_none());
        assert!(!app.copy_pending);

        app.popup = Popup::ModelPicker;
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 5, 5));
        assert!(!app.dragging);
        assert!(app.selection.is_none());
    }

    #[test]
    fn wheel_scrolls_chat_and_pickers() {
        let mut app = mouse_app();
        app.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 5, 5));
        assert_eq!(app.scroll, 0);
        app.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 5, 5));
        assert_eq!(app.scroll, 3);
        app.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 5, 5));
        assert_eq!(app.scroll, 0);

        app.popup = Popup::ModelPicker;
        app.available_models = (0..50).map(|i| format!("m{i:02}")).collect();
        app.model_cursor = 10;
        app.handle_mouse(mouse_event(MouseEventKind::ScrollDown, 5, 5));
        assert_eq!(app.model_cursor, 13);
        app.handle_mouse(mouse_event(MouseEventKind::ScrollUp, 5, 5));
        assert_eq!(app.model_cursor, 10);
    }

    #[test]
    fn wrapped_rows_map_to_content_lines() {
        let mut app = test_app();
        app.chat_area = Rect::new(0, 0, 14, 10);
        app.vis_rows = vec![(0, 0), (0, 8), (1, 0)];

        let a = app.screen_to_content(1, 1).expect("maps");
        assert_eq!((a.line, a.col), (0, 0));

        let b = app.screen_to_content(3, 2).expect("maps");
        assert_eq!((b.line, b.col), (0, 8 + 2));

        let c = app.screen_to_content(1, 3).expect("maps");
        assert_eq!((c.line, c.col), (1, 0));
    }

    #[test]
    fn ctrl_o_toggles_recent_reasoning() {
        let mut app = test_app();
        assert!(!app.toggle_recent_reasoning(), "nothing to toggle");
        let mut m = Msg::new("assistant", "hello");
        m.reasoning = Some("private chain".into());
        app.messages.push(m);
        assert!(app.toggle_recent_reasoning(), "toggles the reasoning block");
        assert!(app.expanded_reasoning.contains(&0), "expanded");
        assert!(app.toggle_recent_reasoning(), "toggles again");
        assert!(!app.expanded_reasoning.contains(&0), "collapsed");
        app.show_thinking = true;
        let text = render_text(&mut app, 80, 30);
        assert!(
            text.contains("Ctrl+O"),
            "hint advertises the working key, not plain o"
        );
    }

    #[test]
    fn ctrl_o_falls_back_to_global_toggle_without_reasoning() {
        // Previously Ctrl+O silently no-op'd when no message carried
        // reasoning (e.g. nothing captured yet) — the reported dead key.
        let mut app = test_app();
        assert!(!app.thinking_expanded);
        app.ctrl_o_expand();
        assert!(app.thinking_expanded, "global live block expands");
        assert!(
            app.status.contains("no saved reasoning"),
            "fallback explained: {}",
            app.status
        );
        app.ctrl_o_expand();
        assert!(!app.thinking_expanded, "flips back");
    }

    #[test]
    fn ctrl_o_prefers_recent_reasoning_with_feedback() {
        let mut app = test_app();
        let mut m = Msg::new("assistant", "hello");
        m.reasoning = Some("private chain".into());
        app.messages.push(m);
        app.ctrl_o_expand();
        assert!(
            app.expanded_reasoning.contains(&0),
            "per-message block wins over the global toggle"
        );
        assert!(
            !app.thinking_expanded,
            "global untouched when a block toggled"
        );
        assert!(!app.status.is_empty(), "visible feedback");
    }

    #[test]
    fn reasoning_delta_accumulates_while_hidden() {
        // Display off must not lose thinking: the finished message and
        // the loop's DB persist both keep it; render stays gated.
        let mut app = test_app();
        app.show_thinking = false;
        App::accumulate_reasoning_delta(
            &mut app.thinking_buf,
            &mut app.status,
            app.show_thinking,
            "abc",
        );
        App::accumulate_reasoning_delta(
            &mut app.thinking_buf,
            &mut app.status,
            app.show_thinking,
            "def",
        );
        assert_eq!(app.thinking_buf, "abcdef", "kept while hidden");
        assert!(
            !app.status.contains("thinking…"),
            "no status spam while hidden: {}",
            app.status
        );
        app.show_thinking = true;
        App::accumulate_reasoning_delta(
            &mut app.thinking_buf,
            &mut app.status,
            app.show_thinking,
            "!",
        );
        assert_eq!(app.thinking_buf, "abcdef!");
        assert!(
            app.status.contains("thinking…"),
            "live counter when shown: {}",
            app.status
        );
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    async fn type_text(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(KeyCode::Char(c)).await.unwrap();
        }
    }

    #[tokio::test]
    async fn shift_arrows_select_input_text() {
        let mut app = test_app();
        type_text(&mut app, "hello").await;
        app.handle_key_event(shift(KeyCode::Left)).await.unwrap();
        app.handle_key_event(shift(KeyCode::Left)).await.unwrap();
        assert_eq!(app.input.selected_text().as_deref(), Some("lo"));
        app.handle_key_event(shift(KeyCode::Home)).await.unwrap();
        assert_eq!(app.input.selected_text().as_deref(), Some("hello"));
        app.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::empty()))
            .await
            .unwrap();
        assert_eq!(app.input.selected_range(), None, "plain End collapses");
    }

    #[tokio::test]
    async fn typing_replaces_keyboard_selection() {
        let mut app = test_app();
        type_text(&mut app, "hello").await;
        app.handle_key_event(shift(KeyCode::Home)).await.unwrap();
        app.handle_key(KeyCode::Char('X')).await.unwrap();
        assert_eq!(app.input.text, "X");
        assert_eq!(app.input.selected_range(), None);
    }

    #[tokio::test]
    async fn esc_clears_input_selection_first() {
        let mut app = test_app();
        type_text(&mut app, "hello").await;
        app.handle_key_event(shift(KeyCode::Left)).await.unwrap();
        assert!(app.input.selected_range().is_some());
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert_eq!(app.input.selected_range(), None);
        assert_eq!(app.input.text, "hello", "text kept, only selection cleared");
    }

    #[test]
    fn mouse_drag_selects_inside_input() {
        let mut app = mouse_app();
        app.input_area = Rect::new(0, 26, 100, 3);
        app.input.text = "hello world".into();
        app.input.cursor = 11;
        // Down at column for byte index 6 ('w'), drag back to 0.
        app.handle_mouse(mouse_event(MouseEventKind::Down(MouseButton::Left), 7, 27));
        assert_eq!(app.input.cursor, 6);
        assert!(app.input_drag);
        app.handle_mouse(mouse_event(MouseEventKind::Drag(MouseButton::Left), 1, 27));
        assert_eq!(
            app.input.selected_text().as_deref(),
            Some("hello "),
            "drag extends the selection"
        );
        app.handle_mouse(mouse_event(MouseEventKind::Up(MouseButton::Left), 1, 27));
        assert!(!app.input_drag);
        assert!(
            app.status.contains("copi") || app.status.contains("copy failed"),
            "release copies (or reports backend failure): {}",
            app.status
        );
    }

    #[test]
    fn copy_input_selection_reports_result() {
        let mut app = test_app();
        app.input.text = "copy me".into();
        app.input.cursor = 7;
        app.input.sel_anchor = Some(0);
        app.copy_input_selection();
        assert!(
            app.status.contains("copied") || app.status.contains("copy failed"),
            "status reports the outcome: {}",
            app.status
        );
        let mut empty = test_app();
        empty.copy_input_selection();
        assert!(
            empty.status.contains("nothing selected"),
            "empty selection guided: {}",
            empty.status
        );
    }

    #[test]
    fn input_col_mapping_clamps_and_counts_wide_chars() {
        let mut app = test_app();
        app.input_area = Rect::new(0, 0, 100, 3);
        app.input.text = "aｱb".into();
        assert_eq!(app.input_col_to_idx(1), 0);
        assert_eq!(app.input_col_to_idx(2), 1, "after 'a'");
        assert_eq!(app.input_col_to_idx(3), 4, "wide char occupies two columns");
        assert_eq!(app.input_col_to_idx(200), 5, "clamped to end");
    }

    #[test]
    fn test_app_is_memory_only() {
        let app = test_app();
        assert!(!app.input.persist, "unit tests never touch disk history");
        assert!(
            app.input.history.is_empty(),
            "no user history leaks into tests"
        );
        assert!(app.input.history_file.is_none());
    }

    #[tokio::test]
    async fn slash_enter_records_history() {
        let mut app = test_app();
        app.input.text = "/model".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(
            app.input.history.last().map(String::as_str),
            Some("/model"),
            "slash kept for arrow-up: {:?}",
            app.input.history
        );
    }

    #[tokio::test]
    async fn deferred_slash_records_history_once() {
        let mut app = test_app();
        app.busy = true;
        app.input.text = "/new".into();
        app.input.cursor = 4;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(
            app.input.history.iter().filter(|h| *h == "/new").count(),
            1,
            "queued once: {:?}",
            app.input.history
        );
        // Draining a deferred slash must not push it a second time.
        app.busy = false;
        app.drain_queue();
        assert_eq!(
            app.input.history.iter().filter(|h| *h == "/new").count(),
            1,
            "no double on drain: {:?}",
            app.input.history
        );
    }

    #[tokio::test]
    async fn idle_esc_arms_first_second_quits() {
        let mut app = test_app();
        assert!(!app.should_quit);
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.should_quit, "first idle Esc only arms");
        assert!(app.quit_armed(), "armed");
        assert!(app.status.contains("again to quit"), "{}", app.status);
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.should_quit, "second Esc quits");
    }

    #[tokio::test]
    async fn other_keys_disarm_pending_quit() {
        let mut app = test_app();
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.quit_armed());
        app.handle_key(KeyCode::Char('x')).await.unwrap();
        assert!(!app.quit_armed(), "typing cancels the arm");
        assert_eq!(app.input.text, "x");
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.should_quit, "re-arms instead of quitting");
        assert!(app.input.text.is_empty(), "idle Esc still clears input");
    }

    #[test]
    fn bare_ctrl_c_arms_first_second_quits() {
        let mut app = test_app();
        assert!(!app.handle_ctrl_c(), "first press arms");
        assert!(app.quit_armed());
        assert!(app.handle_ctrl_c(), "second press quits");
    }

    #[test]
    fn ctrl_c_with_selection_copies_and_disarms() {
        let mut app = test_app();
        assert!(!app.handle_ctrl_c(), "armed");
        app.input.text = "hello".into();
        app.input.cursor = 5;
        app.input.sel_anchor = Some(0);
        assert!(app.input.selected_range().is_some());
        assert!(!app.handle_ctrl_c(), "copies instead of quitting");
        assert!(!app.quit_armed(), "copying disarms");
        assert!(!app.should_quit);
    }

    #[test]
    fn sigint_flag_shuts_down_cleanly() {
        let mut app = test_app();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(!app.check_sigint(&flag), "unset flag is a no-op");
        assert!(!app.should_quit);
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(app.check_sigint(&flag), "set flag breaks the loop");
        assert!(app.should_quit, "clean quit flag set");
        assert!(app.status.contains("shutting down"), "{}", app.status);
    }

    #[tokio::test]
    async fn busy_esc_arms_first_second_cancels() {
        let mut app = test_app();
        app.busy = true;
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.busy, "first Esc only arms");
        assert!(app.cancel_armed(), "cancel armed");
        assert!(!app.quit_armed(), "quit arm untouched");
        assert!(app.status.contains("again to cancel"), "{}", app.status);
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.busy, "second Esc cancels the turn");
        assert!(
            app.messages.iter().any(|m| m.content.contains("cancelled")),
            "cancel noted"
        );
    }

    #[tokio::test]
    async fn busy_esc_arm_dies_on_other_keys_and_time() {
        let mut app = test_app();
        app.busy = true;
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.cancel_armed());
        app.handle_key(KeyCode::Char('x')).await.unwrap();
        assert!(!app.cancel_armed(), "typing disarms");
        assert!(app.busy, "turn survives a single Esc");
        // A lapsed arm counts as no arm: re-arms instead of cancelling.
        app.input.text.clear();
        app.input.cursor = 0;
        app.confirm_armed = Some((
            ConfirmKind::CancelTurn,
            std::time::Instant::now() - std::time::Duration::from_secs(30),
        ));
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(app.busy, "stale arm re-arms");
        assert!(app.cancel_armed(), "fresh arm");
    }
}
