#[derive(Debug, Default)]
pub struct InputState {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) hist_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) completion_idx: usize,
}

impl InputState {
    pub(crate) fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    pub(crate) fn backspace(&mut self) {
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
        if self.cursor < self.text.len() {
            let len = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.text.drain(self.cursor..self.cursor + len);
        }
    }
    pub(crate) fn move_left(&mut self) {
        if self.cursor > 0 {
            let p = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.cursor -= p;
        }
    }
    pub(crate) fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            let l = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.cursor += l;
        }
    }
    pub(crate) fn move_to_start(&mut self) {
        self.cursor = 0;
    }
    pub(crate) fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }
    pub(crate) fn delete_to_start(&mut self) {
        if self.cursor > 0 {
            self.text.drain(0..self.cursor);
            self.cursor = 0;
        }
    }
    pub(crate) fn delete_to_end(&mut self) {
        if self.cursor < self.text.len() {
            self.text.truncate(self.cursor);
        }
    }
    pub(crate) fn delete_word_before(&mut self) {
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
