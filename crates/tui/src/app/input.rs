#[derive(Debug, Default)]
pub struct InputState {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) hist_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) completion_idx: usize,
    /// Keyboard/mouse text selection anchor (byte index). The selection
    /// spans anchor..cursor; None means no selection.
    pub(crate) sel_anchor: Option<usize>,
}

impl InputState {
    /// Ordered, non-empty selected byte range, if any.
    pub(crate) fn selected_range(&self) -> Option<(usize, usize)> {
        let a = self.sel_anchor?;
        let (lo, hi) = (a.min(self.cursor), a.max(self.cursor));
        if lo == hi {
            return None;
        }
        Some((lo.min(self.text.len()), hi.min(self.text.len())))
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.selected_range()
            .map(|(lo, hi)| self.text[lo..hi].to_string())
    }

    pub(crate) fn clear_selection(&mut self) {
        self.sel_anchor = None;
    }

    /// Delete the selected range, placing the cursor at its start.
    /// Returns true when something was deleted.
    pub(crate) fn delete_selection(&mut self) -> bool {
        if let Some((lo, hi)) = self.selected_range() {
            self.text.drain(lo..hi);
            self.cursor = lo;
            self.sel_anchor = None;
            true
        } else {
            false
        }
    }

    fn move_to(&mut self, pos: usize, extend: bool) {
        let pos = pos.min(self.text.len());
        if extend {
            if self.sel_anchor.is_none() {
                self.sel_anchor = Some(self.cursor);
            }
            self.cursor = pos;
            if self.sel_anchor == Some(self.cursor) {
                self.sel_anchor = None;
            }
        } else {
            self.cursor = pos;
            self.sel_anchor = None;
        }
    }

    pub(crate) fn insert(&mut self, c: char) {
        self.delete_selection();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    pub(crate) fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            let prev = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.cursor -= prev;
            self.text.remove(self.cursor);
        }
    }
    pub(crate) fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor < self.text.len() {
            let len = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.text.drain(self.cursor..self.cursor + len);
        }
    }
    pub(crate) fn move_left(&mut self, extend: bool) {
        if self.cursor > 0 {
            let p = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.move_to(self.cursor - p, extend);
        } else if !extend {
            self.sel_anchor = None;
        }
    }
    pub(crate) fn move_right(&mut self, extend: bool) {
        if self.cursor < self.text.len() {
            let l = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.move_to(self.cursor + l, extend);
        } else if !extend {
            self.sel_anchor = None;
        }
    }
    pub(crate) fn move_to_start(&mut self, extend: bool) {
        self.move_to(0, extend);
    }
    pub(crate) fn move_to_end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }
    pub(crate) fn delete_to_start(&mut self) {
        self.sel_anchor = None;
        if self.cursor > 0 {
            self.text.drain(0..self.cursor);
            self.cursor = 0;
        }
    }
    pub(crate) fn delete_to_end(&mut self) {
        self.sel_anchor = None;
        if self.cursor < self.text.len() {
            self.text.truncate(self.cursor);
        }
    }
    pub(crate) fn delete_word_before(&mut self) {
        self.delete_selection();
        if self.cursor == 0 {
            return;
        }
        let mut end = self.cursor;

        while end > 0 {
            let c = self.text[..end].chars().next_back().unwrap();
            if !c.is_whitespace() {
                break;
            }
            end -= c.len_utf8();
        }

        while end > 0 {
            let c = self.text[..end].chars().next_back().unwrap();
            if c.is_whitespace() {
                break;
            }
            end -= c.len_utf8();
        }
        self.text.drain(end..self.cursor);
        self.cursor = end;
    }
    pub(crate) fn delete_word_after(&mut self) {
        self.delete_selection();
        if self.cursor >= self.text.len() {
            return;
        }
        let mut start = self.cursor;

        while start < self.text.len() {
            let c = self.text[start..].chars().next().unwrap();
            if !c.is_whitespace() {
                break;
            }
            start += c.len_utf8();
        }
        let mut end = start;
        while end < self.text.len() {
            let c = self.text[end..].chars().next().unwrap();
            if c.is_whitespace() {
                break;
            }
            end += c.len_utf8();
        }
        self.text.drain(self.cursor..end);
    }
    pub(crate) fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }
    pub(crate) fn push_history(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        self.history.push(entry);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.hist_idx = None;
    }
    pub(crate) fn hist_prev(&mut self) {
        self.sel_anchor = None;
        if self.history.is_empty() {
            return;
        }
        if self.hist_idx.is_none() {
            self.draft = self.text.clone();
            self.hist_idx = Some(self.history.len());
        }
        if let Some(idx) = self.hist_idx {
            if idx > 0 {
                let n = idx - 1;
                self.hist_idx = Some(n);
                self.text = self.history[n].clone();
                self.cursor = self.text.len();
            }
        }
    }
    pub(crate) fn hist_next(&mut self) {
        self.sel_anchor = None;
        if let Some(idx) = self.hist_idx {
            if idx + 1 < self.history.len() {
                let n = idx + 1;
                self.hist_idx = Some(n);
                self.text = self.history[n].clone();
                self.cursor = self.text.len();
            } else {
                self.hist_idx = None;
                self.text = self.draft.clone();
                self.cursor = self.text.len();
            }
        }
    }
    pub(crate) fn slash_completions(&self) -> Vec<(&'static str, &'static str)> {
        const CMDS: &[(&str, &str)] = &[
            ("/help", "show help"),
            ("/clear", "clear chat"),
            ("/sessions", "list sessions — chats history"),
            ("/chats", "list chats — alias for /sessions"),
            ("/history", "chat history — alias for /sessions"),
            ("/conversations", "alias for /sessions"),
            ("/ls", "list sessions"),
            ("/providers", "manage API keys — /providers"),
            ("/provider", "alias for /providers"),
            ("/new", "new chat — fresh session"),
            ("/resume", "resume chat — /resume <id>"),
            ("/r", "alias for /resume"),
            ("/open", "alias for /resume"),
            ("/fork", "fork chat — /fork [at_seq]"),
            ("/rename", "rename chat — /rename <title>"),
            ("/archive", "archive chat"),
            ("/delete", "delete chat — careful!"),
            ("/export", "export chat JSONL"),
            (
                "/model",
                "switch model — /model <name> or /model for picker",
            ),
            (
                "/theme",
                "switch theme — /theme <name> or /theme for picker",
            ),
            ("/thinking", "toggle thinking — /thinking on/off"),
            (
                "/verbosity",
                "tool card verbosity — /verbosity [tool] <hidden|quiet|compact|full>",
            ),
            ("/undo", "undo last tool"),
            (
                "/rewind",
                "rewind chat — restore checkpoints at any message",
            ),
            (
                "/compact",
                "compact context now — auto at 80% with real summary",
            ),
            ("/quit", "quit tui"),
            ("/q", "alias for /quit"),
            ("/exit", "alias for /quit"),
            ("/permissions", "show permissions"),
            ("/tasks", "background tasks — list, logs, kill"),
            ("/errors", "error log — list, full text, clear"),
            ("/perms", "alias for /permissions"),
            ("/diff", "show last diff"),
            ("/output", "view last tool output (bash, very long)"),
            ("/view", "alias for /output"),
            ("/tool", "alias for /output"),
            ("/skills", "list skills — auto-loaded on intent"),
            (
                "/skill-new",
                "describe a skill — AI names it and writes SKILL.md (--local for ./skills)",
            ),
        ];
        if !self.text.starts_with('/') {
            return vec![];
        }
        let q = self.text.as_str();

        if q.contains(' ') {
            let base = q.split_whitespace().next().unwrap_or("");
            if base != q {
                return vec![];
            }
        }
        CMDS.iter()
            .filter(|(c, _)| c.starts_with(q))
            .cloned()
            .collect()
    }

    pub(crate) fn apply_completion(&mut self, completion: &str) {
        self.sel_anchor = None;
        if let Some(space) = self.text.find(' ') {
            let rest = self.text[space..].to_string();
            self.text = format!("{completion}{rest}");
        } else {
            let needs_space = matches!(completion, "/model" | "/verbosity" | "/verbose");
            self.text = if needs_space {
                format!("{completion} ")
            } else {
                completion.to_string()
            };
        }
        self.cursor = self.text.len();
        self.completion_idx = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> InputState {
        let mut input = InputState::default();
        input.insert_str(s);
        input.move_to_end(false);
        input.clear_selection();
        input
    }

    #[test]
    fn shift_arrows_extend_and_collapse() {
        let mut input = typed("hello");
        assert_eq!(input.selected_range(), None);
        input.move_left(true);
        input.move_left(true);
        assert_eq!(input.selected_range(), Some((3, 5)));
        assert_eq!(input.selected_text().as_deref(), Some("lo"));
        input.move_right(true);
        assert_eq!(input.selected_range(), Some((4, 5)));
        input.move_right(true);
        assert_eq!(input.selected_range(), None, "back at anchor collapses");
        input.move_left(false);
        assert_eq!(input.selected_range(), None);
        assert_eq!(input.cursor, 4);
    }

    #[test]
    fn home_end_with_shift_select_to_edges() {
        let mut input = typed("hello");
        input.move_to_start(true);
        assert_eq!(input.selected_text().as_deref(), Some("hello"));
        input.move_to_end(false);
        assert_eq!(input.selected_range(), None);
        input.move_to_start(false);
        input.move_right(true);
        input.move_right(true);
        assert_eq!(input.selected_text().as_deref(), Some("he"));
    }

    #[test]
    fn typing_replaces_selection() {
        let mut input = typed("hello");
        input.move_to_start(true);
        input.insert('X');
        assert_eq!(input.text, "X");
        assert_eq!(input.cursor, 1);
        assert_eq!(input.selected_range(), None);
    }

    #[test]
    fn backspace_deletes_selection() {
        let mut input = typed("hello");
        input.move_left(true);
        input.move_left(true);
        input.backspace();
        assert_eq!(input.text, "hel");
        assert_eq!(input.selected_range(), None);
    }

    #[test]
    fn wide_chars_move_by_char_not_byte() {
        let mut input = typed("aｱb");
        input.move_to_start(true);
        assert_eq!(input.selected_text().as_deref(), Some("aｱb"));
        input.clear_selection();
        input.move_right(true);
        assert_eq!(input.cursor, 1);
        input.move_right(true);
        assert_eq!(input.cursor, 4, "skips the 3-byte char");
    }
}
