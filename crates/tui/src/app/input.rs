// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Default)]
pub struct InputState {
    pub(crate) text: String,
    pub(crate) cursor: usize,
    pub(crate) history: Vec<String>,
    pub(crate) hist_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) completion_idx: usize,
    /// Persist history across restarts. False in tests so the suite never
    /// touches the real history file.
    pub(crate) persist: bool,
    /// When Some, history is loaded from / saved to this file instead of
    /// the global [`Self::history_path`]. Tests use a temp file here so
    /// `cargo test` can never clobber the user's real prompt history
    /// (env vars are process-global and test binaries run in parallel,
    /// so the old `VIORAHARNESS_HISTORY` swap raced across processes).
    pub(crate) history_file: Option<std::path::PathBuf>,
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
        if self.history.last().is_some_and(|last| *last == entry) {
            self.hist_idx = None;
            return;
        }
        self.history.push(entry);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.hist_idx = None;
        if self.persist {
            let target = self.history_file.clone().unwrap_or_else(Self::history_path);
            Self::save_history_to(&target, &self.history);
        }
    }

    /// Drop the last history entry when it equals `expected` (used to
    /// discard a generated mega-prompt in favour of the slash the user
    /// actually typed). Re-saves when persisting so the dropped entry
    /// does not survive a restart.
    pub(crate) fn drop_last_history_if(&mut self, expected: &str) {
        if self.history.last().is_some_and(|last| last == expected) {
            self.history.pop();
            self.hist_idx = None;
            if self.persist {
                let target = self.history_file.clone().unwrap_or_else(Self::history_path);
                Self::save_history_to(&target, &self.history);
            }
        }
    }

    /// Production constructor: history restored from disk, every push
    /// saved back. Arrow-key navigation then works across restarts.
    pub(crate) fn with_disk_history() -> Self {
        Self {
            history: Self::load_history(),
            persist: true,
            history_file: Some(Self::history_path()),
            ..Self::default()
        }
    }

    /// Test constructor: isolated temp file, never the real history.
    /// No env vars, so parallel test binaries cannot race each other.
    #[cfg(test)]
    pub(crate) fn with_history_file(path: std::path::PathBuf) -> Self {
        Self {
            history: Self::load_history_from(&path),
            persist: true,
            history_file: Some(path),
            ..Self::default()
        }
    }

    pub(crate) fn history_path() -> std::path::PathBuf {
        if let Ok(p) = std::env::var("VIORAHARNESS_HISTORY") {
            if !p.trim().is_empty() {
                return std::path::PathBuf::from(p);
            }
        }
        let base = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            });
        base.join("vioraharness/prompt_history.json")
    }

    pub(crate) fn load_history() -> Vec<String> {
        Self::load_history_from(&Self::history_path())
    }

    pub(crate) fn load_history_from(path: &std::path::Path) -> Vec<String> {
        let mut hist: Vec<String> = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if hist.len() > 100 {
            hist = hist.split_off(hist.len() - 100);
        }
        hist
    }

    fn save_history_to(path: &std::path::Path, hist: &[String]) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_string(hist).unwrap_or_default());
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
                "/mode",
                "switch agent mode — /mode <eda|web|android> (tool allowlist)",
            ),
            (
                "/theme",
                "switch theme — /theme <name> or /theme for picker",
            ),
            (
                "/settings",
                "open settings — theme, mode, model, thinking, cards, tasks, compaction, notifications",
            ),
            (
                "/thinking",
                "display toggle + depth — /thinking on/off/low/medium/high/…",
            ),
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
            ("/agents", "live agents — turn, subagents, tasks"),
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

    fn temp_history_path(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("vh_hist_{tag}_{}_{n}.json", std::process::id()))
    }

    fn cleanup(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn history_survives_restart() {
        let path = temp_history_path("roundtrip");
        let mut a = InputState::with_history_file(path.clone());
        a.push_history("first".into());
        a.push_history("second".into());
        drop(a);
        let b = InputState::with_history_file(path.clone());
        assert_eq!(b.history, vec!["first".to_string(), "second".to_string()]);
        assert!(b.persist, "fresh instance keeps persisting");
        cleanup(&path);
    }

    #[test]
    fn history_skips_consecutive_duplicates() {
        let path = temp_history_path("dedup");
        let mut a = InputState::with_history_file(path.clone());
        a.push_history("same".into());
        a.push_history("same".into());
        assert_eq!(a.history.len(), 1);
        assert_eq!(InputState::load_history_from(&path).len(), 1);
        cleanup(&path);
    }

    #[test]
    fn history_tolerates_missing_and_corrupt_files() {
        let path = temp_history_path("corrupt");
        assert!(
            InputState::load_history_from(&path).is_empty(),
            "missing file"
        );
        std::fs::write(&path, "not json{{").unwrap();
        assert!(
            InputState::load_history_from(&path).is_empty(),
            "corrupt file"
        );
        std::fs::write(&path, "[\"kept\", 42]").unwrap();
        assert!(
            InputState::load_history_from(&path).is_empty(),
            "wrong shape"
        );
        cleanup(&path);
    }

    #[test]
    fn history_caps_at_100_entries() {
        let path = temp_history_path("cap");
        let mut a = InputState::with_history_file(path.clone());
        for i in 0..105 {
            a.push_history(format!("p{i}"));
        }
        assert_eq!(a.history.len(), 100);
        assert_eq!(a.history[0], "p5");
        let disk: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk.len(), 100);
        cleanup(&path);
    }

    #[test]
    fn memory_only_by_default() {
        let mut a = InputState::default();
        assert!(!a.persist, "tests and non-TUI uses stay memory-only");
        assert!(a.history_file.is_none());
        a.push_history("x".into());
        assert_eq!(a.history, vec!["x".to_string()]);
    }

    #[test]
    fn drop_last_history_if_discards_generated_prompt() {
        let path = temp_history_path("drop");
        let mut a = InputState::with_history_file(path.clone());
        a.push_history("/skill-new plot things".into());
        a.push_history("GENERATED-MEGA-PROMPT".into());
        a.drop_last_history_if("GENERATED-MEGA-PROMPT");
        assert_eq!(a.history, vec!["/skill-new plot things".to_string()]);
        let disk = InputState::load_history_from(&path);
        assert_eq!(
            disk,
            vec!["/skill-new plot things".to_string()],
            "pop re-saved"
        );
        // Non-matching expected leaves history alone.
        a.drop_last_history_if("something-else");
        assert_eq!(a.history.len(), 1);
        cleanup(&path);
    }

    #[test]
    fn history_files_are_isolated() {
        let p1 = temp_history_path("iso1");
        let p2 = temp_history_path("iso2");
        let mut a = InputState::with_history_file(p1.clone());
        a.push_history("only-in-one".into());
        assert!(
            InputState::load_history_from(&p2).is_empty(),
            "no cross-talk"
        );
        assert_eq!(InputState::load_history_from(&p1).len(), 1);
        cleanup(&p1);
        cleanup(&p2);
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
