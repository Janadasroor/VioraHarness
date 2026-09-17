// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

//! Terminal window title follows the chat session.
//!
//! Terminals show the shell path by default; while the TUI runs we set
//! the window title to the session title (`OSC 0` escape) so the window
//! identifies the chat instead of the cwd. The title re-syncs on session
//! switch (detected in the event loop) and after `/rename`; on TUI exit
//! it is restored to the cwd.

use super::App;

/// Compose what the window should show: the session title when set,
/// otherwise `untitled <short-id>` — always suffixed with the app name.
pub(crate) fn session_window_title(session_id: &str, title: Option<&str>) -> String {
    let label = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => sanitize_title(t),
        None => {
            let short = &session_id[..8.min(session_id.len())];
            format!("untitled {short}")
        }
    };
    format!("{label} — VioraHarness")
}

/// Strip control characters an OSC sequence must not contain and cap
/// the length so a huge title cannot spam the terminal.
fn sanitize_title(raw: &str) -> String {
    const MAX_CHARS: usize = 120;
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_CHARS)
        .collect()
}

/// Emit an `OSC 0` window-title sequence. Zero-width for the terminal;
/// safe to write between ratatui draws (never mid-draw). No-op in unit
/// tests so the suite never pollutes captured output.
pub(crate) fn emit_terminal_title(title: &str) {
    #[cfg(test)]
    {
        let _ = title;
    }
    #[cfg(not(test))]
    {
        use std::io::Write;
        let clean = sanitize_title(title);
        let _ = write!(std::io::stdout(), "\x1b]0;{clean}\x07");
        let _ = std::io::stdout().flush();
    }
}

/// Best-effort DB lookup of a session's title. `None` when the session
/// is new (not yet persisted), archived, or the DB is unavailable.
pub(crate) fn read_session_title(session_id: &str) -> Option<String> {
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    vioraharness_core::session::SessionStore::new(&db)
        .ok()?
        .get_session(session_id)
        .ok()?
        .and_then(|sess| sess.title)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

impl App {
    /// Re-read the current session's title and push it to the window
    /// when it changed. Called on startup, on session switch (event
    /// loop detects the id change), and explicitly after `/rename`.
    pub(crate) fn sync_terminal_title(&mut self) {
        self.session_title = read_session_title(&self.session_id);
        self.titled_session = self.session_id.clone();
        let composed = session_window_title(&self.session_id, self.session_title.as_deref());
        if composed != self.last_window_title {
            self.last_window_title = composed.clone();
            emit_terminal_title(&composed);
        }
    }

    /// Restore a path-like title on TUI exit so a reused terminal does
    /// not keep showing a stale session name (shells with a title-setting
    /// `PROMPT_COMMAND` overwrite this on the next prompt anyway).
    pub(crate) fn restore_terminal_title() {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !cwd.is_empty() {
            emit_terminal_title(&cwd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titled_session_composes_with_app_suffix() {
        assert_eq!(
            session_window_title("abcdef123456", Some("my api work")),
            "my api work — VioraHarness"
        );
    }

    #[test]
    fn untitled_session_shows_short_id() {
        assert_eq!(
            session_window_title("abcdef123456", None),
            "untitled abcdef12 — VioraHarness"
        );
        assert_eq!(
            session_window_title("abcdef123456", Some("   ")),
            "untitled abcdef12 — VioraHarness",
            "blank titles count as untitled"
        );
    }

    #[test]
    fn sanitize_strips_controls_and_caps_length() {
        assert_eq!(sanitize_title("a\x1bb\x07c\nd"), "abcd");
        let long = "x".repeat(500);
        assert_eq!(sanitize_title(&long).len(), 120);
    }

    #[test]
    fn emit_is_noop_in_tests() {
        // Would pollute captured output if it wrote; just returns.
        emit_terminal_title("anything — VioraHarness");
    }

    #[test]
    fn sync_caches_title_and_composes_window_label() {
        let (db, prev, _guard) = crate::app::testkit::with_temp_db("win-title");
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session("sess-win-1", "m", Some("my api work"))
            .unwrap();
        let mut app = crate::app::testkit::test_app();
        app.session_id = "sess-win-1".into();
        app.sync_terminal_title();
        assert_eq!(app.session_title.as_deref(), Some("my api work"));
        assert_eq!(app.last_window_title, "my api work — VioraHarness");
        assert_eq!(app.titled_session, "sess-win-1");
        // Unknown session → untitled fallback, no crash.
        app.session_id = "nope-missing".into();
        app.sync_terminal_title();
        assert!(app.session_title.is_none());
        assert_eq!(app.last_window_title, "untitled nope-mis — VioraHarness");
        crate::app::testkit::restore_db_env(prev, &db);
    }
}
