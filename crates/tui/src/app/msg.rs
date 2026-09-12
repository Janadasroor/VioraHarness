#[derive(Debug, Clone)]
pub(crate) struct QueuedPrompt {
    pub(crate) session_id: String,
    pub(crate) send: String,
    pub(crate) image: Option<(String, String)>,
    /// Deferred slash command: run via the command handler when idle
    /// instead of sending as a model prompt.
    pub(crate) slash: bool,
    /// Echo the prompt into chat when its turn starts. User-typed prompts
    /// echo (so display order always matches execution order); deferred
    /// commands and task follow-ups do not.
    pub(crate) echo: bool,
}

#[derive(Debug, Clone)]
pub enum Content {
    Text(String),
    Reasoning(String),
    ToolCall {
        id: String,
        name: String,
        args: String,
        status: ToolStatus,
    },
    ToolResult {
        id: String,
        content: String,
        ok: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub role: String,
    pub content: String,
    pub items: Vec<Content>,
    pub timestamp: String,
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingImage {
    pub(crate) b64: String,
    pub(crate) mime: &'static str,

    pub(crate) label: String,
}

impl Msg {
    pub(crate) fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        let ts = chrono_like_now();
        Self {
            role: role.into(),
            content: content.into(),
            items: Vec::new(),
            timestamp: ts,
            reasoning: None,
        }
    }
}

pub(crate) fn chrono_like_now() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "--:--".into())
}

pub(crate) fn new_session_id() -> String {
    format!(
        "{:010x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            & 0xffffffffff
    )
}

pub(crate) fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Popup {
    None,
    Help,
    Sessions,
    ModelPicker,
    Permissions,
    ThemePicker,
    Providers,
    Diff,
    PermissionAsk,
    Question,
    ToolOutput,
    Skills,
    Tasks,
    Errors,
    Rewind,
}
