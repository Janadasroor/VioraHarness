use super::*;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};

impl App {
    pub(crate) async fn handle_key(&mut self, code: KeyCode) -> anyhow::Result<()> {
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
                self.input.backspace();
                self.input.completion_idx = 0;
            }
            KeyCode::Delete => {
                self.input.delete();
                self.input.completion_idx = 0;
            }
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.cursor = 0,
            KeyCode::End => self.input.cursor = self.input.text.len(),
            KeyCode::Up => {
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
                if self.selection.take().is_some() {
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
                } else {
                    self.popup = Popup::None;
                }
            }
            KeyCode::Enter => {
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

                if prompt.starts_with('/') && Self::is_known_slash(&prompt) {
                    if self.busy && !Self::is_busy_safe_slash(&prompt) {
                        let base = prompt.split_whitespace().next().unwrap_or(&prompt);
                        self.messages.push(Msg::new(
                            "system",
                            format!("{base} waits for the current turn to finish (Esc cancels it)"),
                        ));
                        return Ok(());
                    }
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
            .and_then(|s| s.list_sessions_filtered(None, None, true, 50, 0))
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

    pub(crate) fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.popup != Popup::None {
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
                if !self.dragging || self.popup != Popup::None {
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
}
