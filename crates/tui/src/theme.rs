use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

struct JsonFileCache {
    entries: HashMap<PathBuf, (SystemTime, serde_json::Value)>,
}

fn json_cache() -> &'static Mutex<JsonFileCache> {
    static CACHE: OnceLock<Mutex<JsonFileCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(JsonFileCache {
            entries: HashMap::new(),
        })
    })
}

fn cached_json(path: &str) -> Option<serde_json::Value> {
    let pb = PathBuf::from(path);
    let mtime = std::fs::metadata(&pb).ok()?.modified().ok()?;
    {
        let cache = json_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some((m, v)) = cache.entries.get(&pb) {
            if *m == mtime {
                return Some(v.clone());
            }
        }
    }
    let s = std::fs::read_to_string(&pb).ok()?;
    let v: serde_json::Value = serde_json::from_str(&s).ok()?;
    json_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries
        .insert(pb, (mtime, v.clone()));
    Some(v)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_name")]
    pub theme: String,
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    #[serde(default = "default_thinking_title")]
    pub thinking_title: String,
    #[serde(default = "default_thinking_label")]
    pub thinking_label: String,
    #[serde(
        default = "default_opacity",
        alias = "thinking_opacity",
        alias = "thinkingopacity"
    )]
    pub thinkingOpacity: f32,
}

fn default_theme_name() -> String {
    "eye-comfort".into()
}
fn default_true() -> bool {
    true
}
fn default_thinking_title() -> String {
    "Thinking".into()
}
fn default_thinking_label() -> String {
    "Reasoning".into()
}
fn default_opacity() -> f32 {
    0.5
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            theme: default_theme_name(),
            show_thinking: true,
            thinking_title: default_thinking_title(),
            thinking_label: default_thinking_label(),
            thinkingOpacity: 0.6,
        }
    }
}

impl ThemeConfig {
    pub fn load() -> Self {
        for cand in vioraharness_core::loop_mod::config_candidates() {
            if let Some(v) = cached_json(&cand) {
                if let Some(tui) = v.get("tui") {
                    if let Ok(cfg) = serde_json::from_value::<ThemeConfig>(tui.clone()) {
                        return cfg;
                    }
                }

                if let Some(th) = v.get("theme") {
                    if let Ok(pal) = serde_json::from_value::<Palette>(th.clone()) {
                        let cfg = ThemeConfig::default();

                        let _ = pal;
                        return cfg;
                    }
                }
            }
        }
        ThemeConfig::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct Palette {
    pub primary: String,
    pub secondary: String,
    pub accent: String,
    pub error: String,
    pub warning: String,
    pub success: String,
    pub info: String,
    pub text: String,
    #[serde(alias = "text_muted", alias = "textmuted")]
    pub textMuted: String,
    pub background: String,
    #[serde(alias = "background_panel", alias = "backgroundpanel")]
    pub backgroundPanel: String,
    #[serde(alias = "background_element", alias = "backgroundelement")]
    pub backgroundElement: String,
    pub border: String,
    #[serde(alias = "border_subtle", alias = "bordersubtle")]
    pub borderSubtle: String,
    #[serde(alias = "border_active", alias = "borderactive")]
    pub borderActive: String,
}

impl Palette {
    pub fn load() -> Self {
        for cand in vioraharness_core::loop_mod::config_candidates() {
            if let Some(v) = cached_json(&cand) {
                if let Some(th) = v.get("theme") {
                    if let Ok(p) = serde_json::from_value::<Palette>(th.clone()) {
                        return p;
                    }
                }
            }
        }
        Self::tokyonight()
    }

    pub fn tokyonight() -> Self {
        Self {
            primary: "#7aa2f7".into(),
            secondary: "#bb9af7".into(),
            accent: "#7dcfff".into(),
            error: "#f7768e".into(),
            warning: "#e0af68".into(),
            success: "#9ece6a".into(),
            info: "#7dcfff".into(),
            text: "#c0caf5".into(),
            textMuted: "#565f89".into(),
            background: "#1a1b26".into(),
            backgroundPanel: "#24283b".into(),
            backgroundElement: "#414868".into(),
            border: "#414868".into(),
            borderSubtle: "#24283b".into(),
            borderActive: "#7aa2f7".into(),
        }
    }
    pub fn catppuccin() -> Self {
        Self {
            primary: "#89b4fa".into(),
            secondary: "#f38ba8".into(),
            accent: "#94e2d5".into(),
            error: "#f38ba8".into(),
            warning: "#fab387".into(),
            success: "#a6e3a1".into(),
            info: "#89b4fa".into(),
            text: "#cdd6f4".into(),
            textMuted: "#6c7086".into(),
            background: "#1e1e2e".into(),
            backgroundPanel: "#313244".into(),
            backgroundElement: "#45475a".into(),
            border: "#45475a".into(),
            borderSubtle: "#313244".into(),
            borderActive: "#89b4fa".into(),
        }
    }
    pub fn dracula() -> Self {
        Self {
            primary: "#bd93f9".into(),
            secondary: "#ff79c6".into(),
            accent: "#8be9fd".into(),
            error: "#ff5555".into(),
            warning: "#f1fa8c".into(),
            success: "#50fa7b".into(),
            info: "#8be9fd".into(),
            text: "#f8f8f2".into(),
            textMuted: "#6272a4".into(),
            background: "#282a36".into(),
            backgroundPanel: "#44475a".into(),
            backgroundElement: "#6272a4".into(),
            border: "#6272a4".into(),
            borderSubtle: "#44475a".into(),
            borderActive: "#bd93f9".into(),
        }
    }
    pub fn gruvbox() -> Self {
        Self {
            primary: "#fabd2f".into(),
            secondary: "#d3869b".into(),
            accent: "#83a598".into(),
            error: "#fb4934".into(),
            warning: "#fabd2f".into(),
            success: "#b8bb26".into(),
            info: "#83a598".into(),
            text: "#ebdbb2".into(),
            textMuted: "#928374".into(),
            background: "#282828".into(),
            backgroundPanel: "#3c3836".into(),
            backgroundElement: "#504945".into(),
            border: "#504945".into(),
            borderSubtle: "#3c3836".into(),
            borderActive: "#fabd2f".into(),
        }
    }
    pub fn nord() -> Self {
        Self {
            primary: "#88c0d0".into(),
            secondary: "#81a1c1".into(),
            accent: "#8fbcbb".into(),
            error: "#bf616a".into(),
            warning: "#ebcb8b".into(),
            success: "#a3be8c".into(),
            info: "#88c0d0".into(),
            text: "#eceff4".into(),
            textMuted: "#4c566a".into(),
            background: "#2e3440".into(),
            backgroundPanel: "#3b4252".into(),
            backgroundElement: "#434c5e".into(),
            border: "#434c5e".into(),
            borderSubtle: "#3b4252".into(),
            borderActive: "#88c0d0".into(),
        }
    }
    pub fn eye_comfort() -> Self {
        Self {
            primary: "#c9a86a".into(),
            secondary: "#d4b896".into(),
            accent: "#e0c9a6".into(),
            error: "#d98c8c".into(),
            warning: "#e0b88c".into(),
            success: "#a8c090".into(),
            info: "#c9a86a".into(),
            text: "#d6c7b8".into(),
            textMuted: "#8a7d6b".into(),
            background: "#1e1c1a".into(),
            backgroundPanel: "#252220".into(),
            backgroundElement: "#2f2b28".into(),
            border: "#3a3632".into(),
            borderSubtle: "#252220".into(),
            borderActive: "#c9a86a".into(),
        }
    }
    pub fn warm_dark() -> Self {
        Self {
            primary: "#fbbf24".into(),
            secondary: "#f59e0b".into(),
            accent: "#fcd34d".into(),
            error: "#f87171".into(),
            warning: "#fbbf24".into(),
            success: "#a3e635".into(),
            info: "#fcd34d".into(),
            text: "#e7e5e4".into(),
            textMuted: "#a8a29e".into(),
            background: "#1c1917".into(),
            backgroundPanel: "#292524".into(),
            backgroundElement: "#3a3632".into(),
            border: "#44403c".into(),
            borderSubtle: "#292524".into(),
            borderActive: "#fbbf24".into(),
        }
    }
    pub fn tokyonight_soft() -> Self {
        Self {
            primary: "#82a1f0".into(),
            secondary: "#b4a0f5".into(),
            accent: "#7dcfff".into(),
            error: "#f7768e".into(),
            warning: "#e0af68".into(),
            success: "#9ece6a".into(),
            info: "#7dcfff".into(),
            text: "#c8d3f5".into(),
            textMuted: "#6e7aa0".into(),
            background: "#1e2030".into(),
            backgroundPanel: "#262a40".into(),
            backgroundElement: "#3b3f5a".into(),
            border: "#3b3f5a".into(),
            borderSubtle: "#262a40".into(),
            borderActive: "#82a1f0".into(),
        }
    }
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "catppuccin" | "catppuccin-mocha" => Self::catppuccin(),
            "dracula" => Self::dracula(),
            "gruvbox" => Self::gruvbox(),
            "nord" => Self::nord(),
            "eye-comfort" | "eye_comfort" | "eye" => Self::eye_comfort(),
            "warm-dark" | "warm_dark" | "warm" => Self::warm_dark(),
            "tokyonight-soft" | "tokyonight_soft" | "soft" => Self::tokyonight_soft(),
            _ => Self::tokyonight(),
        }
    }
}

fn hex_to_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        ) {
            return Color::Rgb(r, g, b);
        }
    }
    Color::White
}

pub struct Theme;

impl Theme {
    fn palette() -> Palette {
        Palette::load()
    }
    fn config() -> ThemeConfig {
        ThemeConfig::load()
    }

    pub fn header_bg() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundPanel))
            .fg(hex_to_color(&p.text))
    }
    pub fn header_title() -> Style {
        let p = Self::palette();
        Style::default()
            .fg(hex_to_color(&p.primary))
            .add_modifier(Modifier::BOLD)
    }
    pub fn header_model() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.accent))
    }
    pub fn header_session() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn header_status_ready() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.success))
    }
    pub fn header_status_busy() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.warning))
    }
    pub fn header_status_error() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(Color::Rgb(60, 20, 20))
            .fg(hex_to_color(&p.error))
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_message() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(Color::Rgb(45, 18, 18))
            .fg(hex_to_color(&p.error))
            .add_modifier(Modifier::BOLD)
    }

    pub fn error_prefix() -> Style {
        let p = Self::palette();
        Style::default()
            .fg(hex_to_color(&p.error))
            .add_modifier(Modifier::BOLD)
    }

    pub fn busy_button() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.warning))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn busy_button_shell() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.warning))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn user_prefix() -> Style {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    }
    pub fn assistant_prefix() -> Style {
        let p = Self::palette();
        Style::default()
            .fg(hex_to_color(&p.text))
            .add_modifier(Modifier::BOLD)
    }
    pub fn tool_prefix() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn system_prefix() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.info))
    }
    pub fn reasoning_prefix() -> Style {
        let cfg = Self::config();
        let p = Self::palette();
        let base = hex_to_color(&p.textMuted);

        let _ = cfg.thinkingOpacity;
        Style::default().fg(base).add_modifier(Modifier::ITALIC)
    }
    pub fn thinking_title_style() -> Style {
        let p = Self::palette();

        Style::default()
            .fg(hex_to_color(&p.textMuted))
            .add_modifier(Modifier::ITALIC)
    }
    pub fn thinking_icon_style() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.textMuted))
    }
    pub fn thinking_label_style() -> Style {
        let p = Self::palette();
        Style::default()
            .fg(hex_to_color(&p.textMuted))
            .add_modifier(Modifier::ITALIC)
    }

    pub fn card_border() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.borderSubtle))
    }
    pub fn card_title() -> Style {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    }
    pub fn chat_bg() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.background))
            .fg(hex_to_color(&p.text))
    }
    pub fn panel_bg() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundPanel))
            .fg(hex_to_color(&p.text))
    }
    pub fn input_border() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.borderActive))
    }
    pub fn input_border_busy() -> Style {
        let p = Self::palette();
        // textMuted, NOT borderSubtle: subtle == panel background on most
        // palettes, which painted the whole input box (and typed text)
        // invisible while busy.
        Style::default().fg(hex_to_color(&p.textMuted))
    }
    pub fn footer() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundPanel))
            .fg(hex_to_color(&p.textMuted))
    }
    pub fn popup_border() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.borderActive))
    }
    pub fn selection() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundElement))
            .fg(hex_to_color(&p.text))
    }

    pub fn code_block() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundElement))
            .fg(hex_to_color(&p.text))
    }

    pub fn comment() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundElement))
            .fg(hex_to_color(&p.textMuted))
            .add_modifier(Modifier::ITALIC)
    }

    pub fn list_marker() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.primary))
    }

    pub fn title_marker() -> Style {
        let p = Self::palette();
        Style::default()
            .fg(hex_to_color(&p.primary))
            .add_modifier(Modifier::BOLD)
    }

    pub fn text_selection() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.accent))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    }

    pub fn table_border() -> Style {
        let p = Self::palette();
        Style::default().fg(hex_to_color(&p.border))
    }

    pub fn inline_code() -> Style {
        let p = Self::palette();

        Style::default()
            .bg(hex_to_color(&p.backgroundElement))
            .fg(hex_to_color(&p.accent))
    }
    pub fn math() -> Style {
        let p = Self::palette();

        Style::default()
            .fg(hex_to_color(&p.accent))
            .add_modifier(Modifier::ITALIC)
    }

    pub fn code_lang_label() -> Style {
        let p = Self::palette();
        Style::default()
            .bg(hex_to_color(&p.backgroundElement))
            .fg(hex_to_color(&p.primary))
            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_input_style_stays_visible() {
        // Regression: the busy input box once vanished because it was
        // painted in borderSubtle, which equals the panel background.
        // It must use a real text color and differ from the background.
        let p = Theme::palette();
        assert_ne!(
            p.textMuted, p.backgroundPanel,
            "palette itself must keep muted text readable"
        );
        let border = Theme::input_border_busy();
        assert_eq!(
            border.fg,
            Some(hex_to_color(&p.textMuted)),
            "busy chrome uses muted text, never bg-matching subtle"
        );
        assert_ne!(border.fg, Theme::panel_bg().bg);
    }
}
