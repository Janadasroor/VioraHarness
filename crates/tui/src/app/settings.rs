use super::*;

/// Settings dialog rows — every TUI behaviour knob in one place.
/// Order is the display order; `SETTINGS_COUNT` must match.
pub(crate) const SETTINGS_COUNT: usize = 9;

pub(crate) const SETTINGS_THEME: usize = 0;
pub(crate) const SETTINGS_MODE: usize = 1;
pub(crate) const SETTINGS_MODEL: usize = 2;
pub(crate) const SETTINGS_THINKING: usize = 3;
pub(crate) const SETTINGS_THINK_LEVEL: usize = 4;
pub(crate) const SETTINGS_TOOL_CARDS: usize = 5;
pub(crate) const SETTINGS_WAKE: usize = 6;
pub(crate) const SETTINGS_COMPACT_THRESHOLD: usize = 7;
pub(crate) const SETTINGS_COMPACT_KEEP: usize = 8;

const THRESHOLDS: [f64; 6] = [0.5, 0.6, 0.7, 0.8, 0.85, 0.9];
const KEEP_TAILS: [usize; 5] = [4, 10, 20, 40, 80];
const CARD_LEVELS: [&str; 4] = ["hidden", "quiet", "compact", "full"];

/// First writable config file: `VIORAHARNESS_CONFIG` wins (it is candidate
/// 0), else the first existing candidate, else `./vioraharness.json`.
pub(crate) fn config_target_path() -> String {
    let cands = vioraharness_core::loop_mod::config_candidates();
    for cand in &cands {
        if std::path::Path::new(cand).is_file() {
            return cand.clone();
        }
    }
    cands
        .into_iter()
        .next()
        .unwrap_or_else(|| "vioraharness.json".into())
}

/// Read-modify-write the target config file, preserving all other keys.
/// Creates a minimal `{}` file when none exists yet.
pub(crate) fn update_config_file(mut f: impl FnMut(&mut serde_json::Value)) {
    let path = config_target_path();
    let mut val: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !val.is_object() {
        val = serde_json::json!({});
    }
    f(&mut val);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    if let Ok(pretty) = serde_json::to_string_pretty(&val) {
        let _ = std::fs::write(&path, pretty);
    }
}

fn set_nested(val: &mut serde_json::Value, section: &str, key: &str, v: serde_json::Value) {
    if val.get(section).and_then(|s| s.as_object()).is_none() {
        val.as_object_mut()
            .expect("config root is object")
            .insert(section.into(), serde_json::json!({}));
    }
    val[section][key] = v;
}

/// Persist a theme switch so it actually applies: `tui_state.json`
/// `last_theme` (session-restore marker) + config `tui.theme` (the name
/// `ThemeConfig::load` reads) + config `theme` palette object (what
/// `Palette::load` paints). The palette cache is mtime-keyed, so the new
/// colors go live on the next render — no restart needed.
pub(crate) fn persist_theme_to_config(name: &str) {
    App::save_tui_state(serde_json::json!({"last_theme": name}));
    let palette = serde_json::to_value(crate::theme::Palette::from_name(name))
        .unwrap_or_else(|_| serde_json::json!({}));
    update_config_file(|val| {
        set_nested(val, "tui", "theme", serde_json::Value::String(name.into()));
        val.as_object_mut()
            .expect("config root is object")
            .insert("theme".into(), palette.clone());
    });
}

pub(crate) fn persist_wake_on_tasks(wake: bool) {
    update_config_file(|val| {
        set_nested(
            val,
            "tui",
            "wake_on_task_done",
            serde_json::Value::Bool(wake),
        );
    });
}

pub(crate) fn persist_thinking_level(level: &str) {
    App::save_tui_state(serde_json::json!({"thinking_level": level}));
    update_config_file(|val| {
        set_nested(
            val,
            "tui",
            "thinking_level",
            serde_json::Value::String(level.into()),
        );
    });
}

pub(crate) fn persist_tool_display_default(level: &str) {
    update_config_file(|val| {
        let section = val
            .as_object_mut()
            .expect("config root is object")
            .entry("tui")
            .or_insert_with(|| serde_json::json!({}));
        let td = section
            .as_object_mut()
            .expect("tui is object")
            .entry("tool_display")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(obj) = td.as_object_mut() {
            obj.insert("default".into(), serde_json::Value::String(level.into()));
        }
    });
}

pub(crate) fn persist_compaction(threshold: f64, keep_tail: usize) {
    update_config_file(|val| {
        set_nested(
            val,
            "compaction",
            "threshold",
            serde_json::Value::from(threshold),
        );
        set_nested(
            val,
            "compaction",
            "keepTail",
            serde_json::Value::from(keep_tail as u64),
        );
    });
}

fn nearest_threshold_idx(t: f64) -> usize {
    let mut best = 0;
    let mut best_d = f64::MAX;
    for (i, c) in THRESHOLDS.iter().enumerate() {
        let d = (c - t).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

fn nearest_keep_idx(k: usize) -> usize {
    let mut best = 0;
    let mut best_d = usize::MAX;
    for (i, c) in KEEP_TAILS.iter().enumerate() {
        let d = c.abs_diff(k);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

fn nearest_card_idx(level: &str) -> usize {
    let canon = tool_verbosity_name(parse_tool_verbosity(level));
    CARD_LEVELS.iter().position(|l| *l == canon).unwrap_or(2)
}

impl App {
    pub(crate) fn open_settings(&mut self) {
        self.settings_cursor = self.settings_cursor.min(SETTINGS_COUNT - 1);
        self.popup = Popup::Settings;
    }

    pub(crate) fn settings_len() -> usize {
        SETTINGS_COUNT
    }

    /// Current theme name as the dialog shows it.
    pub(crate) fn settings_theme_name(&self) -> String {
        crate::theme::ThemeConfig::load().theme
    }

    /// (label, value, hint) for row `idx`.
    pub(crate) fn settings_row(&self, idx: usize) -> (String, String, String) {
        match idx {
            SETTINGS_THEME => ("Theme".into(), self.settings_theme_name(), "palette".into()),
            SETTINGS_MODE => (
                "Agent mode".into(),
                self.agent_mode.clone(),
                "tool allowlist".into(),
            ),
            SETTINGS_MODEL => {
                let short = self.model.split('/').next_back().unwrap_or(&self.model);
                ("Model".into(), short.to_string(), "Enter for picker".into())
            }
            SETTINGS_THINKING => (
                "Thinking".into(),
                if self.show_thinking {
                    "on".into()
                } else {
                    "off".into()
                },
                "reasoning block".into(),
            ),
            SETTINGS_THINK_LEVEL => (
                "Think level".into(),
                self.thinking_level.clone(),
                "depth (Ctrl+T)".into(),
            ),
            SETTINGS_TOOL_CARDS => (
                "Tool cards".into(),
                tool_verbosity_name(self.tool_verbosity("")).into(),
                "chat density".into(),
            ),
            SETTINGS_WAKE => (
                "Wake on task done".into(),
                if self.wake_on_tasks {
                    "on".into()
                } else {
                    "off".into()
                },
                "auto-continue".into(),
            ),
            SETTINGS_COMPACT_THRESHOLD => {
                let t = vioraharness_core::context::compaction::compaction_config().threshold;
                (
                    "Compact at".into(),
                    format!("{}%", (t * 100.0).round() as usize),
                    "auto-compact".into(),
                )
            }
            SETTINGS_COMPACT_KEEP => {
                let k = vioraharness_core::context::compaction::compaction_config().keep_tail;
                (
                    "Compact keep".into(),
                    format!("{k} msgs"),
                    "tail kept".into(),
                )
            }
            _ => ("?".into(), String::new(), String::new()),
        }
    }

    /// Silent theme switch for the dialog (status feedback, no chat spam).
    /// The shared `/theme` command keeps its chat message.
    pub(crate) fn apply_theme_silent(&mut self, name: &str) {
        persist_theme_to_config(name);
        self.status = format!("Theme → {name} (saved, applied)");
    }

    /// Silent mode switch for the dialog: same persistence as `/mode`
    /// (tui_state + per-chat row) without a chat message per step.
    pub(crate) fn apply_mode_silent(&mut self, name: &str) {
        let norm = vioraharness_core::mode::normalize_mode_name(name);
        if !vioraharness_core::mode::is_known_mode(&norm) || norm == self.agent_mode {
            return;
        }
        self.agent_mode = norm.clone();
        Self::save_tui_state(serde_json::json!({"last_mode": self.agent_mode}));
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
            let _ = store.set_session_mode(&self.session_id, &norm);
        }
        let tools = vioraharness_core::mode::registry_for_mode(&norm)
            .all()
            .len();
        self.status = format!("mode → {norm} ({tools} tools, saved)");
    }

    /// Step the reasoning-depth dial (`Ctrl+T`): persists to TUI state
    /// and config at once and applies to new turns. Status feedback
    /// only, no chat spam.
    pub(crate) fn cycle_thinking_level(&mut self, dir: i32) {
        let next = vioraharness_core::thinking::cycle_thinking_level(&self.thinking_level, dir);
        self.thinking_level = next.clone();
        persist_thinking_level(&next);
        self.status = format!("think level → {next} (new turns, saved)");
    }

    pub(crate) fn toggle_thinking_silent(&mut self) {
        self.show_thinking = !self.show_thinking;
        self.thinking_expanded = self.show_thinking;
        Self::save_tui_state(serde_json::json!({"show_thinking": self.show_thinking}));
        update_config_file(|val| {
            set_nested(
                val,
                "tui",
                "show_thinking",
                serde_json::Value::Bool(self.show_thinking),
            );
        });
        self.status = format!(
            "Thinking {} — saved",
            if self.show_thinking { "ON" } else { "OFF" }
        );
    }

    pub(crate) fn cycle_tool_cards_silent(&mut self, dir: i32) {
        let cur = nearest_card_idx(&self.tool_display_default);
        let n = CARD_LEVELS.len() as i32;
        let next = CARD_LEVELS[((cur as i32 + dir).rem_euclid(n)) as usize];
        self.tool_display_default = next.to_string();
        self.save_tool_display();
        persist_tool_display_default(next);
        self.status = format!("tool cards default → {next} — saved");
    }

    pub(crate) fn toggle_wake_silent(&mut self) {
        self.wake_on_tasks = !self.wake_on_tasks;
        persist_wake_on_tasks(self.wake_on_tasks);
        self.status = format!(
            "wake on task done {} — saved",
            if self.wake_on_tasks { "ON" } else { "OFF" }
        );
    }

    pub(crate) fn cycle_theme_silent(&mut self, dir: i32) {
        if self.available_themes.is_empty() {
            return;
        }
        let cur_name = self.settings_theme_name();
        let cur = self
            .available_themes
            .iter()
            .position(|t| t == &cur_name)
            .unwrap_or(0) as i32;
        let n = self.available_themes.len() as i32;
        let next = self.available_themes[((cur + dir).rem_euclid(n)) as usize].clone();
        self.apply_theme_silent(&next);
    }

    pub(crate) fn cycle_mode_silent(&mut self, dir: i32) {
        let modes = vioraharness_core::mode::builtin_modes();
        if modes.is_empty() {
            return;
        }
        let cur = modes
            .iter()
            .position(|m| m.name == self.agent_mode)
            .unwrap_or(0) as i32;
        let n = modes.len() as i32;
        let next = modes[((cur + dir).rem_euclid(n)) as usize].name.to_string();
        self.apply_mode_silent(&next);
    }

    pub(crate) fn cycle_threshold_silent(&mut self, dir: i32) {
        let cfg = vioraharness_core::context::compaction::compaction_config();
        let cur = nearest_threshold_idx(cfg.threshold);
        let n = THRESHOLDS.len() as i32;
        let next = THRESHOLDS[((cur as i32 + dir).rem_euclid(n)) as usize];
        persist_compaction(next, cfg.keep_tail);
        self.status = format!(
            "auto-compact at {}% — saved",
            (next * 100.0).round() as usize
        );
    }

    pub(crate) fn cycle_keep_tail_silent(&mut self, dir: i32) {
        let cfg = vioraharness_core::context::compaction::compaction_config();
        let cur = nearest_keep_idx(cfg.keep_tail);
        let n = KEEP_TAILS.len() as i32;
        let next = KEEP_TAILS[((cur as i32 + dir).rem_euclid(n)) as usize];
        persist_compaction(cfg.threshold, next);
        self.status = format!("compact keep → {next} msgs — saved");
    }

    /// ←/→ (or h/l) on the highlighted row.
    pub(crate) fn settings_cycle(&mut self, dir: i32) {
        match self.settings_cursor {
            SETTINGS_THEME => self.cycle_theme_silent(dir),
            SETTINGS_MODE => self.cycle_mode_silent(dir),
            SETTINGS_MODEL => {
                self.popup = Popup::ModelPicker;
                self.model_filter.clear();
                self.model_cursor = 0;
            }
            SETTINGS_THINKING => self.toggle_thinking_silent(),
            SETTINGS_THINK_LEVEL => self.cycle_thinking_level(dir),
            SETTINGS_TOOL_CARDS => self.cycle_tool_cards_silent(dir),
            SETTINGS_WAKE => self.toggle_wake_silent(),
            SETTINGS_COMPACT_THRESHOLD => self.cycle_threshold_silent(dir),
            SETTINGS_COMPACT_KEEP => self.cycle_keep_tail_silent(dir),
            _ => {}
        }
    }

    /// Enter/Space on the highlighted row.
    pub(crate) fn settings_activate(&mut self) {
        match self.settings_cursor {
            SETTINGS_MODEL => {
                self.popup = Popup::ModelPicker;
                self.model_filter.clear();
                self.model_cursor = 0;
            }
            SETTINGS_THEME => self.cycle_theme_silent(1),
            SETTINGS_MODE => self.cycle_mode_silent(1),
            SETTINGS_THINKING => self.toggle_thinking_silent(),
            SETTINGS_THINK_LEVEL => self.cycle_thinking_level(1),
            SETTINGS_WAKE => self.toggle_wake_silent(),
            _ => self.settings_cycle(1),
        }
    }

    pub(crate) fn settings_move(&mut self, dir: i32) {
        let n = SETTINGS_COUNT as i32;
        let cur = self.settings_cursor as i32;
        self.settings_cursor = ((cur + dir).rem_euclid(n)) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;

    struct EnvGuard {
        prev_config: Option<String>,
        prev_xdg: Option<String>,
        prev_db: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: std::path::PathBuf,
    }

    fn isolated_env(tag: &str) -> EnvGuard {
        let lock = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("vh_settings_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prev_config = std::env::var("VIORAHARNESS_CONFIG").ok();
        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let prev_db = std::env::var("VIORAHARNESS_DB").ok();
        let cfg = dir.join("vioraharness.json");
        std::fs::write(&cfg, "{}").unwrap();
        std::env::set_var("VIORAHARNESS_CONFIG", &cfg);
        std::env::set_var("XDG_DATA_HOME", &dir);
        let db = dir.join("sessions.db");
        std::env::set_var("VIORAHARNESS_DB", &db);
        EnvGuard {
            prev_config,
            prev_xdg,
            prev_db,
            _lock: lock,
            _dir: dir,
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev_config {
                Some(v) => std::env::set_var("VIORAHARNESS_CONFIG", v),
                None => std::env::remove_var("VIORAHARNESS_CONFIG"),
            }
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
            match &self.prev_db {
                Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
                None => std::env::remove_var("VIORAHARNESS_DB"),
            }
            let _ = std::fs::remove_dir_all(&self._dir);
        }
    }

    #[test]
    fn settings_command_opens_dialog() {
        let _g = isolated_env("open");
        let mut app = test_app();
        app.handle_slash("/settings");
        assert_eq!(app.popup, Popup::Settings);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Settings"), "dialog title: {text:?}");
        assert!(text.contains("Theme"), "theme row");
        assert!(text.contains("Tool cards"), "verbosity row");
        assert!(text.contains("Compact"), "compaction rows");
    }

    #[test]
    fn settings_aliases_open_dialog() {
        let _g = isolated_env("alias");
        for cmd in ["/setting", "/config", "/options", "/preferences"] {
            let mut app = test_app();
            app.handle_slash(cmd);
            assert_eq!(app.popup, Popup::Settings, "{cmd} opens settings");
        }
    }

    #[test]
    fn settings_thinking_toggle_persists() {
        let _g = isolated_env("thinking");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_THINKING;
        let before = app.show_thinking;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.show_thinking, !before);
        assert!(app.status.contains("Thinking"), "status: {}", app.status);
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v.get("tui")
                .and_then(|t| t.get("show_thinking"))
                .and_then(|x| x.as_bool()),
            Some(!before)
        );
    }

    #[test]
    fn settings_wake_toggle_persists() {
        let _g = isolated_env("wake");
        let mut app = test_app();
        let before = app.wake_on_tasks;
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_WAKE;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.wake_on_tasks, !before);
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v.get("tui")
                .and_then(|t| t.get("wake_on_task_done"))
                .and_then(|x| x.as_bool()),
            Some(!before)
        );
    }

    #[test]
    fn settings_tool_cards_cycle_persists() {
        let _g = isolated_env("cards");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_TOOL_CARDS;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert!(CARD_LEVELS.contains(&app.tool_display_default.as_str()));
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v.get("tui")
                .and_then(|t| t.get("tool_display"))
                .and_then(|t| t.get("default"))
                .and_then(|x| x.as_str()),
            Some(app.tool_display_default.as_str())
        );
    }

    #[test]
    fn settings_compaction_cycles_persist() {
        let _g = isolated_env("compact");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_COMPACT_THRESHOLD;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        app.settings_cursor = SETTINGS_COMPACT_KEEP;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        let t = v
            .get("compaction")
            .and_then(|c| c.get("threshold"))
            .and_then(|x| x.as_f64())
            .expect("threshold persisted");
        let k = v
            .get("compaction")
            .and_then(|c| c.get("keepTail"))
            .and_then(|x| x.as_u64())
            .expect("keepTail persisted");
        assert!(THRESHOLDS.contains(&t), "threshold {t}");
        assert!(KEEP_TAILS.contains(&(k as usize)), "keep {k}");
    }

    #[test]
    fn settings_theme_cycles_and_persists_palette() {
        let _g = isolated_env("theme");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_THEME;
        let before = app.settings_theme_name();
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        let after = app.settings_theme_name();
        assert_ne!(after, before, "theme advanced");
        assert!(app.available_themes.contains(&after));
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            v.get("tui")
                .and_then(|t| t.get("theme"))
                .and_then(|x| x.as_str()),
            Some(after.as_str())
        );
        assert!(
            v.get("theme")
                .and_then(|t| t.get("primary"))
                .and_then(|x| x.as_str())
                .is_some(),
            "palette object written"
        );
    }

    #[test]
    fn settings_model_row_opens_picker() {
        let _g = isolated_env("model");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = SETTINGS_MODEL;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.popup, Popup::ModelPicker);
    }

    #[test]
    fn settings_cursor_wraps_and_esc_closes() {
        let _g = isolated_env("nav");
        let mut app = test_app();
        app.handle_slash("/settings");
        app.settings_cursor = 0;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.settings_cursor, SETTINGS_COUNT - 1);
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.settings_cursor, 0);
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.popup, Popup::None);
    }

    #[test]
    fn think_level_cycle_persists_to_both_files() {
        let _g = isolated_env("thinklevel");
        let mut app = test_app();
        assert_eq!(app.thinking_level, "medium");
        app.cycle_thinking_level(1);
        assert_eq!(app.thinking_level, "high");
        assert!(app.status.contains("high"), "status: {}", app.status);
        app.cycle_thinking_level(-1);
        assert_eq!(app.thinking_level, "medium");
        app.cycle_thinking_level(-1);
        assert_eq!(app.thinking_level, "low");
        // TUI state file.
        let xdg = std::env::var("XDG_DATA_HOME").unwrap();
        let state = std::fs::read_to_string(
            std::path::PathBuf::from(xdg).join("vioraharness/tui_state.json"),
        )
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&state).unwrap();
        assert_eq!(
            v.get("thinking_level").and_then(|x| x.as_str()),
            Some("low")
        );
        // Project config file.
        let cfg = std::fs::read_to_string(config_target_path()).unwrap();
        let c: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(
            c.get("tui")
                .and_then(|t| t.get("thinking_level"))
                .and_then(|x| x.as_str()),
            Some("low")
        );
    }

    #[test]
    fn thinking_command_sets_depth_but_display_words_do_not() {
        let _g = isolated_env("thinkcmd");
        let mut app = test_app();
        app.handle_slash("/thinking high");
        assert_eq!(app.thinking_level, "high");
        assert!(
            app.messages.last().unwrap().content.contains("high"),
            "confirmed in chat"
        );
        app.handle_slash("/thinking off");
        assert!(!app.show_thinking, "legacy display toggle kept");
        assert_eq!(
            app.thinking_level, "high",
            "display words never move the dial"
        );
        app.handle_slash("/thinking none");
        assert_eq!(app.thinking_level, "off");
        app.handle_slash("/thinking bogus");
        assert_eq!(app.thinking_level, "off", "unknown toggles display only");
    }

    #[test]
    fn settings_dialog_shows_and_cycles_think_level_row() {
        let _g = isolated_env("thinkrow");
        let mut app = test_app();
        app.handle_slash("/settings");
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Think level"), "row label: {text:?}");
        assert!(text.contains("medium"), "current level shown");
        app.settings_cursor = SETTINGS_THINK_LEVEL;
        app.handle_popup_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Right,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.thinking_level, "high");
        let (_, value, _) = app.settings_row(SETTINGS_THINK_LEVEL);
        assert_eq!(value, "high");
    }

    #[test]
    fn input_title_shows_level_next_to_model() {
        let _g = isolated_env("titlechip");
        let mut app = test_app();
        app.model = "test/probe-model".into();
        app.thinking_level = "high".into();
        let text = render_text(&mut app, 120, 30);
        assert!(
            text.contains("probe-model · high"),
            "level next to model: {text:?}"
        );
    }
}
