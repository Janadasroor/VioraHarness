use crate::theme::Theme;
use base64::Engine as _;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::{
    backend::TestBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
struct QueuedPrompt {
    session_id: String,
    send: String,
    image: Option<(String, String)>,
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
struct PendingImage {
    b64: String,
    mime: &'static str,

    label: String,
}

const PASTE_CHIP_LINES: usize = 8;
const PASTE_CHIP_CHARS: usize = 600;

const CHAT_COLLAPSE_LINES: usize = 12;
const CHAT_COLLAPSE_HEAD: usize = 8;

fn paste_chip(id: usize, lines: usize) -> String {
    format!("[paste #{id} · {lines} lines]")
}

fn image_chip(label: &str) -> String {
    format!("[image · {label}]")
}

fn short_task_id(id: &str) -> String {
    let hex = id.strip_prefix("task_").unwrap_or(id);
    let chars: Vec<char> = hex.chars().collect();
    if chars.len() > 8 {
        chars[chars.len() - 8..].iter().collect()
    } else {
        hex.to_string()
    }
}

fn expand_paste_chips(prompt: &str, texts: &[(usize, String)]) -> String {
    let mut out = prompt.to_string();
    for (id, text) in texts {
        let chip = paste_chip(*id, text.lines().count());
        if out.contains(&chip) {
            out = out.replace(&chip, text);
        }
    }
    out
}

fn is_long_paste(txt: &str) -> bool {
    txt.lines().count() > PASTE_CHIP_LINES || txt.chars().count() > PASTE_CHIP_CHARS
}

fn image_ext_mime(ext: &str) -> Option<&'static str> {
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn detect_image_path(text: &str) -> Option<std::path::PathBuf> {
    let mut t = text.trim();
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            t = &t[1..t.len() - 1];
            break;
        }
    }
    let t = t.strip_prefix("file://").unwrap_or(t).trim();
    if t.is_empty() {
        return None;
    }
    let expanded = if let Some(rest) = t.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/{rest}")
    } else {
        t.to_string()
    };
    let path = std::path::PathBuf::from(&expanded);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    image_ext_mime(&ext)?;
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn load_image_file(path: &std::path::Path) -> Result<(String, &'static str, String), String> {
    const RAW_IMAGE_CAP: usize = 600_000;
    const BUDGET_B64: usize = 700_000;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image")
        .to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext == "bmp" {
        return Err(format!("{name}: BMP not supported — convert to PNG first"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: cannot read ({e})"))?;
    if ext == "png" {
        let (w, h, px) =
            rgba_from_png(&bytes).ok_or_else(|| format!("{name}: cannot decode PNG"))?;
        for max in [1024u32, 768, 512, 384] {
            let (w2, h2, small) = shrink_rgba(w, h, &px, max);
            if let Some(png) = png_from_rgba(w2, h2, &small) {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
                if b64.len() <= BUDGET_B64 {
                    return Ok((b64, "image/png", format!("{name} · {w2}×{h2}")));
                }
            }
        }
        return Err(format!("{name}: too large even downscaled to 384px"));
    }
    let Some(mime) = image_ext_mime(&ext) else {
        return Err(format!("{name}: unsupported image type"));
    };
    if bytes.len() > RAW_IMAGE_CAP {
        return Err(format!(
            "{name}: too large ({}KB, JPEG/GIF can't downscale — screenshot to PNG instead)",
            bytes.len() / 1024
        ));
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok((b64, mime, name))
}

fn collapse_long_content(content: &str, expanded: bool) -> (String, Option<usize>) {
    let total = content.lines().count();
    if expanded || total <= CHAT_COLLAPSE_LINES {
        return (content.to_string(), None);
    }
    let mut head: String = content
        .lines()
        .take(CHAT_COLLAPSE_HEAD)
        .collect::<Vec<_>>()
        .join("\n");
    if head.lines().filter(|l| l.trim().starts_with("```")).count() % 2 == 1 {
        head.push_str("\n```");
    }
    (head, Some(total))
}

impl Msg {
    fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
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

fn chrono_like_now() -> String {
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "--:--".into())
}

fn new_session_id() -> String {
    format!(
        "{:010x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            & 0xffffffffff
    )
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ToolVerbosity {
    Hidden,
    Quiet,
    Compact,
    Full,
}

fn parse_tool_verbosity(s: &str) -> ToolVerbosity {
    match s.trim().to_lowercase().as_str() {
        "hidden" | "off" | "none" => ToolVerbosity::Hidden,
        "quiet" | "name" | "minimal" => ToolVerbosity::Quiet,
        "full" | "verbose" | "detail" => ToolVerbosity::Full,
        _ => ToolVerbosity::Compact,
    }
}

fn tool_verbosity_name(v: ToolVerbosity) -> &'static str {
    match v {
        ToolVerbosity::Hidden => "hidden",
        ToolVerbosity::Quiet => "quiet",
        ToolVerbosity::Compact => "compact",
        ToolVerbosity::Full => "full",
    }
}

fn load_tool_display_config() -> (String, std::collections::HashMap<String, String>) {
    for cand in vioraharness_core::loop_mod::config_candidates() {
        if let Ok(s) = std::fs::read_to_string(&cand) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(td) = v.get("tui").and_then(|t| t.get("tool_display")) {
                    let default = td
                        .get("default")
                        .and_then(|d| d.as_str())
                        .unwrap_or("compact")
                        .to_string();
                    let mut map = std::collections::HashMap::new();
                    if let Some(obj) = td.as_object() {
                        for (k, vv) in obj {
                            if k == "default" {
                                continue;
                            }
                            if let Some(s) = vv.as_str() {
                                map.insert(k.clone(), s.to_string());
                            }
                        }
                    }
                    return (default, map);
                }
            }
        }
    }
    ("compact".into(), std::collections::HashMap::new())
}

fn home_prefix() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

fn rel_path(p: &str) -> String {
    let home = home_prefix();
    let viospice_abs = format!("{home}/qt_projects/viospice/");
    let mut s = p.to_string();
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if s.starts_with(&cwd_str) {
            let rel = s[cwd_str.len()..].trim_start_matches('/').to_string();
            s = if rel.is_empty() { ".".into() } else { rel };
        } else if s.starts_with(&viospice_abs) {
            s = s.replacen(&viospice_abs, "viospice/", 1);
        }
    }
    if s.starts_with(&format!("{home}/")) {
        s = s.replacen(&format!("{home}/"), "~/", 1);
    }
    if s.starts_with("~/qt_projects/viospice/") {
        s = s.replacen("~/qt_projects/viospice/", "viospice/", 1);
    }
    if s.len() > 150 {
        format!("…{}", &s[s.len() - 150..])
    } else {
        s
    }
}

fn rel_in_str(s: &str) -> String {
    let home = home_prefix();
    let viospice_abs = format!("{home}/qt_projects/viospice/");
    let mut out = s.to_string();
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if out.contains(&cwd_str) {
            out = out.replace(&cwd_str, ".");
        }
    }
    out.replace(&viospice_abs, "viospice/")
        .replace("~/qt_projects/viospice/", "viospice/")
        .replace(&format!("{home}/"), "~/")
}

fn summarize_todos(todos: &[serde_json::Value], done_override: Option<u64>) -> String {
    let done = done_override.unwrap_or_else(|| {
        todos
            .iter()
            .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("completed"))
            .count() as u64
    });
    let active = todos
        .iter()
        .find(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
        .and_then(|t| t.get("content").and_then(|c| c.as_str()))
        .map(|c| format!(" • now: {}", truncate_chars(c, 60)))
        .unwrap_or_default();
    format!("{}/{} todos done{active}", done, todos.len())
}

fn pretty_tool_args(name: &str, args: &str) -> String {
    pretty_tool_args_wide(name, args, false)
}

fn pretty_tool_args_wide(name: &str, args: &str, wide: bool) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
        match name {
            "read" | "write" => {
                if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
                    let mut out = rel_path(p);

                    if name == "write" {
                        if let Some(content) = v.get("content").and_then(|x| x.as_str()) {
                            let n = content.lines().count();
                            out = format!("{out} · {n} lines");
                            return out;
                        }
                    }
                    let off_s = if let Some(off) = v.get("offset").and_then(|x| x.as_u64()) {
                        if off != 0 {
                            off.to_string()
                        } else {
                            String::new()
                        }
                    } else if let Some(s) = v.get("offset").and_then(|x| x.as_str()) {
                        s.to_string()
                    } else {
                        String::new()
                    };
                    let lim_s = if let Some(limit) = v.get("limit") {
                        if let Some(s) = limit.as_str() {
                            s.to_string()
                        } else if let Some(n) = limit.as_u64() {
                            n.to_string()
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };
                    let off_ok = !off_s.is_empty() && off_s != "0";
                    let lim_ok = !lim_s.is_empty() && lim_s != "0";
                    if off_ok && lim_ok {
                        out = format!("{out} (offset {off_s}, limit {lim_s})");
                    } else if off_ok {
                        out = format!("{out} (offset {off_s})");
                    } else if lim_ok {
                        out = format!("{out} (limit {lim_s})");
                    }
                    return out;
                }
            }
            "bash" => {
                if let Some(cmd) = v.get("command").and_then(|x| x.as_str()) {
                    let short = rel_in_str(cmd);
                    let short = short
                        .replace("2>&1", "")
                        .replace("  ", " ")
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let short = short.trim().to_string();

                    let bg = if v
                        .get("background")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false)
                    {
                        " ↩ background"
                    } else {
                        ""
                    };
                    let cap = if wide { 600 } else { 160 };
                    if short.chars().count() > cap {
                        return format!("{}…{bg}", truncate_chars(&short, cap));
                    }
                    return format!("{short}{bg}");
                }
            }
            "edit" => {
                if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
                    let mut out = rel_path(p);
                    if v.get("replace_all")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false)
                    {
                        out.push_str(" · replace all");
                    }
                    return out;
                }
            }
            "todowrite" => {
                if let Some(todos) = v.get("todos").and_then(|x| x.as_array()) {
                    return summarize_todos(todos, None);
                }
            }
            "glob" => {
                let pat = v.get("pattern").and_then(|x| x.as_str()).unwrap_or("");
                let path = v
                    .get("path")
                    .or_else(|| v.get("base"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if !pat.is_empty() && !path.is_empty() {
                    return format!("{} in {}", pat, rel_path(path));
                } else if !pat.is_empty() {
                    return pat.to_string();
                }
            }
            "grep" => {
                let pat = v.get("pattern").and_then(|x| x.as_str()).unwrap_or("");
                let inc = v
                    .get("include")
                    .or_else(|| v.get("glob"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                if !pat.is_empty() && !inc.is_empty() {
                    return format!("{pat} · {inc}");
                } else if !pat.is_empty() {
                    return pat.to_string();
                }
            }
            _ => {}
        }

        if let Some(obj) = v.as_object() {
            for key in [
                "path", "file", "pattern", "query", "command", "tool", "input", "name",
            ] {
                if let Some(val) = obj.get(key).and_then(|x| x.as_str()) {
                    let rel = rel_in_str(val);
                    let cap = if wide { 600 } else { 180 };
                    let trunc = if rel.chars().count() > cap {
                        format!("{}…", truncate_chars(&rel, cap))
                    } else {
                        rel
                    };

                    if name == "viora" && key == "tool" {
                        return trunc;
                    }
                    if obj.len() == 1 {
                        return trunc;
                    }

                    let mut parts = Vec::new();
                    let (part_cap, part_max) = if wide { (200, 4) } else { (80, 2) };
                    for (k, vv) in obj.iter() {
                        let vs = if let Some(s) = vv.as_str() {
                            rel_in_str(s)
                        } else {
                            vv.to_string()
                        };
                        let vs = if vs.chars().count() > part_cap {
                            format!("{}…", truncate_chars(&vs, part_cap))
                        } else {
                            vs
                        };
                        parts.push(format!("{k}={vs}"));
                        if parts.len() >= part_max {
                            break;
                        }
                    }
                    return parts.join(" ");
                }
            }

            let compact = args
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}')
                .trim()
                .to_string();
            let rel = rel_in_str(&compact);
            let rel = rel.replace("\"", "").replace(":", " ").replace(",", " ");
            let t = rel.split_whitespace().collect::<Vec<_>>().join(" ");
            let cap = if wide { 900 } else { 240 };
            return if t.chars().count() > cap {
                format!("{}…", truncate_chars(&t, cap))
            } else {
                t
            };
        }
    }

    let rel = rel_in_str(args);
    let t = rel
        .trim()
        .trim_matches(|c| c == '{' || c == '}' || c == '"')
        .to_string();
    let cap = if wide { 900 } else { 240 };
    if t.chars().count() > cap {
        format!("{}…", truncate_chars(&t, cap))
    } else if t.is_empty() {
        String::new()
    } else {
        t
    }
}

fn pretty_tool_result_wide(content: &str, wide: bool) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(obj) = v.as_object() {
            if let Some(files) = obj.get("files").and_then(|x| x.as_array()) {
                let count = files.len();
                let sample: Vec<String> = files
                    .iter()
                    .take(4)
                    .filter_map(|x| x.as_str().map(rel_path))
                    .collect();
                let sample_str = if sample.is_empty() {
                    "".into()
                } else {
                    format!(": {}", sample.join(", "))
                };
                let more = if count > 4 {
                    format!(" +{} more", count - 4)
                } else {
                    "".into()
                };
                return format!("{count} files{sample_str}{more}");
            }

            if let Some(c) = obj.get("content").and_then(|x| x.as_str()) {
                let path = obj
                    .get("path")
                    .and_then(|x| x.as_str())
                    .map(rel_path)
                    .unwrap_or_default();
                let off = obj.get("offset").and_then(|x| x.as_u64()).unwrap_or(0);
                let lim = obj.get("limit").and_then(|x| x.as_u64()).unwrap_or(0);
                let total = obj.get("total_lines").and_then(|x| x.as_u64()).unwrap_or(0);
                let range = if off != 0 || lim != 0 {
                    let lim_s = if lim != 0 {
                        format!(", limit {lim}")
                    } else {
                        "".into()
                    };
                    let off_s = if off != 0 {
                        format!("offset {off}")
                    } else {
                        "start".into()
                    };
                    format!(
                        " ({off_s}{lim_s}{})",
                        if total != 0 {
                            format!(", total {total}")
                        } else {
                            "".into()
                        }
                    )
                } else if total != 0 {
                    format!(" (total {total})")
                } else {
                    "".into()
                };
                if total != 0 || off != 0 || lim != 0 {
                    if !path.is_empty() {
                        return format!("{path}{range}");
                    }
                    let shown = c.lines().count();
                    return format!("{shown} lines{range}");
                }
                let first = c
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect::<String>();
                if !path.is_empty() {
                    return format!("{}{} — {}", path, range, first);
                }
                return format!("{}{}", truncate_chars(first.trim(), 120), range);
            }

            if obj.contains_key("stdout") || obj.contains_key("code") {
                if let Some(err) = obj.get("error").and_then(|x| x.as_str()) {
                    if !err.is_empty() {
                        return truncate_chars(err, 200);
                    }
                }
                let out = obj.get("stdout").and_then(|x| x.as_str()).unwrap_or("");
                let err = obj.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
                let combined = if !err.is_empty() {
                    format!("{out}\n{err}")
                } else {
                    out.to_string()
                };
                let total_lines = combined.lines().count();
                let non_empty: Vec<&str> =
                    combined.lines().filter(|l| !l.trim().is_empty()).collect();
                if non_empty.is_empty() {
                    let code = obj.get("code").and_then(|x| x.as_i64()).unwrap_or(0);
                    return if code == 0 {
                        "ok".into()
                    } else {
                        format!("exit {code}")
                    };
                }

                let shown_lines = non_empty
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" | ");
                let mut preview = truncate_chars(shown_lines.trim(), 200);
                let log_path = obj.get("log").and_then(|x| x.as_str()).unwrap_or("");
                let log_hint = if !log_path.is_empty() {
                    format!(" log: {}", rel_path(log_path))
                } else {
                    String::new()
                };

                let is_truncated = content.contains("...(truncated)")
                    || combined.len() > 7000
                    || total_lines > 3
                    || !log_path.is_empty()
                        && (obj
                            .get("truncated")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false)
                            || combined.len() > 5000);
                if is_truncated {
                    let remaining = total_lines.saturating_sub(3);
                    if remaining > 0 {
                        preview = format!(
                            "{preview} … (+{} lines, {} chars total — press V for full{} )",
                            remaining,
                            combined.len(),
                            log_hint
                        );
                    } else {
                        preview =
                            format!("{preview} … (truncated — press V for full{} )", log_hint);
                    }
                } else if total_lines > 1 {
                    preview = format!("{preview} ({} lines{})", total_lines, log_hint);
                } else if !log_hint.is_empty() {
                    preview = format!("{preview}{}", log_hint);
                }
                return preview;
            }

            if let Some(e) = obj.get("error").and_then(|x| x.as_str()) {
                return truncate_chars(e, 200);
            }

            if let Some(todos) = obj.get("todos").and_then(|x| x.as_array()) {
                let done = obj.get("done").and_then(|x| x.as_u64());
                return summarize_todos(todos, done);
            }

            if obj.contains_key("base64") || obj.contains_key("base64_len") {
                if let Some(p) = obj.get("path").and_then(|x| x.as_str()) {
                    let path = rel_path(p);
                    let dims = match (
                        obj.get("width").and_then(|x| x.as_u64()),
                        obj.get("height").and_then(|x| x.as_u64()),
                    ) {
                        (Some(w), Some(h)) if w > 0 && h > 0 => format!(" · {w}×{h}"),
                        _ => String::new(),
                    };
                    let size = obj
                        .get("bytes")
                        .and_then(|x| x.as_u64())
                        .map(|b| format!(" · {}KB", b / 1024))
                        .unwrap_or_default();
                    return format!("{path}{dims}{size} → vision");
                }
            }

            if obj
                .get("background")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                if let Some(tid) = obj.get("task_id").and_then(|x| x.as_str()) {
                    let tid = short_task_id(tid);
                    let status = obj
                        .get("status")
                        .and_then(|x| x.as_str())
                        .unwrap_or("running");
                    let log = obj
                        .get("log")
                        .and_then(|x| x.as_str())
                        .map(rel_path)
                        .unwrap_or_default();
                    return if log.is_empty() {
                        format!("task {tid} · {status}")
                    } else {
                        format!("task {tid} · {status} — log {log}")
                    };
                }
            }

            if obj.contains_key("new_lines") || obj.contains_key("bytes") {
                if let Some(p) = obj.get("path").and_then(|x| x.as_str()) {
                    let path = rel_path(p);
                    if let Some(note) = obj.get("note").and_then(|x| x.as_str()) {
                        return format!("{path} · {note}");
                    }
                    let lines = match (
                        obj.get("old_lines").and_then(|x| x.as_u64()),
                        obj.get("new_lines").and_then(|x| x.as_u64()),
                    ) {
                        (Some(o), Some(n)) => format!(" · {o}→{n} lines"),
                        (None, Some(n)) => format!(" · {n} lines"),
                        _ => String::new(),
                    };
                    let bytes = obj
                        .get("bytes")
                        .and_then(|x| x.as_u64())
                        .map(|b| format!(" · {b} bytes"))
                        .unwrap_or_default();
                    let repl = obj
                        .get("replacements")
                        .and_then(|x| x.as_u64())
                        .filter(|&r| r > 1)
                        .map(|r| format!(" · {r} replacements"))
                        .unwrap_or_default();
                    return format!("{path}{lines}{bytes}{repl}");
                }
            }

            if let Some(answers) = obj.get("answers").and_then(|x| x.as_array()) {
                let picks: Vec<String> = answers
                    .iter()
                    .flat_map(|a| {
                        let mut v: Vec<String> = a
                            .get("selected")
                            .and_then(|s| s.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Some(c) = a.get("custom").and_then(|x| x.as_str()) {
                            v.push(format!("“{c}”"));
                        }
                        v
                    })
                    .collect();
                if !picks.is_empty() {
                    return format!("answered: {}", truncate_chars(&picks.join(", "), 160));
                }
                return "answered".into();
            }

            if let Some(ok) = obj.get("ok") {
                let ok_str = if ok.as_bool().unwrap_or(true) {
                    "ok"
                } else {
                    "error"
                };

                for (k, vv) in obj.iter() {
                    if k == "ok" || k == "truncated" {
                        continue;
                    }
                    if let Some(s) = vv.as_str() {
                        return format!("{ok_str} — {}: {}", k, truncate_chars(s, 120));
                    }
                    if let Some(arr) = vv.as_array() {
                        return format!("{ok_str} — {}: {} items", k, arr.len());
                    }
                }
                return ok_str.to_string();
            }
        }

        let s = v.to_string();
        let cap = if wide { 900 } else { 200 };
        return if s.chars().count() > cap {
            format!("{}…", truncate_chars(&s, cap))
        } else {
            s
        };
    }

    let salvaged = salvage_broken_result(content);
    if !salvaged.is_empty() {
        return salvaged;
    }
    let t = rel_in_str(content);
    let cap = if wide { 900 } else { 200 };
    if t.chars().count() > cap {
        format!("{}…", truncate_chars(&t, cap))
    } else {
        t
    }
}

fn scan_json_str(hay: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let mut search = hay;
    loop {
        let i = search.find(&pat)?;
        let mut rest = search[i + pat.len()..].trim_start();
        if !rest.starts_with(':') {
            search = &search[i + pat.len()..];
            continue;
        }
        rest = rest[1..].trim_start();
        if !rest.starts_with('"') {
            search = &search[i + pat.len()..];
            continue;
        }

        let mut out = String::new();
        let mut chars = rest[1..].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    None => break,
                    Some('n') => out.push_str(" | "),
                    Some('t') => out.push(' '),
                    Some('r') => {}
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).collect();
                        if hex.len() == 4 {
                            if let Ok(n) = u32::from_str_radix(&hex, 16) {
                                out.push(char::from_u32(n).unwrap_or('?'));
                                continue;
                            }
                        }
                        out.push('?');
                    }
                    Some(c) => out.push(c),
                },
                '"' => break,
                c => out.push(c),
            }
            if out.len() > 400 {
                break;
            }
        }

        let tidy = out.split_whitespace().collect::<Vec<_>>().join(" ");
        return Some(tidy);
    }
}

fn scan_json_num(hay: &str, key: &str) -> Option<u64> {
    let pat = format!("\"{key}\"");
    let mut search = hay;
    loop {
        let i = search.find(&pat)?;
        let mut rest = search[i + pat.len()..].trim_start();
        if !rest.starts_with(':') {
            search = &search[i + pat.len()..];
            continue;
        }
        rest = rest[1..].trim_start();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            search = &search[i + pat.len()..];
            continue;
        }
        return digits.parse().ok();
    }
}

fn diff_card_lines(diff: &str, max: usize) -> (Vec<(bool, String)>, usize) {
    let mut shown = Vec::new();
    let mut total = 0usize;
    for raw in diff.lines() {
        let (is_add, body) = match raw.chars().next() {
            Some('+') if !raw.starts_with("+++") => (true, &raw[1..]),
            Some('-') if !raw.starts_with("---") => (false, &raw[1..]),
            _ => continue,
        };
        total += 1;
        if shown.len() < max {
            let t = body.trim();
            let t = if t.chars().count() > 110 {
                format!("{}…", truncate_chars(t, 110))
            } else {
                t.to_string()
            };
            shown.push((is_add, t));
        }
    }
    let rest = total.saturating_sub(shown.len());
    (shown, rest)
}

fn result_diff_text(content: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(d) = v
            .get("diff_preview")
            .or_else(|| v.get("diff"))
            .and_then(|x| x.as_str())
        {
            return Some(d.to_string());
        }
    }
    scan_json_str(content, "diff_preview").filter(|s| !s.is_empty())
}

fn salvage_broken_result(content: &str) -> String {
    if let Some(e) = scan_json_str(content, "error") {
        if !e.is_empty() {
            return truncate_chars(&e, 200);
        }
    }

    if let Some(p) = scan_json_str(content, "path") {
        if content.contains("\"new_lines\"") || content.contains("\"bytes\"") {
            let path = rel_path(&p);
            let lines = match (
                scan_json_num(content, "old_lines"),
                scan_json_num(content, "new_lines"),
            ) {
                (Some(o), Some(n)) => format!(" · {o}→{n} lines"),
                (None, Some(n)) => format!(" · {n} lines"),
                _ => String::new(),
            };
            let bytes = scan_json_num(content, "bytes")
                .map(|b| format!(" · {b} bytes"))
                .unwrap_or_default();
            return format!("{path}{lines}{bytes}");
        }

        if content.contains("\"content\"") {
            if let Some(c) = scan_json_str(content, "content") {
                let head = truncate_chars(c.trim(), 120);
                return format!("{} — {}", rel_path(&p), head);
            }
            return rel_path(&p);
        }
    }

    if content.contains("\"stdout\"") {
        if let Some(s) = scan_json_str(content, "stdout") {
            let head = s
                .split(" | ")
                .filter(|l| !l.trim().is_empty())
                .take(2)
                .collect::<Vec<_>>()
                .join(" | ");
            if head.is_empty() {
                return "ok".into();
            }
            return format!("{}… (truncated)", truncate_chars(head.trim(), 180));
        }
    }

    if content.contains("\"todos\"") {
        let total = content.matches("\"content\"").count();
        let done = content.matches("\"completed\"").count();
        if total > 0 {
            return format!("{done}/{total} todos done");
        }
    }
    String::new()
}

fn latex_symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ε",
        "varepsilon" => "ɛ",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "omicron" => "ο",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "φ",
        "varphi" => "ϕ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "infty" => "∞",
        "leq" => "≤",
        "le" => "≤",
        "geq" => "≥",
        "ge" => "≥",
        "neq" => "≠",
        "ne" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "times" => "×",
        "cdot" => "·",
        "pm" => "±",
        "mp" => "∓",
        "div" => "÷",
        "sum" => "∑",
        "int" => "∫",
        "oint" => "∮",
        "prod" => "∏",
        "coprod" => "∐",
        "sqrt" => "√",
        "partial" => "∂",
        "nabla" => "∇",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "to" => "→",
        "rightarrow" => "→",
        "leftarrow" => "←",
        "gets" => "←",
        "leftrightarrow" => "↔",
        "Rightarrow" => "⇒",
        "Leftarrow" => "⇐",
        "implies" => "⟹",
        "ldots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "forall" => "∀",
        "exists" => "∃",
        "neg" => "¬",
        "lnot" => "¬",
        "land" => "∧",
        "lor" => "∨",
        "top" => "⊤",
        "bot" => "⊥",
        "angle" => "∠",
        "perp" => "⊥",
        "parallel" => "∥",
        "sim" => "∼",
        "cong" => "≅",
        "propto" => "∝",
        "hbar" => "ħ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "emptyset" => "∅",
        "circ" => "∘",
        "bullet" => "•",
        "degree" => "°",
        "prime" => "′",

        "quad" => "  ",
        "qquad" => "    ",
        _ => return None,
    })
}

fn blackboard(ch: char) -> Option<char> {
    Some(match ch {
        'R' => 'ℝ',
        'N' => 'ℕ',
        'Z' => 'ℤ',
        'Q' => 'ℚ',
        'C' => 'ℂ',
        'H' => 'ℍ',
        'P' => 'ℙ',
        'E' => '𝔼',
        'F' => '𝔽',
        _ => return None,
    })
}

fn translate_latex(s: &str) -> String {
    fn braced(chars: &[char], i: &mut usize) -> Option<String> {
        if chars.get(*i) != Some(&'{') {
            return None;
        }
        *i += 1;
        let mut depth = 1;
        let start = *i;
        while *i < chars.len() {
            match chars[*i] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let inner: String = chars[start..*i].iter().collect();
                        *i += 1;
                        return Some(translate_latex(&inner));
                    }
                }
                _ => {}
            }
            *i += 1;
        }
        None
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            if let Some(inner) = braced(&chars, &mut i) {
                out.push_str(&inner);
            } else {
                break;
            }
            continue;
        }
        if c == '}' {
            i += 1;
            continue;
        }
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }

        i += 1;
        let Some(&n) = chars.get(i) else {
            out.push('\\');
            break;
        };
        if !n.is_ascii_alphabetic() {
            match n {
                '\\' => out.push('\n'),

                ' ' | ',' | ';' | ':' => out.push(' '),

                '!' => {}

                other => out.push(other),
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_alphabetic() {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        if name == "left" || name == "right" {
            continue;
        }
        if name == "frac" {
            let a = braced(&chars, &mut i);
            let b = braced(&chars, &mut i);
            match (a, b) {
                (Some(a), Some(b)) => {
                    out.push_str(&format!("({a})/({b})"));
                }
                _ => out.push_str("frac"),
            }
            continue;
        }
        if name == "sqrt" {
            if let Some(x) = braced(&chars, &mut i) {
                out.push_str(&format!("√({x})"));
            } else {
                out.push('√');
            }
            continue;
        }
        if name == "text" || name == "mathrm" || name == "mathbf" || name == "mathit" {
            if let Some(x) = braced(&chars, &mut i) {
                out.push_str(&x);
            }
            continue;
        }
        if name == "mathbb" {
            if let Some(x) = braced(&chars, &mut i) {
                let mapped: String = x.chars().map(|ch| blackboard(ch).unwrap_or(ch)).collect();
                out.push_str(&mapped);
            }
            continue;
        }
        if name == "hspace" {
            let _ = braced(&chars, &mut i);
            out.push(' ');
            continue;
        }
        if name == "vspace" {
            let _ = braced(&chars, &mut i);
            out.push('\n');
            continue;
        }
        if let Some(sym) = latex_symbol(&name) {
            out.push_str(sym);
            continue;
        }

        if let Some(x) = braced(&chars, &mut i) {
            out.push_str(&x);
        } else {
            out.push_str(&name);
        }
    }
    out
}

fn translate_bare_latex_chunk(s: &str) -> String {
    fn braced_arg(chars: &[char], i: &mut usize) -> Option<String> {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
        if chars.get(*i) == Some(&'{') {
            *i += 1;
            let mut depth = 1;
            let start = *i;
            while *i < chars.len() {
                match chars[*i] {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            let inner: String = chars[start..*i].iter().collect();
                            *i += 1;
                            return Some(translate_bare_latex_chunk(&inner));
                        }
                    }
                    _ => {}
                }
                *i += 1;
            }
            return None;
        }

        chars.get(*i).map(|c| {
            *i += 1;
            c.to_string()
        })
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < chars.len() && chars[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == i + 1 {
            match chars.get(j) {
                None => {
                    out.push('\\');
                    i += 1;
                }

                Some('\\') => {
                    out.push_str("\\\\");
                    i = j + 1;
                }
                Some(',') | Some(';') | Some(':') | Some(' ') => {
                    out.push(' ');
                    i = j + 1;
                }

                Some('!') => {
                    i = j + 1;
                }

                Some(c) => {
                    out.push(*c);
                    i = j + 1;
                }
            }
            continue;
        }
        let name: String = chars[i + 1..j].iter().collect();
        if name == "left" || name == "right" {
            i = j;
            continue;
        }
        if name == "frac" {
            let mut k = j;
            let a = braced_arg(&chars, &mut k);
            let b = braced_arg(&chars, &mut k);
            if let (Some(a), Some(b)) = (a, b) {
                out.push_str(&format!("({a})/({b})"));
                i = k;
            } else {
                out.push_str("\\frac");
                i = j;
            }
            continue;
        }
        if name == "sqrt" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                out.push_str(&format!("√({x})"));
                i = k;
            } else {
                out.push_str("\\sqrt");
                i = j;
            }
            continue;
        }
        if name == "text" || name == "mathrm" || name == "mathbf" || name == "mathit" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                out.push_str(&x);
                i = k;
            } else {
                out.push('\\');
                out.push_str(&name);
                i = j;
            }
            continue;
        }
        if name == "mathbb" {
            let mut k = j;
            if let Some(x) = braced_arg(&chars, &mut k) {
                let mapped: String = x.chars().map(|ch| blackboard(ch).unwrap_or(ch)).collect();
                out.push_str(&mapped);
                i = k;
            } else {
                out.push_str("\\mathbb");
                i = j;
            }
            continue;
        }
        if name == "hspace" {
            let mut k = j;
            if braced_arg(&chars, &mut k).is_some() {
                out.push(' ');
                i = k;
            } else {
                out.push_str("\\hspace");
                i = j;
            }
            continue;
        }
        if name == "vspace" {
            let mut k = j;
            if braced_arg(&chars, &mut k).is_some() {
                i = k;
            } else {
                out.push_str("\\vspace");
                i = j;
            }
            continue;
        }
        if let Some(sym) = latex_symbol(&name) {
            out.push_str(sym);
            i = j;
            continue;
        }

        out.push('\\');
        out.push_str(&name);
        i = j;
    }
    out
}

fn markdown_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let s = text;
    let mut i = 0;
    let bytes = s.as_bytes();
    let len = s.len();

    let push_plain = |out: &mut Vec<Span<'static>>, chunk: &str| {
        if !chunk.is_empty() {
            out.push(Span::raw(translate_bare_latex_chunk(chunk)));
        }
    };
    while i < len {
        if bytes[i] == b'`' {
            if let Some(end) = s[i + 1..].find('`') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        crate::theme::Theme::inline_code(),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        if i + 1 < len && bytes[i] == b'$' && bytes[i + 1] == b'$' {
            if let Some(end) = s[i + 2..].find("$$") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        translate_latex(inner),
                        crate::theme::Theme::math(),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'$' {
            let escaped = i > 0 && bytes[i - 1] == b'\\';
            let after = s[i + 1..].chars().next();
            if !escaped && matches!(after, Some(c) if !c.is_whitespace() && c != '$') {
                let rest = &s[i + 1..];
                let rb = rest.as_bytes();
                let mut j = 0;
                let mut found = None;
                while j < rb.len() {
                    if rb[j] == b'\n' {
                        break;
                    }
                    if rb[j] == b'$'
                        && j > 0
                        && !(rb[j - 1] as char).is_whitespace()
                        && rb[j - 1] != b'\\'
                    {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(j) = found {
                    let inner = &rest[..j];
                    if !inner.is_empty() && !inner.contains('$') {
                        out.push(Span::styled(
                            translate_latex(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 1 + j + 1;
                        continue;
                    }
                }
            }
        }
        if bytes[i] == b'\\' {
            if i + 1 < len && bytes[i + 1] == b'\\' {
                push_plain(&mut out, &s[i..i + 2]);
                i += 2;
                continue;
            }
            if i + 1 < len && bytes[i + 1] == b'(' {
                if let Some(end) = s[i + 2..].find("\\)") {
                    let inner = &s[i + 2..i + 2 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        out.push(Span::styled(
                            translate_latex(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 2 + end + 2;
                        continue;
                    }
                }
            }

            if i + 1 < len && bytes[i + 1] == b'[' {
                if let Some(end) = s[i + 2..].find("\\]") {
                    let inner = &s[i + 2..i + 2 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        out.push(Span::styled(
                            translate_latex(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end) = s[i + 2..].find("**") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if i + 1 < len && bytes[i] == b'_' && bytes[i + 1] == b'_' {
            if let Some(end) = s[i + 2..].find("__") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'*' {
            if let Some(end) = s[i + 1..].find('*') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains("**") {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }
        if bytes[i] == b'_' {
            if let Some(end) = s[i + 1..].find('_') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains("__") {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        let next_markers = ["**", "__", "`", "*", "_", "$$", "$", "\\(", "\\["]
            .iter()
            .filter_map(|m| s[i..].find(m).map(|p| p + i))
            .min()
            .unwrap_or(len);
        let end = if next_markers == i {
            i + 1
        } else {
            next_markers
        };

        if end == i {
            push_plain(&mut out, &s[i..i + 1]);
            i += 1;
        } else {
            push_plain(&mut out, &s[i..end]);
            i = end;
        }
    }
    if out.is_empty() {
        out.push(Span::raw(text.to_string()));
    }
    out
}

fn markdown_header_style(level: usize) -> Style {
    match level {
        1 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        2 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    }
}

fn code_lang_from_fence(line: &str) -> String {
    let t = line.trim();
    if t.starts_with("```") {
        let lang = t.trim_start_matches("```").trim().to_lowercase();

        lang.split_whitespace().next().unwrap_or("").to_string()
    } else {
        String::new()
    }
}

const FENCE_WIDTH: usize = 34;

fn fence_open_spans(lang: &str) -> Vec<Span<'static>> {
    let label = if lang.is_empty() {
        "code".to_string()
    } else {
        lang.to_string()
    };
    let mut spans = vec![Span::styled("┌─ ", Theme::code_block())];
    spans.push(Span::styled(format!("{label} "), Theme::code_lang_label()));
    let dashes = FENCE_WIDTH.saturating_sub(4 + label.len());
    spans.push(Span::styled("─".repeat(dashes), Theme::code_block()));
    spans
}

fn fence_close_spans() -> Vec<Span<'static>> {
    vec![Span::styled(
        format!("└{}", "─".repeat(FENCE_WIDTH - 1)),
        Theme::code_block(),
    )]
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TableAlign {
    Left,
    Center,
    Right,
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let mut s = line.trim();
    if !s.contains('|') {
        return None;
    }
    if s.starts_with('|') {
        s = &s[1..];
    }
    if s.ends_with('|') && !(s.ends_with("\\|") && !s.ends_with("\\\\|")) {
        s = &s[..s.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('|') => {
                    cur.push('|');
                    chars.next();
                }
                _ => cur.push(c),
            }
        } else if c == '|' {
            cells.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    Some(cells)
}

fn parse_delim_cell(cell: &str) -> Option<TableAlign> {
    let c = cell.trim();
    if c.is_empty() {
        return None;
    }
    let left = c.starts_with(':');
    let right = c.ends_with(':');
    let inner = c.trim_matches(':');
    if inner.is_empty() || !inner.chars().all(|x| x == '-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => TableAlign::Center,
        (false, true) => TableAlign::Right,
        _ => TableAlign::Left,
    })
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

fn split_list_marker(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    for m in ["- ", "* ", "• "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some((m.trim_end().to_string(), rest.to_string()));
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(rest) = t[digits.len()..].strip_prefix(". ") {
            return Some((format!("{digits}."), rest.to_string()));
        }
    }
    None
}

fn is_section_title(rest: &str) -> bool {
    let t = rest.trim_start();
    t.starts_with("**") && t[2..].contains("**")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelPos {
    line: usize,
    col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Selection {
    anchor: SelPos,
    cursor: SelPos,
    session: String,
    msg_len: usize,
}

impl Selection {
    fn ordered(&self) -> (SelPos, SelPos) {
        if (self.cursor.line, self.cursor.col) < (self.anchor.line, self.anchor.col) {
            (self.cursor, self.anchor)
        } else {
            (self.anchor, self.cursor)
        }
    }

    fn is_caret(&self) -> bool {
        self.anchor == self.cursor
    }
}

fn highlight_range(
    spans: &[Span],
    start_col: usize,
    end_col: usize,
    sel_style: Style,
) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for s in spans {
        let mut before = String::new();
        let mut middle = String::new();
        let mut after = String::new();
        for c in s.content.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            let span = if col + w <= start_col || col >= end_col {
                if col < start_col {
                    &mut before
                } else {
                    &mut after
                }
            } else {
                &mut middle
            };
            span.push(c);
            col += w;
        }
        if !before.is_empty() {
            out.push(Span::styled(before, s.style));
        }
        if !middle.is_empty() {
            out.push(Span::styled(middle, s.style.patch(sel_style)));
        }
        if !after.is_empty() {
            out.push(Span::styled(after, s.style));
        }
    }
    if out.is_empty() {
        out.push(Span::raw(""));
    }
    out
}

fn extract_selection_text(lines: &[Line], top: SelPos, bottom: SelPos) -> String {
    let mut parts = Vec::new();
    if lines.is_empty() {
        return String::new();
    }
    let end = bottom.line.min(lines.len().saturating_sub(1));
    for (li, line) in lines.iter().enumerate().skip(top.line) {
        if li > end {
            break;
        }
        let spans = &line.spans;
        let s = if li == top.line { top.col } else { 0 };
        let e = if li == bottom.line {
            bottom.col
        } else {
            usize::MAX
        };
        if s < e {
            parts.push(extract_range_text(spans, s, e).trim_end().to_string());
        } else {
            parts.push(String::new());
        }
    }
    parts.join("\n")
}

fn extract_range_text(spans: &[Span], start_col: usize, end_col: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut col = 0;
    for s in spans {
        for c in s.content.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            if col + w > start_col && col < end_col {
                out.push(c);
            }
            col += w;
        }
    }
    out
}

fn chat_line(
    head: Vec<Span<'static>>,
    mut body: Vec<Span<'static>>,
    tail: Vec<Span<'static>>,
) -> Line<'static> {
    let mut full = head;
    full.append(&mut body);
    full.extend(tail);
    let text: String = full.iter().map(|s| s.content.as_ref()).collect();
    if text.trim().is_empty() {
        Line::default()
    } else {
        Line::from(full)
    }
}

fn push_cell_char(row: &mut Vec<Span<'static>>, row_w: &mut usize, c: char, st: Style) {
    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    if let Some(last) = row.last_mut() {
        if last.style == st {
            last.content.to_mut().push(c);
            *row_w += cw;
            return;
        }
    }
    row.push(Span::styled(c.to_string(), st));
    *row_w += cw;
}

fn wrap_spans(
    segs: Vec<Span<'static>>,
    first_head: Vec<Span<'static>>,
    cont_head: Vec<Span<'static>>,
    tail: Vec<Span<'static>>,
    width: usize,
) -> Vec<Line<'static>> {
    let head_w = spans_width(&first_head);
    let cont_w = spans_width(&cont_head);

    let mut cells: Vec<(char, Style)> = Vec::new();
    for s in &segs {
        for c in s.content.chars() {
            cells.push((c, s.style));
        }
    }

    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for cell in cells {
        if cell.0.is_whitespace() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(cell);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let word_width = |w: &[(char, Style)]| -> usize {
        w.iter()
            .map(|(c, _)| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
            .sum()
    };

    let max_avail = width.saturating_sub(head_w.min(cont_w)).max(8);
    let mut norm: Vec<Vec<(char, Style)>> = Vec::new();
    for w in words {
        if word_width(&w) <= max_avail || w.len() <= 1 {
            norm.push(w);
        } else {
            let mut chunk: Vec<(char, Style)> = Vec::new();
            let mut cw = 0;
            for cell in w {
                let cwid = unicode_width::UnicodeWidthChar::width(cell.0).unwrap_or(0);
                if cw + cwid > max_avail && !chunk.is_empty() {
                    norm.push(std::mem::take(&mut chunk));
                    cw = 0;
                }
                chunk.push(cell);
                cw += cwid;
            }
            if !chunk.is_empty() {
                norm.push(chunk);
            }
        }
    }
    if norm.is_empty() {
        return vec![chat_line(first_head, Vec::new(), tail)];
    }
    let mut rows: Vec<Line> = Vec::new();
    let mut row: Vec<Span> = Vec::new();
    let mut row_w = 0;
    let mut avail = width.saturating_sub(head_w).max(8);
    let mut first_row = true;
    let mut wi = 0;
    while wi < norm.len() {
        let ww = word_width(&norm[wi]);

        let need = ww + usize::from(!row.is_empty());
        if row_w + need <= avail {
            if !row.is_empty() {
                push_cell_char(&mut row, &mut row_w, ' ', Style::default());
            }
            for (c, st) in &norm[wi] {
                push_cell_char(&mut row, &mut row_w, *c, *st);
            }
            wi += 1;
        } else if row_w == 0 {
            for (c, st) in &norm[wi] {
                push_cell_char(&mut row, &mut row_w, *c, *st);
            }
            wi += 1;
        } else {
            let head = if first_row {
                first_head.clone()
            } else {
                cont_head.to_vec()
            };
            rows.push(chat_line(head, std::mem::take(&mut row), Vec::new()));
            first_row = false;
            avail = width.saturating_sub(cont_w).max(8);
            row_w = 0;
        }
    }
    let head = if first_row {
        first_head
    } else {
        cont_head.to_vec()
    };

    rows.push(chat_line(head, row, tail));
    rows
}

fn wrap_line_preserving(line: &Line<'static>, width: usize) -> Vec<(Vec<Span<'static>>, usize)> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let cell_w = |c: char| UnicodeWidthChar::width(c).unwrap_or(0);

    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
        .collect();

    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut in_space = cells.first().is_some_and(|(c, _)| *c == ' ' || *c == '\t');
    for cell in cells {
        let ws = cell.0 == ' ' || cell.0 == '\t';
        if ws != in_space && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
            in_space = ws;
        }
        cur.push(cell);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let word_width = |w: &[(char, Style)]| -> usize {
        w.iter()
            .map(|(c, _)| if *c == '\t' { 4 } else { cell_w(*c) })
            .sum()
    };
    let mut rows: Vec<(Vec<Span<'static>>, usize)> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_w: usize = 0;
    let mut consumed: usize = 0;
    let mut wi = 0;

    let mut flush_row = |row: &mut Vec<Span<'static>>, row_w: &mut usize, consumed: &mut usize| {
        rows.push((std::mem::take(row), *consumed));
        *consumed += *row_w;
        *row_w = 0;
    };
    while wi < words.len() {
        let ww = word_width(&words[wi]);
        if row_w + ww <= width {
            for (c, st) in &words[wi] {
                if *c == '\t' {
                    let pad = 4 - (row_w % 4);
                    for _ in 0..pad {
                        push_cell_char(&mut row, &mut row_w, ' ', *st);
                    }
                } else {
                    push_cell_char(&mut row, &mut row_w, *c, *st);
                }
            }
            wi += 1;
        } else if row_w == 0 {
            let mut placed_any = false;
            let mut ci = 0;
            while ci < words[wi].len() {
                let (c, st) = words[wi][ci];
                let cw = if c == '\t' {
                    4 - (row_w % 4)
                } else {
                    cell_w(c)
                };
                if row_w + cw > width && !row.is_empty() {
                    break;
                }
                if c == '\t' {
                    for _ in 0..cw {
                        push_cell_char(&mut row, &mut row_w, ' ', st);
                    }
                } else {
                    push_cell_char(&mut row, &mut row_w, c, st);
                }
                placed_any = true;
                ci += 1;
            }
            words[wi].drain(..ci);
            if words[wi].is_empty() {
                wi += 1;
            }
            if placed_any {
                flush_row(&mut row, &mut row_w, &mut consumed);
            } else {
                wi += 1;
            }
        } else {
            flush_row(&mut row, &mut row_w, &mut consumed);
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push((row, consumed));
    }
    rows
}

fn truncate_by_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn join_math_lines(lines: &[&str]) -> Vec<String> {
    fn has_unclosed_bracket(line: &str) -> bool {
        match line.rfind("\\[") {
            Some(pos) => !line[pos + 2..].contains("\\]"),
            None => false,
        }
    }
    fn dollar_unclosed(line: &str) -> bool {
        line.matches("$$").count() % 2 == 1
    }
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        if !in_fence && (has_unclosed_bracket(lines[i]) || dollar_unclosed(lines[i])) {
            let closer = if has_unclosed_bracket(lines[i]) {
                "\\]"
            } else {
                "$$"
            };
            let mut buf = lines[i].to_string();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() && j - i <= 60 {
                buf.push('\n');
                buf.push_str(lines[j]);
                j += 1;
                if lines[j - 1].contains(closer) {
                    closed = true;
                    break;
                }
            }
            if closed {
                out.push(buf);
                i = j;
                continue;
            }

            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out
}

fn render_math_block(lines: &[&str], max_width: usize) -> Option<(Vec<Vec<Span<'static>>>, usize)> {
    use unicode_width::UnicodeWidthStr as _;
    let first = lines.first()?.trim();
    let (close, after_open) = match (first.strip_prefix("$$"), first.strip_prefix("\\[")) {
        (Some(rest), _) => ("$$", rest),
        (_, Some(rest)) => ("\\]", rest),
        _ => return None,
    };
    let (content, consumed) = if let Some(end) = after_open.find(close) {
        (after_open[..end].to_string(), 1usize)
    } else {
        let mut buf = after_open.to_string();
        let mut consumed = 1usize;
        let mut closed = false;
        for line in lines.iter().skip(1) {
            consumed += 1;
            if let Some(pos) = line.find(close) {
                buf.push('\n');
                buf.push_str(&line[..pos]);
                closed = true;
                break;
            }
            buf.push('\n');
            buf.push_str(line);
            if consumed > 60 {
                break;
            }
        }
        if !closed {
            return None;
        }
        (buf, consumed)
    };
    let translated = translate_latex(&content);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    for eq in translated.lines() {
        let eq = eq.trim();
        if eq.is_empty() {
            continue;
        }
        let eq = truncate_by_width(eq, max_width);
        let pad = max_width.saturating_sub(eq.width()) / 2;
        rows.push(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(eq, crate::theme::Theme::math()),
        ]);
    }
    if rows.is_empty() {
        return None;
    }
    Some((rows, consumed))
}

fn render_table_block(
    lines: &[&str],
    max_width: usize,
) -> Option<(Vec<Vec<Span<'static>>>, usize)> {
    let header = split_table_row(lines.first()?)?;
    if header.is_empty() {
        return None;
    }
    let delim_line = lines.get(1)?;
    let delim_cells = split_table_row(delim_line)?;
    let mut aligns: Vec<TableAlign> = delim_cells
        .iter()
        .map(|c| parse_delim_cell(c))
        .collect::<Option<Vec<_>>>()?;
    let mut rows: Vec<Vec<String>> = vec![header];
    let mut consumed = 2;
    for line in lines.iter().skip(2) {
        match split_table_row(line) {
            Some(cells) if !cells.iter().all(|c| c.is_empty()) => {
                rows.push(cells);
                consumed += 1;
            }
            _ => break,
        }
    }

    let ncols = rows.first().map(|r| r.len()).unwrap_or(0);
    if ncols == 0 {
        return None;
    }
    for row in rows.iter_mut().skip(1) {
        if row.len() > ncols {
            let tail = row[ncols - 1..].join("|");
            row.truncate(ncols - 1);
            row.push(tail);
        }
    }
    for row in rows.iter_mut() {
        row.resize(ncols, String::new());
    }
    aligns.resize(ncols, TableAlign::Left);

    let styled: Vec<Vec<Vec<Span>>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| markdown_inline_spans(&c.replace('\n', " ")))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = (0..ncols)
        .map(|c| {
            styled
                .iter()
                .map(|row| spans_width(&row[c]))
                .max()
                .unwrap_or(0)
        })
        .collect();

    for w in widths.iter_mut() {
        *w = (*w).min(48);
    }
    loop {
        let total: usize = widths.iter().sum::<usize>() + ncols * 2 + (ncols + 1);
        if total <= max_width.max(ncols * 2 + ncols + 1 + 4) {
            break;
        }
        let mut widest = None;
        for (i, w) in widths.iter().enumerate() {
            if *w > 4 && widest.map(|(_, mw)| *w > mw).unwrap_or(true) {
                widest = Some((i, *w));
            }
        }
        match widest {
            Some((i, _)) => widths[i] -= 1,
            None => break,
        }
    }

    let border = Theme::table_border();
    let rule_row = |left: &str, mid: &str, right: &str| -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled(left.to_string(), border)];
        for (i, w) in widths.iter().enumerate() {
            spans.push(Span::styled("─".repeat(w + 2), border));
            if i + 1 == widths.len() {
                spans.push(Span::styled(right.to_string(), border));
            } else {
                spans.push(Span::styled(mid.to_string(), border));
            }
        }
        spans
    };
    let data_row = |cells: &[Vec<Span<'static>>], bold: bool| -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled("│ ".to_string(), border)];
        for (i, cell) in cells.iter().enumerate() {
            let w = spans_width(cell);
            let target = widths[i];
            let pad = target.saturating_sub(w.min(target));

            let shown: Vec<Span> = if w > target {
                let plain: String = cell.iter().map(|s| s.content.as_ref()).collect();
                vec![Span::styled(
                    truncate_by_width(&plain, target),
                    Style::default().add_modifier(if bold {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                )]
            } else {
                cell.to_vec()
            };
            let inner: Vec<Span> = shown
                .into_iter()
                .map(|s| {
                    if bold {
                        Span {
                            content: s.content.clone(),
                            style: s.style.add_modifier(Modifier::BOLD),
                        }
                    } else {
                        s
                    }
                })
                .collect();
            match aligns[i] {
                TableAlign::Left => {
                    spans.extend(inner);
                    if pad > 0 {
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                }
                TableAlign::Right => {
                    if pad > 0 {
                        spans.push(Span::raw(" ".repeat(pad)));
                    }
                    spans.extend(inner);
                }
                TableAlign::Center => {
                    let left = pad / 2;
                    let right = pad - left;
                    if left > 0 {
                        spans.push(Span::raw(" ".repeat(left)));
                    }
                    spans.extend(inner);
                    if right > 0 {
                        spans.push(Span::raw(" ".repeat(right)));
                    }
                }
            }
            if i + 1 == cells.len() {
                spans.push(Span::styled(" │".to_string(), border));
            } else {
                spans.push(Span::styled(" │ ".to_string(), border));
            }
        }
        spans
    };

    let mut out = Vec::new();
    out.push(rule_row("┌", "┬", "┐"));
    out.push(data_row(&styled[0], true));
    out.push(rule_row("├", "┼", "┤"));
    for row in styled.iter().skip(1) {
        out.push(data_row(row, false));
    }
    out.push(rule_row("└", "┴", "┘"));
    Some((out, consumed))
}

fn is_code_keyword(w: &str) -> bool {
    matches!(
        w,
        "fn" | "let"
            | "mut"
            | "const"
            | "var"
            | "pub"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "if"
            | "else"
            | "elif"
            | "match"
            | "for"
            | "while"
            | "loop"
            | "return"
            | "break"
            | "continue"
            | "def"
            | "class"
            | "import"
            | "from"
            | "as"
            | "with"
            | "async"
            | "await"
            | "yield"
            | "try"
            | "except"
            | "raise"
            | "function"
            | "export"
            | "extends"
            | "super"
            | "this"
            | "new"
            | "using"
            | "namespace"
            | "template"
            | "typename"
            | "virtual"
            | "override"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "inline"
            | "include"
            | "define"
            | "ifdef"
            | "endif"
            | "int"
            | "float"
            | "double"
            | "char"
            | "bool"
            | "void"
            | "string"
            | "String"
            | "Vec"
            | "Option"
            | "Result"
            | "self"
            | "Self"
            | "crate"
            | "mod"
            | "use"
    )
}

fn highlighted_code_spans(line: &str) -> Vec<Span<'static>> {
    let base = crate::theme::Theme::code_block();

    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("#") || trimmed.starts_with("--") {
        return vec![Span::styled(
            line.to_string(),
            crate::theme::Theme::comment(),
        )];
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut in_str: Option<char> = None;
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, in_str: Option<char>| {
        if buf.is_empty() {
            return;
        }
        let w = buf.clone();
        buf.clear();
        if in_str.is_some() {
            spans.push(Span::styled(
                w,
                base.patch(Style::default().fg(Color::Green)),
            ));
        } else if is_code_keyword(&w) {
            spans.push(Span::styled(
                w,
                base.patch(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ));
        } else if w.chars().all(|c| c.is_ascii_digit()) {
            spans.push(Span::styled(
                w,
                base.patch(Style::default().fg(Color::Cyan)),
            ));
        } else {
            spans.push(Span::styled(w, base));
        }
    };
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            buf.push(c);
            if c == q && (i == 0 || chars[i - 1] != '\\') {
                flush(&mut buf, &mut spans, in_str);
                in_str = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            flush(&mut buf, &mut spans, None);
            in_str = Some(c);
            buf.push(c);
            i += 1;
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            buf.push(c);
        } else {
            flush(&mut buf, &mut spans, None);

            spans.push(Span::styled(c.to_string(), base));
        }
        i += 1;
    }
    flush(&mut buf, &mut spans, in_str);
    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), base));
    }
    spans
}

fn osc52_copy_text(text: &str) -> Result<(usize, bool), String> {
    use std::io::Write as _;
    const OSC52_MAX_CHARS: usize = 100_000;
    let capped: String = text.chars().take(OSC52_MAX_CHARS).collect();
    let truncated = capped.chars().count() < text.chars().count();
    let b64 = base64::engine::general_purpose::STANDARD.encode(capped.as_bytes());
    let seq = format!("\x1b]52;c;{b64}\x07");
    match std::io::stdout()
        .write_all(seq.as_bytes())
        .and_then(|_| std::io::stdout().flush())
    {
        Ok(()) => Ok((capped.chars().count(), truncated)),
        Err(e) => Err(format!("OSC52 write: {e}")),
    }
}

static CLIPBOARD_HOLDER: std::sync::OnceLock<std::sync::Mutex<Option<arboard::Clipboard>>> =
    std::sync::OnceLock::new();

fn pipe_to_clipboard(prog: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: spawn failed ({e})"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{prog}: stdin write failed ({e})"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{prog}: wait failed ({e})"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{prog}: exited {:?} ({})",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn clipboard_copy_text(text: &str) -> Result<(usize, &'static str), String> {
    let n = text.chars().count();
    let mut errs: Vec<String> = Vec::new();
    match pipe_to_clipboard("wl-copy", &[], text).and(pipe_to_clipboard(
        "wl-copy",
        &["--primary"],
        text,
    )) {
        Ok(()) => return Ok((n, "wl-copy")),
        Err(e) => errs.push(e),
    }

    match pipe_to_clipboard("xclip", &["-selection", "clipboard"], text).and(pipe_to_clipboard(
        "xclip",
        &["-selection", "primary"],
        text,
    )) {
        Ok(()) => return Ok((n, "xclip")),
        Err(e) => {
            errs.push(e);
            match pipe_to_clipboard("xsel", &["--clipboard", "--input"], text)
                .and(pipe_to_clipboard("xsel", &["--primary", "--input"], text))
            {
                Ok(()) => return Ok((n, "xsel")),
                Err(e) => errs.push(e),
            }
        }
    }
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.set_text(text.to_string()) {
            Ok(()) => {
                if let Ok(mut slot) = CLIPBOARD_HOLDER
                    .get_or_init(|| std::sync::Mutex::new(None))
                    .lock()
                {
                    *slot = Some(cb);
                }
                return Ok((n, "clipboard"));
            }
            Err(e) => errs.push(format!("arboard set_text failed ({e})")),
        },
        Err(e) => errs.push(format!("arboard unavailable ({e})")),
    }
    match osc52_copy_text(text) {
        Ok((m, t)) => {
            return Ok((
                m,
                if t {
                    "OSC52 unverified (truncated 100k)"
                } else {
                    "OSC52 unverified"
                },
            ));
        }
        Err(e) => errs.push(e),
    }
    tracing::warn!("clipboard copy failed: {}", errs.join("; "));
    Err(format!("no working clipboard ({})", errs.join("; ")))
}

fn read_from_clipboard(prog: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let txt = String::from_utf8_lossy(&out.stdout).to_string();
    if txt.trim().is_empty() {
        None
    } else {
        Some(txt)
    }
}

fn clipboard_paste_text() -> Option<String> {
    if let Some(t) = read_from_clipboard("wl-paste", &["--no-newline"]) {
        return Some(t);
    }
    if let Some(t) = read_from_clipboard("xclip", &["-o", "-selection", "clipboard"]) {
        return Some(t);
    }
    if let Some(t) = read_from_clipboard("xsel", &["--clipboard", "--output"]) {
        return Some(t);
    }

    if let Ok(mut cb) = arboard::Clipboard::new() {
        if let Ok(txt) = cb.get_text() {
            if !txt.trim().is_empty() {
                return Some(txt);
            }
        }
    }
    None
}

fn clipboard_paste_image_base64() -> Option<(String, u32, u32)> {
    let rgba: Option<(u32, u32, Vec<u8>)> = if let Ok(mut cb) = arboard::Clipboard::new() {
        cb.get_image()
            .ok()
            .map(|img| (img.width as u32, img.height as u32, img.bytes.to_vec()))
    } else {
        None
    };
    let rgba = rgba.or_else(|| clipboard_image_png_bytes().and_then(|png| rgba_from_png(&png)));
    let (w, h, px) = rgba?;
    if w == 0 || h == 0 || px.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let (w, h, px) = shrink_rgba(w, h, &px, PASTE_IMAGE_MAX_DIM);
    let png_bytes = png_from_rgba(w, h, &px)?;
    if png_bytes.is_empty() {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Some((b64, w, h))
}

const PASTE_IMAGE_MAX_DIM: u32 = 1024;

fn clipboard_image_png_bytes() -> Option<Vec<u8>> {
    const CAP: usize = 20 << 20;
    for (prog, args) in [
        ("wl-paste", &["--no-newline", "-t", "image/png"][..]),
        (
            "xclip",
            &["-o", "-selection", "clipboard", "-t", "image/png"][..],
        ),
    ] {
        if let Ok(out) = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
        {
            if out.status.success()
                && !out.stdout.is_empty()
                && out.stdout.len() <= CAP
                && out.stdout.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10])
            {
                return Some(out.stdout);
            }
        }
    }
    None
}

fn rgba_from_png(png_bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let decoder = png::Decoder::new(png_bytes);
    let mut reader = decoder.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return None;
    }
    let mut raw = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).ok()?;
    let px = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => raw,
        (png::ColorType::Rgb, png::BitDepth::Eight) => {
            let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
            let (chunks, _) = raw.as_chunks::<3>();
            for rgb in chunks {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        }
        _ => return None,
    };
    Some((w, h, px))
}

fn shrink_rgba(w: u32, h: u32, px: &[u8], max_dim: u32) -> (u32, u32, Vec<u8>) {
    let m = w.max(h);
    if m <= max_dim || w == 0 || h == 0 {
        return (w, h, px.to_vec());
    }
    let scale = max_dim as f64 / m as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    for y in 0..nh {
        for x in 0..nw {
            let x0 = (x as f64 / scale) as u32;
            let x1 = (((x + 1) as f64 / scale).ceil() as u32).min(w).max(x0 + 1);
            let y0 = (y as f64 / scale) as u32;
            let y1 = (((y + 1) as f64 / scale).ceil() as u32).min(h).max(y0 + 1);
            let mut acc = [0u64; 4];
            let mut n = 0u64;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * w + sx) as usize) * 4;
                    for c in 0..4 {
                        acc[c] += px[i + c] as u64;
                    }
                    n += 1;
                }
            }
            let o = ((y * nw + x) as usize) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    (nw, nh, out)
}

fn png_from_rgba(w: u32, h: u32, px: &[u8]) -> Option<Vec<u8>> {
    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(px).ok()?;
    }
    if png_bytes.is_empty() {
        None
    } else {
        Some(png_bytes)
    }
}

#[derive(Debug, Default)]
pub struct InputState {
    text: String,
    cursor: usize,
    history: Vec<String>,
    hist_idx: Option<usize>,
    draft: String,
    completion_idx: usize,
}

impl InputState {
    fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }
    fn backspace(&mut self) {
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
    fn delete(&mut self) {
        if self.cursor < self.text.len() {
            let len = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.text.drain(self.cursor..self.cursor + len);
        }
    }
    fn move_left(&mut self) {
        if self.cursor > 0 {
            let p = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.cursor -= p;
        }
    }
    fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            let l = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.cursor += l;
        }
    }
    fn move_to_start(&mut self) {
        self.cursor = 0;
    }
    fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }
    fn delete_to_start(&mut self) {
        if self.cursor > 0 {
            self.text.drain(0..self.cursor);
            self.cursor = 0;
        }
    }
    fn delete_to_end(&mut self) {
        if self.cursor < self.text.len() {
            self.text.truncate(self.cursor);
        }
    }
    fn delete_word_before(&mut self) {
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
    fn delete_word_after(&mut self) {
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
    fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }
    fn push_history(&mut self, entry: String) {
        if entry.trim().is_empty() {
            return;
        }
        self.history.push(entry);
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.hist_idx = None;
    }
    fn hist_prev(&mut self) {
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
    fn hist_next(&mut self) {
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
    fn slash_completions(&self) -> Vec<(&'static str, &'static str)> {
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
                "/compact",
                "compact context now — auto at 80% with real summary",
            ),
            ("/quit", "quit tui"),
            ("/q", "alias for /quit"),
            ("/exit", "alias for /quit"),
            ("/permissions", "show permissions"),
            ("/tasks", "background tasks — list, logs, kill"),
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

    fn apply_completion(&mut self, completion: &str) {
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum Popup {
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
}

type ModelFetchRx =
    tokio::sync::mpsc::Receiver<(Vec<String>, std::collections::HashMap<String, usize>)>;

pub struct App {
    pub model: String,
    pub session_id: String,
    pub messages: Vec<Msg>,
    pub input: InputState,
    pub scroll: usize,
    pub status: String,
    pub should_quit: bool,
    pub busy: bool,
    pending: Option<tokio::task::JoinHandle<anyhow::Result<String>>>,
    stream_rx: Option<tokio::sync::mpsc::Receiver<vioraharness_core::provider::ProviderEvent>>,
    popup: Popup,
    model_cursor: usize,
    available_models: Vec<String>,
    model_filter: String,
    tick: usize,
    streaming_buf: String,
    thinking_buf: String,
    model_fetch_rx: Option<ModelFetchRx>,

    thinking_expanded: bool,
    thinking_title: String,
    thinking_label: String,
    show_thinking: bool,

    tool_display_default: String,
    tool_display: std::collections::HashMap<String, String>,

    expanded_reasoning: HashSet<usize>,
    chat_search: Option<String>,

    theme_cursor: usize,
    available_themes: Vec<String>,

    session_cursor: usize,
    task_cursor: usize,
    session_filter: String,

    provider_cursor: usize,
    provider_key_input: String,
    provider_input_active: bool,
    provider_selected: Option<String>,
    provider_validating: bool,
    provider_msg: Option<(String, bool)>,

    pending_image: Option<PendingImage>,

    pending_texts: Vec<(usize, String)>,
    paste_seq: usize,

    expanded_messages: HashSet<usize>,

    model_context: std::collections::HashMap<String, usize>,

    mode: String,

    last_diff: Option<String>,

    pending_perm: Option<vioraharness_core::permissions::InteractiveAsk>,
    perm_cursor: usize,
    perm_rx: Option<tokio::sync::mpsc::Receiver<vioraharness_core::permissions::InteractiveAsk>>,

    pending_q: Option<vioraharness_core::permissions::PendingQuestion>,
    q_cursor: usize,
    q_answered: Vec<vioraharness_core::permissions::QuestionAnswer>,
    q_toggled: Vec<usize>,
    q_custom: String,
    q_custom_active: bool,
    q_rx: Option<tokio::sync::mpsc::Receiver<vioraharness_core::permissions::PendingQuestion>>,

    running_tool: Option<(String, String, String, std::time::Instant)>,

    compact_rx: Option<(
        String,
        tokio::sync::oneshot::Receiver<
            Result<vioraharness_core::context::compaction::CompactReport, String>,
        >,
    )>,

    ctx_freed_tokens: usize,

    wake_on_tasks: bool,

    queued_prompts: Vec<QueuedPrompt>,

    last_tool_output: Option<(String, String, String, bool)>,
    tool_output_scroll: usize,

    chat_total_lines: usize,

    selection: Option<Selection>,
    dragging: bool,
    copy_pending: bool,

    chat_area: Rect,
    input_area: Rect,
    view_start: usize,
    view_total: usize,

    vis_rows: Vec<(usize, usize)>,
}

fn render_skill_file(name: &str, description: &str, triggers: &[String]) -> String {
    let trig = if triggers.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", triggers.join(", "))
    };
    let trig_line = if triggers.is_empty() {
        "—".to_string()
    } else {
        triggers.join(", ")
    };
    format!(
        "---\nname: {name}\ndescription: {description}\ntriggers: {trig}\n---\n\
         # {cap} Skill\n\
         > Fill in when-to-use and step-by-step instructions. This skill auto-loads when the user mentions: {trig_line}.\n\
         \n\
         ## When to use\n\
         {description}\n\
         \n\
         ## Instructions\n\
         1. TODO: describe the workflow step by step, naming the exact tools/commands.\n\
         2. TODO: add validation or output expectations.\n\
         \n\
         ## Examples\n\
         - TODO: one example trigger phrase → expected behavior.\n",
        cap = {
            let mut c = name.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    )
}

fn skill_new_request(
    args: &[&str],
    cwd: &std::path::Path,
) -> Result<(String, std::path::PathBuf), String> {
    let mut local = false;
    let mut desc_parts: Vec<&str> = Vec::new();
    for a in args {
        if matches!(*a, "--local" | "--here" | "--project" | "-l") {
            local = true;
        } else {
            desc_parts.push(a);
        }
    }
    let description = desc_parts.join(" ");
    if description.trim().is_empty() {
        return Err("usage: /skill-new [--local] <what the skill should do>".into());
    }
    let target = if local {
        cwd.join("skills")
    } else {
        vioraharness_core::skills::global_skills_dir()
    };
    Ok((description, target))
}

fn skill_creator_prompt(description: &str, target: &std::path::Path) -> String {
    let example = render_skill_file(
        "waveforms",
        "Plot oscilloscope traces",
        &["waveform".into(), "plot".into()],
    );
    format!(
        "Create a new skill from this description: \"{description}\".\n\
         \n\
         Rules:\n\
         - YOU choose a short lowercase name (letters, digits, `-`, `_`).\n\
         - First check the target dir with glob: if a skill with that name exists, pick another name (never overwrite).\n\
         - Write exactly one file: {target}/<name>/SKILL.md (write creates parents).\n\
         - Frontmatter format (follow exactly, with your own values):\n\
         ```\n{example}```\n\
         - Body sections: `## When to use`, `## Instructions` (concrete steps naming exact tools/commands), `## Examples`.\n\
         - Triggers: lowercase single words from the description.\n\
         - Verify with the read tool, then reply one line: Skill '<name>' ready at <path>.\n\
         - Do nothing else.",
        target = target.display()
    )
}

fn hit_rect(area: Rect, mx: u16, my: u16) -> bool {
    area.width > 0
        && area.height > 0
        && mx >= area.x
        && mx < area.x.saturating_add(area.width)
        && my >= area.y
        && my < area.y.saturating_add(area.height)
}

fn has_key(env: &str) -> bool {
    std::env::var(env)
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

fn provider_catalog() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "opencode",
            "Gateway",
            "OPENCODE_API_KEY",
            "Managed gateway — pay-per-use, free tier available",
        ),
        (
            "opencode-go",
            "Gateway Go",
            "OPENCODE_API_KEY",
            "Managed open-model endpoint — same key (Go catalog)",
        ),
        (
            "openrouter",
            "OpenRouter",
            "OPENROUTER_API_KEY",
            "75+ models via one key — recommended",
        ),
        (
            "gemini",
            "Google Gemini",
            "GEMINI_API_KEY",
            "Native Gemini thinking",
        ),
        (
            "anthropic",
            "Anthropic",
            "ANTHROPIC_API_KEY",
            "Direct Claude",
        ),
        ("openai", "OpenAI", "OPENAI_API_KEY", "Direct GPT"),
    ]
}

fn providers_env_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("vioraharness/.env")
}

fn persist_provider_key(env_name: &str, key: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = providers_env_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let line = format!("{env_name}={key}");

    let mut filtered: Vec<String> = existing
        .lines()
        .filter(|l| !l.trim_start().starts_with(&format!("{env_name}=")))
        .map(|s| s.to_string())
        .collect();
    filtered.push(line);
    let content = filtered.join("\n") + "\n";
    std::fs::write(&path, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    std::env::set_var(env_name, key);
    Ok(path)
}

async fn validate_provider_key(provider_id: &str, key: &str) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    match provider_id {
        "opencode" => {
            let resp = client
                .get("https://opencode.ai/zen/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "opencode-go" => {
            let resp = client
                .get("https://opencode.ai/zen/go/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "openrouter" => {
            let resp = client
                .get("https://openrouter.ai/api/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "gemini" => {
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models?key={key}");
            let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("models")
                .and_then(|m| m.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "anthropic" | "openai" => {
            if key.trim().len() < 10 {
                return Err("key too short".into());
            }
            Ok(0)
        }
        _ => Err("unknown provider".into()),
    }
}

impl App {
    fn fetch_models_from_openrouter() -> Vec<String> {
        let has_openrouter = has_key("OPENROUTER_API_KEY");
        let has_gemini = has_key("GEMINI_API_KEY");

        let is_gateway_model = |m: &str| {
            m.starts_with("opencode/")
                || m.starts_with("opencode-go/")
                || m.starts_with("zen/")
                || m.starts_with("go/")
                || vioraharness_core::provider::opencode::is_free_model(m)
        };

        let filter_by_keys = |list: Vec<String>| -> Vec<String> {
            list.into_iter()
                .filter(|m| {
                    if is_gateway_model(m) {
                        true
                    } else {
                        let is_gemini = m.contains("gemini")
                            || m.starts_with("google/")
                            || m.starts_with("gemini-");
                        if is_gemini {
                            has_gemini
                        } else {
                            has_openrouter
                        }
                    }
                })
                .collect()
        };
        let normalize_gemini = |m: String| -> String {
            if m.starts_with("google/") || m.contains('/') {
                m
            } else if m.starts_with("gemini-") {
                format!("google/{m}")
            } else {
                m
            }
        };

        let mut json_models: Vec<String> = Vec::new();
        for path in vioraharness_core::loop_mod::config_candidates() {
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("openrouter"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(normalize_gemini(x.to_string()));
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("gemini"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(normalize_gemini(x.to_string()));
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("opencode"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(x.to_string());
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("opencode-go"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(x.to_string());
                        }
                    }
                }
            }
        }
        json_models.sort();
        json_models.dedup();
        let filtered = filter_by_keys(json_models);
        if !filtered.is_empty() {
            tracing::info!(
                "fetch_models_from_openrouter: {} from vioraharness.json after key filter",
                filtered.len()
            );
            return filtered;
        }

        filtered
    }

    async fn fetch_models_live() -> (Vec<String>, std::collections::HashMap<String, usize>) {
        let fallback = Self::fetch_models_from_openrouter();
        let has_openrouter = has_key("OPENROUTER_API_KEY");
        let has_gemini = has_key("GEMINI_API_KEY");
        let has_gateway = has_key("OPENCODE_API_KEY") || has_key("ZEN_API_KEY");
        tracing::info!(
            "fetch_models_live: has_openrouter={} has_gemini={} has_gateway={} fallback={} ",
            has_openrouter,
            has_gemini,
            has_gateway,
            fallback.len()
        );

        let mut merged: Vec<String> = fallback.clone();
        let mut context_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for m in &fallback {
            context_map.entry(m.clone()).or_insert(128_000);
        }

        // Seed live context sizes from the disk cache first: if the live
        // catalog fetch flakes (it did — 200 OK with a truncated body),
        // the picker still shows real sizes instead of 128k fallbacks.
        // Live values overwrite these below.
        for (id, size) in vioraharness_core::provider::catalog::openrouter_context_cached().await {
            context_map.insert(id, size);
        }

        if has_openrouter {
            match std::env::var("OPENROUTER_API_KEY") {
                Ok(key) => {
                    match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(15))
                        .build()
                    {
                        Ok(client) => {
                            tracing::info!("fetch_models_live: curl OpenRouter GET https://openrouter.ai/api/v1/models Bearer {}...{}", &key[..4.min(key.len())], &key[key.len().saturating_sub(4)..]);
                            // The catalog body is megabytes of JSON; a
                            // truncated stream decodes as 200 OK + parse
                            // failure (seen live). Retry transport/decode,
                            // not HTTP error statuses.
                            let mut fetched: Option<serde_json::Value> = None;
                            for attempt in 1..=3 {
                                match client
                                    .get("https://openrouter.ai/api/v1/models")
                                    .header("Authorization", format!("Bearer {key}"))
                                    .send()
                                    .await
                                {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        tracing::info!(
                                            "fetch_models_live: OpenRouter status {} (attempt {attempt})",
                                            status
                                        );
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!(
                                                "fetch_models_live: OpenRouter error {} — {}",
                                                status,
                                                txt.chars().take(300).collect::<String>()
                                            );
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                fetched = Some(json);
                                                break;
                                            }
                                            Err(e) => tracing::warn!(
                                                "fetch_models_live: OpenRouter json parse failed (attempt {attempt}): {e}"
                                            ),
                                        }
                                    }
                                    Err(e) => tracing::warn!(
                                        "fetch_models_live: OpenRouter request failed (attempt {attempt}): {e}"
                                    ),
                                }
                                if attempt < 3 {
                                    tokio::time::sleep(std::time::Duration::from_secs(
                                        attempt as u64,
                                    ))
                                    .await;
                                }
                            }
                            if let Some(json) = fetched {
                                if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                    let mut out: Vec<String> = Vec::new();
                                    for v in data.iter() {
                                        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                            let id_s = id.to_string();
                                            out.push(id_s.clone());
                                            if let Some(cl) =
                                                v.get("context_length").and_then(|x| x.as_u64())
                                            {
                                                context_map.insert(id_s.clone(), cl as usize);
                                            } else if let Some(cl2) = v
                                                .get("context_length")
                                                .and_then(|x| x.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                            {
                                                context_map.insert(id_s.clone(), cl2 as usize);
                                            }

                                            if let Some(tp) = v
                                                .get("top_provider")
                                                .and_then(|x| x.get("context_length"))
                                                .and_then(|x| x.as_u64())
                                            {
                                                context_map.entry(id_s).or_insert(tp as usize);
                                            }
                                        }
                                    }
                                    let before = out.len();
                                    out.retain(|m| {
                                        !m.contains("embedding") && !m.contains("moderation")
                                    });
                                    tracing::info!("fetch_models_live: OpenRouter {} models ({} after filter embedding/moderation)", before, out.len());
                                    merged.extend(out);
                                } else {
                                    tracing::warn!("fetch_models_live: OpenRouter no data array");
                                }
                            } else {
                                tracing::warn!("fetch_models_live: OpenRouter catalog unavailable after 3 attempts");
                            }
                        }
                        Err(e) => tracing::warn!(
                            "fetch_models_live: OpenRouter client build failed: {}",
                            e
                        ),
                    }
                }
                Err(_) => tracing::warn!("fetch_models_live: OPENROUTER_API_KEY not set"),
            }
        }

        {
            let known: Vec<(String, usize)> = fallback
                .iter()
                .filter_map(|m| {
                    vioraharness_core::provider::catalog::match_or_context(m, &context_map)
                        .map(|s| (m.clone(), s))
                })
                .collect();
            for (m, s) in known {
                context_map.insert(m, s);
            }
        }

        if has_gemini {
            match std::env::var("GEMINI_API_KEY") {
                Ok(key) => {
                    let prefix = &key[..4.min(key.len())];
                    let suffix = &key[key.len().saturating_sub(4)..];
                    let mut gemini_fetched: Vec<String> = Vec::new();
                    let mut gemini_ok = false;

                    match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                    {
                        Ok(client) => {
                            let url =
                                "https://generativelanguage.googleapis.com/v1beta/openai/models";
                            tracing::info!("fetch_models_live: curl Gemini openai compat GET {} Authorization: Bearer {}...{}", url, prefix, suffix);
                            match client
                                .get(url)
                                .header("Authorization", format!("Bearer {key}"))
                                .send()
                                .await
                            {
                                Ok(resp) => {
                                    let status = resp.status();
                                    tracing::info!(
                                        "fetch_models_live: Gemini openai compat status {}",
                                        status
                                    );
                                    if status.is_success() {
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                                    for v in data {
                                                        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                            let short = id.trim_start_matches("models/").to_string();
                                                            let norm = if short.starts_with("gemini-") || short.starts_with("gemma-") || short.starts_with("veo-") || short.starts_with("nano-") || short.starts_with("lyria-") { format!("google/{short}") } else { short };

                                                            gemini_fetched.push(norm);
                                                        }
                                                    }
                                                    tracing::info!("fetch_models_live: Gemini openai compat fetched {} models", gemini_fetched.len());
                                                    gemini_ok = !gemini_fetched.is_empty();
                                                }
                                            }
                                            Err(e) => tracing::warn!("fetch_models_live: Gemini openai compat json parse {}", e),
                                        }
                                    } else {
                                        let txt = resp.text().await.unwrap_or_default();
                                        tracing::warn!(
                                            "fetch_models_live: Gemini openai compat error {} — {}",
                                            status,
                                            txt.chars().take(300).collect::<String>()
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!(
                                    "fetch_models_live: Gemini openai compat request failed: {}",
                                    e
                                ),
                            }
                        }
                        Err(e) => {
                            tracing::warn!("fetch_models_live: Gemini client build failed: {}", e)
                        }
                    }

                    if !gemini_ok {
                        gemini_fetched.clear();
                        if let Ok(client) = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                        {
                            let mut page_token: Option<String> = None;
                            let mut pages = 0;
                            loop {
                                pages += 1;
                                let mut url = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000".to_string();
                                if let Some(tok) = &page_token {
                                    url = format!("{url}&pageToken={tok}");
                                }
                                tracing::info!("fetch_models_live: curl Gemini native GET {} x-goog-api-key: {}...{} (page {})", url, prefix, suffix, pages);
                                match client.get(&url).header("x-goog-api-key", &key).send().await {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!("fetch_models_live: Gemini native header error {} — {}", status, txt.chars().take(300).collect::<String>());
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(models) =
                                                    json.get("models").and_then(|m| m.as_array())
                                                {
                                                    for v in models {
                                                        if let Some(name) =
                                                            v.get("name").and_then(|n| n.as_str())
                                                        {
                                                            let short = name
                                                                .trim_start_matches("models/")
                                                                .to_string();
                                                            let id = if short.starts_with("gemini-")
                                                                || short.starts_with("gemma-")
                                                            {
                                                                format!("google/{short}")
                                                            } else {
                                                                short
                                                            };
                                                            gemini_fetched.push(id.clone());
                                                            if let Some(limit) = v
                                                                .get("inputTokenLimit")
                                                                .and_then(|x| x.as_u64())
                                                            {
                                                                context_map.insert(
                                                                    id.clone(),
                                                                    limit as usize,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Some(next) = json
                                                    .get("nextPageToken")
                                                    .and_then(|t| t.as_str())
                                                {
                                                    if !next.is_empty() && pages < 5 {
                                                        page_token = Some(next.to_string());
                                                        continue;
                                                    }
                                                }
                                                break;
                                            }
                                            Err(e) => {
                                                tracing::warn!("fetch_models_live: Gemini native json parse {}", e);
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "fetch_models_live: Gemini native request failed: {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                            if !gemini_fetched.is_empty() {
                                tracing::info!("fetch_models_live: Gemini native header fetched {} models in {} pages", gemini_fetched.len(), pages);
                                gemini_ok = true;
                            }
                        }
                    }

                    if !gemini_ok {
                        gemini_fetched.clear();
                        if let Ok(client) = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                        {
                            let mut page_token: Option<String> = None;
                            let mut pages = 0;
                            loop {
                                pages += 1;
                                let mut url = format!("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000&key={key}");
                                if let Some(tok) = &page_token {
                                    url = format!("{url}&pageToken={tok}");
                                }
                                tracing::info!("fetch_models_live: curl Gemini native GET {} (query key, page {})", url.chars().take(90).collect::<String>(), pages);
                                match client.get(&url).send().await {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!("fetch_models_live: Gemini native query error {} — {}", status, txt.chars().take(300).collect::<String>());
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(models) =
                                                    json.get("models").and_then(|m| m.as_array())
                                                {
                                                    for v in models {
                                                        if let Some(name) =
                                                            v.get("name").and_then(|n| n.as_str())
                                                        {
                                                            let short = name
                                                                .trim_start_matches("models/")
                                                                .to_string();
                                                            let id = if short.starts_with("gemini-")
                                                                || short.starts_with("gemma-")
                                                            {
                                                                format!("google/{short}")
                                                            } else {
                                                                short
                                                            };
                                                            gemini_fetched.push(id.clone());
                                                            if let Some(limit) = v
                                                                .get("inputTokenLimit")
                                                                .and_then(|x| x.as_u64())
                                                            {
                                                                context_map.insert(
                                                                    id.clone(),
                                                                    limit as usize,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Some(next) = json
                                                    .get("nextPageToken")
                                                    .and_then(|t| t.as_str())
                                                {
                                                    if !next.is_empty() && pages < 5 {
                                                        page_token = Some(next.to_string());
                                                        continue;
                                                    }
                                                }
                                                break;
                                            }
                                            Err(e) => {
                                                tracing::warn!("fetch_models_live: Gemini native query json {}", e);
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "fetch_models_live: Gemini native query request {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                            if !gemini_fetched.is_empty() {
                                tracing::info!(
                                    "fetch_models_live: Gemini native query fetched {} models",
                                    gemini_fetched.len()
                                );
                                gemini_ok = true;
                            }
                        }
                    }
                    if gemini_ok {
                        tracing::info!(
                            "fetch_models_live: Gemini final fetched {} (sample: {:?})",
                            gemini_fetched.len(),
                            gemini_fetched.iter().take(5).cloned().collect::<Vec<_>>()
                        );
                        merged.extend(gemini_fetched);
                    } else {
                        tracing::warn!("fetch_models_live: Gemini all endpoints failed — check GEMINI_API_KEY and run curl isolation");
                    }
                }
                Err(_) => tracing::warn!("fetch_models_live: GEMINI_API_KEY not set"),
            }
        }

        let or_sizes: std::collections::HashMap<String, usize> = context_map.clone();

        {
            let key_opt = std::env::var("OPENCODE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ZEN_API_KEY").ok());
            let has_key = key_opt
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);

            let zen_urls = ["https://opencode.ai/zen/v1/models"];
            for url in zen_urls {
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(4))
                    .build()
                {
                    tracing::info!(
                        "fetch_models_live: curl Zen GET {} {}",
                        url,
                        if has_key { "with key" } else { "public" }
                    );
                    let mut reqb = client.get(url);
                    if has_key {
                        reqb = reqb.header(
                            "Authorization",
                            format!("Bearer {}", key_opt.as_ref().unwrap()),
                        );
                    }
                    match reqb.send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<serde_json::Value>().await {
                                Ok(json) => {
                                    if let Some(data) = json.get("data").and_then(|d| d.as_array())
                                    {
                                        let mut out: Vec<String> = Vec::new();
                                        for v in data {
                                            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                let full = format!("opencode/{}", id);
                                                out.push(full.clone());

                                                let size = vioraharness_core::provider::catalog::match_or_context(
                                                    &full, &or_sizes,
                                                )
                                                .unwrap_or(128_000);
                                                context_map.entry(full.clone()).or_insert(size);

                                                context_map.entry(id.to_string()).or_insert(size);
                                            }
                                        }
                                        tracing::info!("fetch_models_live: Zen fetched {} models (sample {:?})", out.len(), out.iter().take(4).cloned().collect::<Vec<_>>());
                                        merged.extend(out);
                                    }
                                }
                                Err(e) => tracing::warn!("fetch_models_live: Zen json parse {}", e),
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            tracing::warn!(
                                "fetch_models_live: Zen error {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => tracing::warn!("fetch_models_live: Zen request failed {}", e),
                    }
                }
            }
        }

        {
            let key_opt = std::env::var("OPENCODE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ZEN_API_KEY").ok());
            let has_key = key_opt
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);

            let go_urls = ["https://opencode.ai/zen/go/v1/models"];
            for url in go_urls {
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(4))
                    .build()
                {
                    tracing::info!(
                        "fetch_models_live: curl Go GET {} {}",
                        url,
                        if has_key { "with key" } else { "public" }
                    );
                    let mut reqb = client.get(url);
                    if has_key {
                        reqb = reqb.header(
                            "Authorization",
                            format!("Bearer {}", key_opt.as_ref().unwrap()),
                        );
                    }
                    match reqb.send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<serde_json::Value>().await {
                                Ok(json) => {
                                    if let Some(data) = json.get("data").and_then(|d| d.as_array())
                                    {
                                        let mut out: Vec<String> = Vec::new();
                                        for v in data {
                                            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                let full = format!("opencode-go/{}", id);
                                                out.push(full.clone());
                                                let size = vioraharness_core::provider::catalog::match_or_context(
                                                    &full, &or_sizes,
                                                )
                                                .unwrap_or(128_000);
                                                context_map.entry(full.clone()).or_insert(size);
                                            }
                                        }
                                        tracing::info!(
                                            "fetch_models_live: Go fetched {} models (sample {:?})",
                                            out.len(),
                                            out.iter().take(4).cloned().collect::<Vec<_>>()
                                        );
                                        merged.extend(out);
                                    }
                                }
                                Err(e) => tracing::warn!("fetch_models_live: Go json parse {}", e),
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();

                            tracing::info!(
                                "fetch_models_live: Go status {} — {}",
                                status,
                                txt.chars().take(150).collect::<String>()
                            );
                        }
                        Err(e) => tracing::warn!("fetch_models_live: Go request failed {}", e),
                    }
                }
            }
        }
        if merged.is_empty() {
            tracing::warn!(
                "fetch_models_live: merged empty, returning fallback {} ",
                fallback.len()
            );
            return (fallback, context_map);
        }
        merged.retain(|m| {
            !m.to_lowercase().contains("embedding") && !m.to_lowercase().contains("moderation")
                || m.contains("gemini-embedding")
        });
        merged.sort();
        merged.dedup();
        tracing::info!(
            "fetch_models_live: merged total {} (after dedup, sample {:?})",
            merged.len(),
            merged.iter().take(8).cloned().collect::<Vec<_>>()
        );

        let prioritized: Vec<String> = {
            let gemini: Vec<String> = merged
                .iter()
                .filter(|m| m.contains("gemini") || m.contains("gemma"))
                .cloned()
                .collect();
            let gateway_models: Vec<String> = merged
                .iter()
                .filter(|m| m.starts_with("opencode/") || m.starts_with("opencode-go/"))
                .cloned()
                .collect();
            let others: Vec<String> = merged
                .iter()
                .filter(|m| {
                    !(m.contains("gemini")
                        || m.contains("gemma")
                        || m.starts_with("opencode/")
                        || m.starts_with("opencode-go/"))
                })
                .cloned()
                .collect();
            let mut out = gemini;
            out.extend(gateway_models);
            out.extend(others);
            out
        };

        let mut out = prioritized;
        out.truncate(200);
        tracing::info!(
            "fetch_models_live: after family-first truncate {} (gemini={}, gateway={})",
            out.len(),
            out.iter().any(|m| m.contains("gemini")),
            out.iter().any(|m| m.starts_with("opencode")),
        );
        if out.is_empty() {
            (fallback, context_map)
        } else {
            (out, context_map)
        }
    }
    fn tui_state_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            });
        base.join("vioraharness/tui_state.json")
    }

    fn load_tui_state() -> serde_json::Value {
        let p = Self::tui_state_path();
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    }

    fn save_tui_state(patch: serde_json::Value) {
        let p = Self::tui_state_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut cur = Self::load_tui_state();
        if let (Some(cur_obj), Some(patch_obj)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in patch_obj {
                cur_obj.insert(k.clone(), v.clone());
            }
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(&cur).unwrap_or_default());
    }

    fn tool_verbosity(&self, name: &str) -> ToolVerbosity {
        Self::verbosity_for(&self.tool_display, &self.tool_display_default, name)
    }

    fn verbosity_for(
        map: &std::collections::HashMap<String, String>,
        default: &str,
        name: &str,
    ) -> ToolVerbosity {
        if let Some(l) = map.get(name) {
            return parse_tool_verbosity(l);
        }
        parse_tool_verbosity(default)
    }

    fn save_tool_display(&self) {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "default".into(),
            serde_json::Value::String(self.tool_display_default.clone()),
        );
        for (k, v) in &self.tool_display {
            obj.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        Self::save_tui_state(serde_json::json!({"tool_display": obj}));
    }

    fn toggle_recent_reasoning(&mut self) -> bool {
        if let Some(idx) = self
            .messages
            .iter()
            .rposition(|m| m.role == "assistant" && m.reasoning.is_some())
        {
            if self.expanded_reasoning.contains(&idx) {
                self.expanded_reasoning.remove(&idx);
            } else {
                self.expanded_reasoning.insert(idx);
            }
            true
        } else {
            false
        }
    }

    fn toggle_recent_long_message(&mut self) {
        if let Some(idx) = self
            .messages
            .iter()
            .rposition(|m| m.role == "user" && m.content.lines().count() > CHAT_COLLAPSE_LINES)
        {
            if !self.expanded_messages.remove(&idx) {
                self.expanded_messages.insert(idx);
            }
        }
    }

    fn insert_with_space(&mut self, s: &str) {
        let glued = self.input.cursor > 0
            && self.input.cursor <= self.input.text.len()
            && self.input.text[..self.input.cursor]
                .chars()
                .next_back()
                .map(|c| !c.is_whitespace())
                .unwrap_or(false);
        if glued {
            self.input.insert_str(" ");
        }
        self.input.insert_str(s);
    }

    fn insert_pasted_text(&mut self, txt: &str) {
        if is_long_paste(txt) {
            self.paste_seq += 1;
            let id = self.paste_seq;
            let lines = txt.lines().count();
            self.pending_texts.push((id, txt.to_string()));
            self.insert_with_space(&paste_chip(id, lines));
            self.status = format!("pasted {lines} lines → chip (expands on send)");
        } else {
            self.input.insert_str(txt);
        }
    }

    fn attach_pasted_image(&mut self, b64: String, mime: &'static str, label: String) {
        let chip = image_chip(&label);
        let kb = b64.len() / 1024;
        self.pending_image = Some(PendingImage { b64, mime, label });
        self.insert_with_space(&chip);
        self.messages.push(Msg::new(
            "system",
            format!("Image attached ({chip}, ~{kb}KB) — sent with next prompt as vision"),
        ));
    }

    fn cmd_verbosity(&mut self, args: &[&str]) {
        if args.is_empty() {
            let mut lines = vec![format!(
                "tool cards: default = {}",
                tool_verbosity_name(self.tool_verbosity(""))
            )];
            let mut tools: Vec<&String> = self.tool_display.keys().collect();
            tools.sort();
            for t in tools {
                lines.push(format!(
                    "  {t} = {}",
                    tool_verbosity_name(self.tool_verbosity(t))
                ));
            }
            lines.push("usage: /verbosity [tool] <hidden|quiet|compact|full>".into());
            self.messages.push(Msg::new("system", lines.join("\n")));
            return;
        }
        let (tool, level) = if args.len() == 1 {
            let a = args[0].to_lowercase();
            if matches!(
                a.as_str(),
                "hidden"
                    | "off"
                    | "none"
                    | "quiet"
                    | "name"
                    | "minimal"
                    | "compact"
                    | "full"
                    | "verbose"
                    | "detail"
            ) {
                (None, a)
            } else {
                self.messages.push(Msg::new(
                    "system",
                    format!(
                        "{} = {} (default = {})\nusage: /verbosity [tool] <hidden|quiet|compact|full>",
                        args[0],
                        tool_verbosity_name(self.tool_verbosity(args[0])),
                        tool_verbosity_name(self.tool_verbosity("")),
                    ),
                ));
                return;
            }
        } else {
            (Some(args[0].to_string()), args[1].to_lowercase())
        };
        if !matches!(
            level.as_str(),
            "hidden"
                | "off"
                | "none"
                | "quiet"
                | "name"
                | "minimal"
                | "compact"
                | "full"
                | "verbose"
                | "detail"
        ) {
            self.messages.push(Msg::new(
                "system",
                "usage: /verbosity [tool] <hidden|quiet|compact|full>".to_string(),
            ));
            return;
        }
        let canonical = tool_verbosity_name(parse_tool_verbosity(&level)).to_string();
        match tool {
            None => {
                self.tool_display_default = canonical.clone();
                self.save_tool_display();
                self.messages.push(Msg::new(
                    "system",
                    format!("tool cards default → {canonical} — saved"),
                ));
            }
            Some(t) => {
                self.tool_display.insert(t.clone(), canonical.clone());
                self.save_tool_display();
                self.messages.push(Msg::new(
                    "system",
                    format!("tool cards: {t} → {canonical} — saved"),
                ));
            }
        }
    }

    fn save_screenshot(&mut self, path: &str) -> anyhow::Result<()> {
        let backend = TestBackend::new(120, 40);
        let mut term = Terminal::new(backend)?;
        term.draw(|f| self.draw(f))?;
        let buffer = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    pub fn new(model: String) -> Self {
        let sid = new_session_id();
        let tcfg = crate::theme::ThemeConfig::load();
        let state = Self::load_tui_state();

        let cli_default = String::new();
        let model = if model == cli_default {
            if let Some(saved) = state.get("last_model").and_then(|v| v.as_str()) {
                if !saved.is_empty() && saved != cli_default {
                    saved.to_string()
                } else {
                    model
                }
            } else {
                model
            }
        } else {
            model
        };

        let show_thinking = state
            .get("show_thinking")
            .and_then(|v| v.as_bool())
            .unwrap_or(tcfg.show_thinking);
        let thinking_title = state
            .get("thinking_title")
            .and_then(|v| v.as_str())
            .unwrap_or(&tcfg.thinking_title)
            .to_string();
        let thinking_label = state
            .get("thinking_label")
            .and_then(|v| v.as_str())
            .unwrap_or(&tcfg.thinking_label)
            .to_string();

        let (mut tool_display_default, mut tool_display) = load_tool_display_config();
        if let Some(td) = state.get("tool_display") {
            if let Some(d) = td.get("default").and_then(|v| v.as_str()) {
                tool_display_default = d.to_string();
            }
            if let Some(obj) = td.as_object() {
                for (k, vv) in obj {
                    if k == "default" {
                        continue;
                    }
                    if let Some(s) = vv.as_str() {
                        tool_display.insert(k.clone(), s.to_string());
                    }
                }
            }
        }

        let wake_on_tasks: bool = 'cfg: {
            for cand in vioraharness_core::loop_mod::config_candidates() {
                if let Ok(s) = std::fs::read_to_string(&cand) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        if let Some(tui) = v.get("tui") {
                            break 'cfg tui
                                .get("wake_on_task_done")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(true);
                        }
                    }
                }
            }
            true
        };

        let initial_models = Self::fetch_models_from_openrouter();

        let (perm_tx, perm_rx) = tokio::sync::mpsc::channel(8);
        vioraharness_core::permissions::set_interactive_sender(perm_tx);

        let (q_tx, q_rx) = tokio::sync::mpsc::channel(4);
        vioraharness_core::permissions::set_question_sender(q_tx);

        vioraharness_core::tools::bash::prune_old_logs();

        Self {
            model: model.clone(),
            session_id: sid,
            messages: Vec::new(),
            input: InputState::default(),
            scroll: 0,
            status: "ready".into(),
            should_quit: false,
            busy: false,
            pending: None,
            popup: Popup::None,
            model_cursor: 0,
            available_models: initial_models,
            model_filter: String::new(),
            tick: 0,
            streaming_buf: String::new(),
            thinking_buf: String::new(),
            stream_rx: None,
            model_fetch_rx: None,
            thinking_expanded: false,
            thinking_title,
            thinking_label,
            show_thinking,
            tool_display_default,
            tool_display,
            expanded_reasoning: HashSet::new(),
            chat_search: None,
            theme_cursor: 0,
            available_themes: vec![
                "tokyonight".into(),
                "tokyonight-soft".into(),
                "eye-comfort".into(),
                "warm-dark".into(),
                "catppuccin".into(),
                "dracula".into(),
                "gruvbox".into(),
                "nord".into(),
                "system".into(),
            ],
            session_cursor: 0,
            task_cursor: 0,
            session_filter: String::new(),
            provider_cursor: 0,
            provider_key_input: String::new(),
            provider_input_active: false,
            provider_selected: None,
            provider_validating: false,
            provider_msg: None,
            pending_image: None,
            pending_texts: Vec::new(),
            paste_seq: 0,
            expanded_messages: HashSet::new(),
            model_context: std::collections::HashMap::new(),
            mode: "build".into(),
            last_diff: None,
            pending_perm: None,
            perm_cursor: 0,
            perm_rx: Some(perm_rx),
            pending_q: None,
            q_cursor: 0,
            q_answered: Vec::new(),
            q_toggled: Vec::new(),
            q_custom: String::new(),
            q_custom_active: false,
            q_rx: Some(q_rx),
            running_tool: None,
            compact_rx: None,
            ctx_freed_tokens: 0,
            wake_on_tasks,
            queued_prompts: Vec::new(),
            last_tool_output: None,
            tool_output_scroll: 0,
            chat_total_lines: 0,
            selection: None,
            dragging: false,
            copy_pending: false,
            chat_area: Rect::default(),
            input_area: Rect::default(),
            view_start: 0,
            view_total: 0,
            vis_rows: Vec::new(),
        }
    }

    pub async fn run(mut self) -> anyhow::Result<String> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        self.model_fetch_rx = Some(rx);
        tokio::spawn(async move {
            let live = Self::fetch_models_live().await;
            let _ = tx.send(live).await;
        });
        self.status = "fetching models…".into();
        let mut terminal = ratatui::init();

        if let Err(e) = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
        {
            tracing::warn!("mouse capture unavailable: {e}");
        }
        let sid = self.session_id.clone();
        let res = self.event_loop(&mut terminal).await;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        match res {
            Ok(()) => Ok(self.session_id),
            Err(e) => {
                tracing::warn!("tui event_loop error for {}: {e}", sid);
                Err(e)
            }
        }
    }

    async fn event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> anyhow::Result<()> {
        loop {
            terminal.draw(|f| self.draw(f))?;
            if self.should_quit {
                break;
            }

            if let Some(rx) = &mut self.model_fetch_rx {
                match rx.try_recv() {
                    Ok((models, ctx_map)) => {
                        let n = models.len();
                        let has_gem = models.iter().any(|m| m.contains("gemini"));
                        let sample: Vec<String> = models
                            .iter()
                            .filter(|m| m.contains("gemini"))
                            .take(3)
                            .cloned()
                            .collect();
                        tracing::info!(
                            "model_fetch_rx: got {} models (has_gemini={}, sample {:?})",
                            n,
                            has_gem,
                            sample
                        );
                        if n > self.available_models.len()
                            || (n > 0 && n != self.available_models.len())
                        {
                            self.available_models = models;
                            for (k, v) in ctx_map {
                                self.model_context.insert(k, v);
                            }
                            self.status = format!("ready — {n} models");

                            tracing::info!("fetched {n} models");
                        } else if n == 0 {
                            self.status = "ready".into();

                            if self.messages.len() < 2 {
                                self.messages.push(Msg::new("system", "⚠ Fetched 0 models — check keys: /providers or see tui.log — doctor: vioraharness doctor (shell)"));
                            }
                        } else {
                            self.status = "ready".into();
                            tracing::info!(
                                "model_fetch_rx: no growth {} vs {}",
                                n,
                                self.available_models.len()
                            );
                        }
                        self.model_fetch_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.model_fetch_rx = None;
                        self.status = "ready".into();
                        tracing::warn!("model_fetch_rx disconnected");
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            self.poll_compact();

            self.poll_task_completions();

            if let Some(rx) = &mut self.perm_rx {
                match rx.try_recv() {
                    Ok(ask) => {
                        self.pending_perm = Some(ask);
                        self.popup = Popup::PermissionAsk;
                        self.perm_cursor = 0;
                        self.status = "permission ask — choose".into();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.perm_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            if let Some(rx) = &mut self.q_rx {
                match rx.try_recv() {
                    Ok(q) => {
                        if let Some(old) = self.pending_q.take() {
                            let _ = old
                                .tx
                                .send(vioraharness_core::permissions::QuestionResult::Cancelled);
                        }
                        self.pending_q = Some(q);
                        self.popup = Popup::Question;
                        self.q_cursor = 0;
                        self.q_answered.clear();
                        self.q_toggled.clear();
                        self.q_custom.clear();
                        self.q_custom_active = false;
                        self.status = "question — answer to continue".into();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.q_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            if let Some(rx) = &mut self.stream_rx {
                let td_map = self.tool_display.clone();
                let td_default = self.tool_display_default.clone();
                while let Ok(ev) = rx.try_recv() {
                    match ev {
                        vioraharness_core::provider::ProviderEvent::TextDelta(t) => {
                            self.streaming_buf.push_str(&t);
                        }
                        vioraharness_core::provider::ProviderEvent::ReasoningDelta(r) => {
                            if self.show_thinking {
                                self.thinking_buf.push_str(&r);

                                self.status =
                                    format!("thinking… {} chars", self.thinking_buf.len());
                            }
                        }
                        vioraharness_core::provider::ProviderEvent::ToolCallDelta {
                            id,
                            name,
                            args,
                            thought_signature: _,
                        } => {
                            let verbosity = Self::verbosity_for(&td_map, &td_default, &name);
                            let pretty = pretty_tool_args_wide(
                                &name,
                                &args,
                                verbosity == ToolVerbosity::Full,
                            );

                            let mut coalesced = false;
                            if let Some(last) = self.messages.last_mut() {
                                if last.role == "system" && last.items.len() == 1 {
                                    if let Some(Content::ToolCall {
                                        name: ln,
                                        status,
                                        args: la,
                                        id: lid,
                                    }) = last.items.last_mut()
                                    {
                                        if *ln == name && *status == ToolStatus::Running {
                                            *la = args.clone();
                                            *lid = id.clone();
                                            last.content = if pretty.is_empty() {
                                                format!("→ {name}")
                                            } else {
                                                format!("→ {name} {pretty}")
                                            };
                                            coalesced = true;
                                        }
                                    }
                                }
                            }
                            if !coalesced && verbosity != ToolVerbosity::Hidden {
                                self.messages.push(Msg {
                                    role: "system".into(),
                                    content: if pretty.is_empty() {
                                        format!("→ {name}")
                                    } else {
                                        format!("→ {name} {pretty}")
                                    },
                                    items: vec![Content::ToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                        status: ToolStatus::Running,
                                    }],
                                    timestamp: chrono_like_now(),
                                    reasoning: None,
                                });
                            }

                            self.running_tool = Some((
                                id.clone(),
                                name.clone(),
                                pretty.clone(),
                                std::time::Instant::now(),
                            ));
                            if name == "bash" {
                                self.status =
                                    format!("running: bash {}…", truncate_chars(&pretty, 40));
                            } else {
                                self.status = format!("running: {name}…");
                            }
                        }
                        vioraharness_core::provider::ProviderEvent::ToolResultDelta {
                            id,
                            content,
                            ok,
                        } => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Some(d) = v.get("diff").and_then(|x| x.as_str()) {
                                    self.last_diff = Some(d.to_string());
                                } else if let Some(dp) =
                                    v.get("diff_preview").and_then(|x| x.as_str())
                                {
                                    self.last_diff = Some(dp.to_string());
                                }
                            }

                            {
                                let mut tool_name = String::new();
                                for msg in self.messages.iter().rev() {
                                    for it in &msg.items {
                                        if let Content::ToolCall { id: cid, name, .. } = it {
                                            if cid == &id {
                                                tool_name = name.clone();
                                                break;
                                            }
                                        }
                                    }
                                    if !tool_name.is_empty() {
                                        break;
                                    }
                                }
                                let full_out = if let Ok(v) =
                                    serde_json::from_str::<serde_json::Value>(&content)
                                {
                                    if let Some(log_path) = v.get("log").and_then(|x| x.as_str()) {
                                        if let Ok(full) = std::fs::read_to_string(log_path) {
                                            full
                                        } else if let Some(stdout) =
                                            v.get("stdout").and_then(|x| x.as_str())
                                        {
                                            let stderr = v
                                                .get("stderr")
                                                .and_then(|x| x.as_str())
                                                .unwrap_or("");
                                            if !stderr.is_empty() {
                                                format!("{stdout}\n--- stderr ---\n{stderr}")
                                            } else {
                                                stdout.to_string()
                                            }
                                        } else {
                                            content.clone()
                                        }
                                    } else if let Some(stdout) =
                                        v.get("stdout").and_then(|x| x.as_str())
                                    {
                                        let stderr =
                                            v.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
                                        if !stderr.is_empty() {
                                            format!("{stdout}\n--- stderr ---\n{stderr}")
                                        } else {
                                            stdout.to_string()
                                        }
                                    } else if let Some(d) = v
                                        .get("diff_preview")
                                        .or_else(|| v.get("diff"))
                                        .and_then(|x| x.as_str())
                                    {
                                        d.to_string()
                                    } else if let Some(c) =
                                        v.get("content").and_then(|x| x.as_str())
                                    {
                                        c.to_string()
                                    } else if let Some(e) = v.get("error").and_then(|x| x.as_str())
                                    {
                                        e.to_string()
                                    } else {
                                        content.clone()
                                    }
                                } else {
                                    content.clone()
                                };
                                let display_name = if tool_name.is_empty() {
                                    "tool".to_string()
                                } else {
                                    tool_name
                                };
                                self.last_tool_output =
                                    Some((id.clone(), display_name, full_out, ok));
                                self.tool_output_scroll = 0;
                            }

                            for msg in self.messages.iter_mut().rev() {
                                if msg.role != "system" {
                                    continue;
                                }
                                let mut found = false;
                                for item in &mut msg.items {
                                    if let Content::ToolCall {
                                        id: cid, status, ..
                                    } = item
                                    {
                                        if cid == &id {
                                            *status = if ok {
                                                ToolStatus::Done
                                            } else {
                                                ToolStatus::Error
                                            };
                                            found = true;
                                            break;
                                        }
                                    }
                                }
                                if found {
                                    let already = msg.items.iter().any(|it| matches!(it, Content::ToolResult { id: rid, .. } if rid == &id));
                                    if !already {
                                        msg.items.push(Content::ToolResult {
                                            id: id.clone(),
                                            content: content.clone(),
                                            ok,
                                        });
                                    }

                                    if let Some((rid, _, _, _)) = &self.running_tool {
                                        if rid == &id {
                                            self.running_tool = None;

                                            if self.status.starts_with("running:") {
                                                self.status = if ok {
                                                    "ready".into()
                                                } else {
                                                    "error".into()
                                                };
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        vioraharness_core::provider::ProviderEvent::Notice(msg) => {
                            self.ctx_freed_tokens += Self::parse_compact_freed(&msg);
                            self.status = msg;
                        }
                        _ => {}
                    }
                }
            }

            if self.busy {
                if let Some(handle) = &mut self.pending {
                    if handle.is_finished() {
                        let handle = self.pending.take().unwrap();
                        self.busy = false;
                        self.streaming_buf.clear();
                        match handle.await {
                            Ok(Ok(text)) => {
                                let mut msg = Msg::new("assistant", text);

                                if !self.thinking_buf.trim().is_empty() {
                                    msg.reasoning = Some(self.thinking_buf.clone());
                                }
                                self.thinking_buf.clear();
                                self.messages.push(msg);
                                self.status = "ready".into();
                            }
                            Ok(Err(e)) => {
                                self.messages
                                    .push(Msg::new("system", format!("error: {e:#}")));
                                self.status = "error".into();
                            }
                            Err(e) => {
                                self.messages
                                    .push(Msg::new("system", format!("join error: {e}")));
                                self.status = "error".into();
                            }
                        }
                    }
                }

                self.drain_queue();
            }

            if event::poll(Duration::from_millis(80))? {
                match event::read()? {
                    Event::Key(k) => {
                        if k.kind != KeyEventKind::Press {
                            continue;
                        }
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && k.modifiers.contains(KeyModifiers::ALT)
                        {
                            if self.selection.is_some() {
                                self.copy_pending = true;
                            } else {
                                self.status = "nothing selected — drag to select first".into();
                            }
                            continue;
                        }
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT)
                        {
                            if self.selection.is_some() {
                                self.copy_pending = true;
                            } else {
                                self.selection = None;
                                self.should_quit = true;
                                break;
                            }
                            continue;
                        }
                        if k.code == KeyCode::Char('g')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if !self.show_thinking {
                                self.show_thinking = true;
                                self.thinking_expanded = true;
                                Self::save_tui_state(serde_json::json!({"show_thinking": true}));
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "Thinking ON ({} / {}) — Ctrl+G to collapse",
                                        self.thinking_title, self.thinking_label
                                    ),
                                ));
                            } else {
                                if !self.toggle_recent_reasoning() {
                                    self.thinking_expanded = !self.thinking_expanded;
                                }
                            }
                            continue;
                        }

                        if k.code == KeyCode::Char('o')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && self.popup == Popup::None
                        {
                            self.toggle_recent_reasoning();
                            continue;
                        }

                        if (k.code == KeyCode::Char('x') || k.code == KeyCode::Char('X'))
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT)
                            && self.popup == Popup::None
                        {
                            self.toggle_recent_long_message();
                            continue;
                        }

                        if self.input.text.is_empty() && self.popup == Popup::None {
                            match k.code {
                                KeyCode::Char('j')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if self.scroll > 0 {
                                        self.scroll -= 1;
                                    }
                                    continue;
                                }
                                KeyCode::Char('k')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    self.scroll = self.scroll.saturating_add(1);
                                    continue;
                                }
                                KeyCode::Char('G') if k.modifiers.contains(KeyModifiers::SHIFT) => {
                                    self.scroll = 0;
                                    continue;
                                }
                                KeyCode::Char('g')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    self.scroll = 10000;
                                    continue;
                                }
                                _ => {}
                            }
                        }

                        if k.code == KeyCode::Char('f')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && self.popup == Popup::None
                        {
                            self.chat_search = Some(String::new());
                            self.messages
                                .push(Msg::new("system", "Search: type then Enter (Esc to clear)"));
                            continue;
                        }

                        if self.chat_search.is_some() {
                            match k.code {
                                KeyCode::Char(c)
                                    if !k.modifiers.contains(KeyModifiers::CONTROL)
                                        && !k.modifiers.contains(KeyModifiers::ALT) =>
                                {
                                    if let Some(q) = &mut self.chat_search {
                                        q.push(c);
                                    }
                                    continue;
                                }
                                KeyCode::Backspace => {
                                    if let Some(q) = &mut self.chat_search {
                                        q.pop();
                                    }
                                    continue;
                                }
                                KeyCode::Esc => {
                                    self.chat_search = None;
                                    self.messages.push(Msg::new("system", "Search cleared"));
                                    continue;
                                }
                                KeyCode::Enter => {
                                    let q = self.chat_search.clone().unwrap_or_default();
                                    let cnt = self
                                        .messages
                                        .iter()
                                        .filter(|m| m.content.contains(q.as_str()))
                                        .count();
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!("Search: '{}' — {} matches (Esc to clear)", q, cnt),
                                    ));
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        if k.code == KeyCode::Char('t')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            self.popup = if self.popup == Popup::ThemePicker {
                                Popup::None
                            } else {
                                Popup::ThemePicker
                            };
                            continue;
                        }
                        if k.code == KeyCode::F(12) {
                            let ts = chrono_like_now().replace(':', "-");
                            let path = format!("/tmp/vioraharness_screenshot_{}.txt", ts);
                            match self.save_screenshot(&path) {
                            Ok(_) => self.messages.push(Msg::new("system", format!("Screenshot saved → {} (120x40 TestBackend) — attach for vision debug", path))),
                            Err(e) => self.messages.push(Msg::new("system", format!("Screenshot failed: {e}"))),
                        }
                            continue;
                        }
                        if k.code == KeyCode::F(2) {
                            self.provider_cursor = 0;
                            self.provider_key_input.clear();
                            self.provider_input_active = false;
                            self.provider_selected = None;
                            self.provider_msg = None;
                            self.popup = if self.popup == Popup::Providers {
                                Popup::None
                            } else {
                                Popup::Providers
                            };
                            continue;
                        }

                        if self.popup == Popup::None && self.last_tool_output.is_some() {
                            let is_f9 = k.code == KeyCode::F(9);
                            let is_shift_v = k.code == KeyCode::Char('V')
                                && !k.modifiers.contains(KeyModifiers::CONTROL)
                                && !k.modifiers.contains(KeyModifiers::ALT);
                            let is_v_empty = k.code == KeyCode::Char('v')
                                && self.input.text.is_empty()
                                && !k.modifiers.contains(KeyModifiers::CONTROL)
                                && !k.modifiers.contains(KeyModifiers::ALT)
                                && self.popup == Popup::None;

                            if is_f9 || (is_shift_v && self.input.text.is_empty()) {
                                self.popup = Popup::ToolOutput;
                                self.tool_output_scroll = 0;
                                continue;
                            }

                            let _ = is_v_empty;
                        }
                        if k.code == KeyCode::Enter && k.modifiers.contains(KeyModifiers::ALT) {
                            self.input.insert('\n');
                            continue;
                        }
                        if k.code == KeyCode::Char('l')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            self.input.text.clear();
                            self.input.cursor = 0;
                            continue;
                        }

                        if self.popup == Popup::None {
                            if k.modifiers.contains(KeyModifiers::CONTROL)
                                && k.modifiers.contains(KeyModifiers::ALT)
                                && matches!(k.code, KeyCode::Char('v') | KeyCode::Char('V'))
                            {
                                if let Some(txt) = clipboard_paste_text() {
                                    if !txt.is_empty() {
                                        self.insert_pasted_text(&txt);
                                    } else {
                                        self.status = "clipboard empty".into();
                                    }
                                } else {
                                    self.status = "clipboard empty".into();
                                }
                                continue;
                            }
                            if k.modifiers.contains(KeyModifiers::CONTROL) {
                                match k.code {
                                    KeyCode::Char('u') | KeyCode::Char('U') => {
                                        self.input.delete_to_start();
                                        continue;
                                    }
                                    KeyCode::Char('k') | KeyCode::Char('K') => {
                                        self.input.delete_to_end();
                                        continue;
                                    }
                                    KeyCode::Char('a') | KeyCode::Char('A') => {
                                        self.input.move_to_start();
                                        continue;
                                    }
                                    KeyCode::Char('e') | KeyCode::Char('E') => {
                                        self.input.move_to_end();
                                        continue;
                                    }
                                    KeyCode::Char('w') | KeyCode::Char('W') => {
                                        self.input.delete_word_before();
                                        continue;
                                    }
                                    KeyCode::Char('h') | KeyCode::Char('H') => {
                                        self.input.backspace();
                                        continue;
                                    }
                                    KeyCode::Char('d') | KeyCode::Char('D') => {
                                        if !self.input.text.is_empty() {
                                            self.input.delete();
                                        }
                                        continue;
                                    }
                                    KeyCode::Char('v') | KeyCode::Char('V') => {
                                        let mut pasted = false;
                                        if let Some((b64, w, h)) = clipboard_paste_image_base64() {
                                            let label = format!("{w}×{h}");
                                            self.attach_pasted_image(b64, "image/png", label);
                                            pasted = true;
                                        }
                                        if let Some(txt) = clipboard_paste_text() {
                                            if !txt.is_empty() {
                                                if pasted {
                                                    if detect_image_path(&txt).is_none() {
                                                        self.insert_pasted_text(&txt);
                                                    }
                                                } else if let Some(path) = detect_image_path(&txt) {
                                                    match load_image_file(&path) {
                                                        Ok((b64, mime, label)) => {
                                                            self.attach_pasted_image(
                                                                b64, mime, label,
                                                            );
                                                        }
                                                        Err(e) => {
                                                            self.status = e;
                                                            self.insert_pasted_text(&txt);
                                                        }
                                                    }
                                                    pasted = true;
                                                } else {
                                                    self.insert_pasted_text(&txt);
                                                    pasted = true;
                                                }
                                            }
                                        }
                                        if pasted {
                                            continue;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            if k.modifiers.contains(KeyModifiers::ALT) {
                                match k.code {
                                    KeyCode::Char('d') | KeyCode::Char('D') => {
                                        self.input.delete_word_after();
                                        continue;
                                    }
                                    KeyCode::Backspace => {
                                        self.input.delete_word_before();
                                        continue;
                                    }
                                    KeyCode::Char('b') | KeyCode::Char('B') => {
                                        self.input.move_left();
                                        continue;
                                    }
                                    KeyCode::Char('f') | KeyCode::Char('F') => {
                                        self.input.move_right();
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                        }

                        if self.popup != Popup::None {
                            self.handle_popup_key(k);
                            continue;
                        }
                        self.handle_key(k.code).await?;
                    }
                    Event::Resize(_, _) => {
                        continue;
                    }
                    Event::Mouse(m) => {
                        self.handle_mouse(m);
                    }
                    _ => {}
                }
            } else if self.busy {
                self.tick = self.tick.wrapping_add(1);
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(6),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        self.chat_area = chunks[1];
        self.input_area = chunks[2];
        self.draw_header(frame, chunks[0]);
        self.draw_chat(frame, chunks[1]);
        self.draw_input(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);

        if self.popup != Popup::None {
            self.draw_popup(frame, area);
        } else if self.input.text.starts_with('/') {
            self.draw_completions(frame, chunks[2]);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let spinner = if self.busy {
            const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[self.tick % FRAMES.len()]
        } else {
            "○"
        };
        let status_style = if self.busy {
            Theme::header_status_busy()
        } else if self.status == "error" {
            Theme::header_status_error()
        } else {
            Theme::header_status_ready()
        };
        let mode_style = if self.mode == "plan" {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        };

        let busy_button = if self.busy {
            if let Some((_, name, preview, start)) = &self.running_tool {
                let secs = start.elapsed().as_secs();
                let label = if name == "bash" {
                    let short = truncate_chars(preview, 30);
                    if secs > 0 {
                        format!(" {spinner} BUSY: bash {short} ({}s) ", secs)
                    } else {
                        format!(" {spinner} BUSY: bash {short} ")
                    }
                } else {
                    if secs > 0 {
                        format!(" {spinner} BUSY: {name} ({}s) ", secs)
                    } else {
                        format!(" {spinner} BUSY: {name} ")
                    }
                };
                Some((label, Theme::busy_button_shell()))
            } else {
                Some((format!(" {spinner} BUSY "), Theme::busy_button()))
            }
        } else {
            None
        };

        let mut header_spans = vec![
            Span::styled(" VioraHarness ", Theme::header_title()),
            Span::styled(format!("[{}] ", self.mode.to_uppercase()), mode_style),
        ];
        if let Some((label, style)) = busy_button {
            header_spans.push(Span::styled(label, style));

            header_spans.push(Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            header_spans.push(Span::styled(format!("{} ", self.status), status_style));
            header_spans.push(Span::styled(
                format!("{spinner} {}", if self.busy { "busy" } else { "idle" }),
                status_style,
            ));
        }
        let header = Paragraph::new(Line::from({
            let mut v = header_spans;
            v.push(Span::raw(format!(" | {} msgs ", self.messages.len())));
            v.push(Span::styled(
                {
                    let total_chars: usize = self
                        .messages
                        .iter()
                        .map(|m| {
                            let mut n = m.content.len();
                            for it in &m.items {
                                match it {
                                    Content::ToolCall { args, .. } => n += args.len(),
                                    Content::ToolResult { content, .. } => n += content.len(),
                                    Content::Reasoning(r) => n += r.len(),
                                    _ => {}
                                }
                            }
                            if let Some(r) = &m.reasoning {
                                n += r.len();
                            }
                            n
                        })
                        .sum::<usize>()
                        + self.input.text.len()
                        + self.thinking_buf.len()
                        + self.streaming_buf.len();
                    let tokens = (total_chars / 4).saturating_sub(self.ctx_freed_tokens);
                    let ctx_len = *self.model_context.get(&self.model).unwrap_or(&128_000);
                    let ctx_k = if ctx_len >= 1000 {
                        format!("{}k", ctx_len / 1000)
                    } else {
                        ctx_len.to_string()
                    };
                    let used = if tokens >= 1000 {
                        let k = tokens as f32 / 1000.0;
                        let s = format!("{k:.1}k");
                        s.replace(".0k", "k")
                    } else {
                        tokens.to_string()
                    };
                    let pct = ((tokens as f32 / ctx_len as f32) * 100.0) as usize;
                    let pct = pct.min(100);
                    let bar = format!("{}{}", "▓".repeat(pct / 10), "░".repeat(10 - pct / 10));
                    format!("| {used}/{ctx_k} {pct}% {bar} ")
                },
                Style::default().fg(Color::DarkGray),
            ));
            if let Some(q) = &self.chat_search {
                v.push(Span::styled(
                    format!(" [{}] ", q),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            v
        }))
        .style(Theme::header_bg());
        frame.render_widget(header, area);
    }

    fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
        let mut all_lines: Vec<Line> = Vec::new();
        for (abs_idx, m) in self.messages.iter().enumerate() {
            let (prefix, style) = match m.role.as_str() {
                "user" => ("> ", Theme::user_prefix()),
                "assistant" => ("● ", Theme::assistant_prefix()),
                "system" => ("· ", Theme::system_prefix()),
                _ => ("  ", Style::default()),
            };

            let raw = if m.content.chars().count() > 20000 {
                let mut cut = truncate_chars(&m.content, 20000);

                if cut.lines().filter(|l| l.trim().starts_with("```")).count() % 2 == 1 {
                    cut.push_str("\n```");
                }
                format!(
                    "{cut}… ({} chars total, truncated at 20k — scroll or /export)",
                    m.content.chars().count()
                )
            } else {
                m.content.clone()
            };

            let (raw, collapsed_total) = if m.role == "user" {
                collapse_long_content(&raw, self.expanded_messages.contains(&abs_idx))
            } else {
                (raw, None)
            };
            let time = Span::styled(
                format!(" {} ", m.timestamp),
                Style::default().fg(Color::DarkGray),
            );
            if m.role == "assistant" {
                let mut in_code_block = false;
                let raw_lines: Vec<&str> = raw.lines().collect();

                let joined = join_math_lines(&raw_lines);
                let raw_lines: Vec<&str> = joined.iter().map(|s| s.as_str()).collect();

                if raw_lines.is_empty() {
                    let mut spans = markdown_inline_spans(&raw);
                    if let Some(q) = &self.chat_search {
                        if !q.is_empty() && raw.to_lowercase().contains(&q.to_lowercase()) {
                            spans = vec![Span::styled(
                                raw.clone(),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .bg(Color::Rgb(50, 50, 30))
                                    .add_modifier(Modifier::BOLD),
                            )];
                        }
                    }
                    let mut line_spans = Vec::new();
                    line_spans.push(Span::styled(prefix, style));
                    line_spans.extend(spans);
                    line_spans.push(time);
                    if self.busy && !self.streaming_buf.is_empty() {
                        line_spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    }
                    all_lines.push(Line::from(line_spans));
                } else {
                    let table_max_width = (area.width as usize).saturating_sub(4).max(20);
                    let mut skip_until = 0;
                    for (li, line_str) in raw_lines.iter().enumerate() {
                        if li < skip_until {
                            continue;
                        }
                        let trimmed = line_str.trim();

                        if trimmed.starts_with("```") {
                            let lang = code_lang_from_fence(trimmed);
                            let entering = !in_code_block;
                            in_code_block = !in_code_block;
                            if entering {
                                let mut spans =
                                    vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                                spans.extend(fence_open_spans(&lang));
                                if li == 0 {
                                    spans.push(time.clone());
                                }
                                all_lines.push(Line::from(spans));
                            } else {
                                let mut spans = vec![Span::styled("  ", style)];
                                spans.extend(fence_close_spans());

                                all_lines.push(Line::from(spans));
                            }
                            continue;
                        }

                        if !in_code_block
                            && (trimmed.starts_with("$$") || trimmed.starts_with("\\["))
                        {
                            if let Some((mrows, consumed)) =
                                render_math_block(&raw_lines[li..], table_max_width)
                            {
                                for (mi, mrow) in mrows.into_iter().enumerate() {
                                    let mut spans = vec![Span::styled(
                                        if li == 0 && mi == 0 { prefix } else { "  " },
                                        style,
                                    )];
                                    spans.extend(mrow);
                                    if li == 0 && mi == 0 {
                                        spans.push(time.clone());
                                    }
                                    all_lines.push(Line::from(spans));
                                }
                                skip_until = li + consumed;
                                continue;
                            }
                        }

                        if !in_code_block && trimmed.contains('|') {
                            if let Some((tbl, consumed)) =
                                render_table_block(&raw_lines[li..], table_max_width)
                            {
                                for (ti, tline) in tbl.into_iter().enumerate() {
                                    let mut spans = vec![Span::styled(
                                        if li == 0 && ti == 0 { prefix } else { "  " },
                                        style,
                                    )];
                                    spans.extend(tline);
                                    if li == 0 && ti == 0 {
                                        spans.push(time.clone());
                                    }
                                    all_lines.push(Line::from(spans));
                                }
                                skip_until = li + consumed;
                                continue;
                            }
                        }
                        if in_code_block {
                            let mut line_spans = Vec::new();
                            line_spans
                                .push(Span::styled(if li == 0 { prefix } else { "  " }, style));
                            line_spans.push(Span::styled("│ ", crate::theme::Theme::code_block()));
                            line_spans.extend(highlighted_code_spans(line_str));

                            if li == 0 {
                                line_spans.push(time.clone());
                            }
                            all_lines.push(Line::from(line_spans));
                            continue;
                        }
                        if trimmed.is_empty() {
                            let head =
                                vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                            let tail = if li == 0 { vec![time.clone()] } else { vec![] };
                            all_lines.push(chat_line(head, Vec::new(), tail));
                            continue;
                        }

                        if trimmed.starts_with('#') {
                            let level = trimmed.chars().take_while(|&c| c == '#').count();
                            let header_text = trimmed[level..].trim_start();
                            let hstyle = markdown_header_style(level.min(3));
                            let text_width = (area.width as usize).saturating_sub(2).max(20);
                            let gutter: Vec<Span> =
                                vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                            let mut tail: Vec<Span> = Vec::new();
                            if li == 0 {
                                tail.push(time.clone());
                            }

                            if li == 0 && self.busy && !self.streaming_buf.is_empty() {
                                tail.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                            }

                            if let Some(q) = &self.chat_search {
                                if !q.is_empty()
                                    && line_str.to_lowercase().contains(&q.to_lowercase())
                                {
                                    let hl = Style::default()
                                        .fg(Color::Yellow)
                                        .bg(Color::Rgb(50, 50, 30))
                                        .add_modifier(Modifier::BOLD);
                                    let mut spans = gutter;
                                    spans.push(Span::styled((*line_str).to_string(), hl));
                                    spans.extend(tail);
                                    all_lines.push(Line::from(spans));
                                    continue;
                                }
                            }

                            let patch = |s: Span<'static>| Span {
                                content: s.content,
                                style: hstyle.patch(s.style),
                            };
                            let segs: Vec<Span> = match split_list_marker(header_text) {
                                Some((marker, rest)) => {
                                    let mut v = vec![Span::styled(
                                        format!("{marker} "),
                                        hstyle.patch(Theme::title_marker()),
                                    )];
                                    v.extend(markdown_inline_spans(&rest).into_iter().map(patch));
                                    v
                                }
                                None => markdown_inline_spans(header_text)
                                    .into_iter()
                                    .map(patch)
                                    .collect(),
                            };
                            all_lines.extend(wrap_spans(
                                segs,
                                gutter,
                                vec![Span::raw("  ")],
                                tail,
                                text_width,
                            ));
                            continue;
                        }

                        let text_width = (area.width as usize).saturating_sub(2).max(20);
                        let gutter: Vec<Span> =
                            vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                        let mut tail: Vec<Span> = Vec::new();
                        if li == 0 {
                            tail.push(time.clone());
                        }
                        if li == 0 && self.busy && !self.streaming_buf.is_empty() {
                            tail.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                        }
                        let content = *line_str;

                        if let Some(q) = &self.chat_search {
                            if !q.is_empty() && content.to_lowercase().contains(&q.to_lowercase()) {
                                let mut spans = gutter;
                                spans.push(Span::styled(
                                    content.to_string(),
                                    Style::default()
                                        .fg(Color::Yellow)
                                        .bg(Color::Rgb(50, 50, 30))
                                        .add_modifier(Modifier::BOLD),
                                ));
                                spans.extend(tail);
                                all_lines.push(Line::from(spans));
                                continue;
                            }
                        }

                        if let Some((marker, rest)) = split_list_marker(trimmed) {
                            let nest = line_str
                                .chars()
                                .take_while(|c| *c == ' ' || *c == '\t')
                                .map(|c| if c == '\t' { 4 } else { 1 })
                                .sum::<usize>();
                            let title = is_section_title(&rest);
                            let marker_style = if title {
                                Theme::title_marker()
                            } else {
                                Theme::list_marker()
                            };
                            let mut segs = vec![Span::styled(format!("{marker} "), marker_style)];
                            segs.extend(markdown_inline_spans(&rest));

                            let hang = 2 + nest + marker.width() + 1;
                            let cont: Vec<Span> =
                                vec![Span::raw(" ".repeat(hang.min(text_width - 1)))];

                            let mut head = gutter;
                            if nest > 0 {
                                head.push(Span::raw(" ".repeat(nest.min(text_width - 1))));
                            }
                            all_lines.extend(wrap_spans(segs, head, cont, tail, text_width));
                            continue;
                        }
                        let inner_spans = markdown_inline_spans(content);
                        all_lines.extend(wrap_spans(
                            inner_spans,
                            gutter,
                            vec![Span::raw("  ")],
                            tail,
                            text_width,
                        ));
                    }

                    if in_code_block {
                        let mut spans = vec![Span::styled("  ", style)];
                        spans.extend(fence_close_spans());
                        all_lines.push(Line::from(spans));
                    }
                }
            } else {
                let mut display = raw.lines().next().unwrap_or("").to_string();
                let rest: String = raw.lines().skip(1).collect::<Vec<_>>().join(" ");
                if !rest.is_empty() {
                    if display.is_empty() {
                        display = rest;
                    } else {
                        display = format!("{display} {rest}");
                    }
                }
                let is_error_msg = m.role == "system"
                    && (display.to_lowercase().contains("error")
                        || display.contains("✖")
                        || display.contains("denied")
                        || display.contains("blocked")
                        || display.contains("FILE NOT CREATED")
                        || display.contains("failed"));
                let is_code = display.trim_start().starts_with("```") || raw.contains("```");
                let mut content_span = if is_code {
                    Span::styled(
                        display.clone(),
                        Style::default()
                            .bg(Color::Rgb(49, 50, 68))
                            .fg(Color::Rgb(205, 214, 244)),
                    )
                } else if is_error_msg {
                    Span::styled(display.clone(), crate::theme::Theme::error_message())
                } else {
                    Span::raw(display.clone())
                };
                if let Some(q) = &self.chat_search {
                    if !q.is_empty() && display.to_lowercase().contains(&q.to_lowercase()) {
                        content_span = Span::styled(
                            display.clone(),
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::Rgb(50, 50, 30))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
                let effective_prefix = if is_error_msg { "✖ " } else { prefix };
                let effective_style = if is_error_msg {
                    crate::theme::Theme::error_prefix()
                } else {
                    style
                };
                let line = Line::from(vec![
                    Span::styled(effective_prefix, effective_style),
                    content_span,
                    time,
                ]);
                all_lines.push(line);
            }

            let tool_names: std::collections::HashMap<&str, &str> = m
                .items
                .iter()
                .filter_map(|it| {
                    if let Content::ToolCall { id, name, .. } = it {
                        Some((id.as_str(), name.as_str()))
                    } else {
                        None
                    }
                })
                .collect();
            for it in &m.items {
                match it {
                    Content::ToolCall {
                        id,
                        name,
                        args,
                        status,
                    } => {
                        let verbosity = self.tool_verbosity(name);
                        if verbosity == ToolVerbosity::Hidden {
                            continue;
                        }
                        let icon = match status {
                            ToolStatus::Pending => "○",
                            ToolStatus::Running => "◐",
                            ToolStatus::Done => "●",
                            ToolStatus::Error => "✖",
                        };
                        let col = match status {
                            ToolStatus::Done => Color::Green,
                            ToolStatus::Error => Color::Red,
                            ToolStatus::Running => Color::Yellow,
                            _ => Color::DarkGray,
                        };
                        let preview = if verbosity == ToolVerbosity::Quiet {
                            String::new()
                        } else {
                            pretty_tool_args_wide(name, args, verbosity == ToolVerbosity::Full)
                        };

                        let elapsed_suffix = if *status == ToolStatus::Running {
                            if let Some((rid, _, _, start)) = &self.running_tool {
                                let is_running = rid == id;
                                if is_running || *name == "bash" {
                                    let secs = start.elapsed().as_secs();
                                    if secs > 0 {
                                        format!(" ({}s)", secs)
                                    } else {
                                        " (running)".to_string()
                                    }
                                } else {
                                    String::new()
                                }
                            } else if *name == "bash" {
                                " (running)".to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        };
                        let preview_with_elapsed = if elapsed_suffix.is_empty() {
                            preview
                        } else {
                            format!("{preview}{elapsed_suffix}")
                        };
                        let preview_style = if *status == ToolStatus::Error {
                            crate::theme::Theme::error_message()
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        let tline = Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                format!("{icon} {name} "),
                                Style::default().fg(col).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(preview_with_elapsed, preview_style),
                        ]);
                        all_lines.push(tline);
                    }
                    Content::ToolResult { id, content, ok } => {
                        let icon = if *ok { "✔" } else { "✖" };
                        let col = if *ok { Color::Green } else { Color::Red };

                        let tname = tool_names.get(id.as_str()).copied().unwrap_or("");
                        let mut verbosity = self.tool_verbosity(tname);
                        if !ok && verbosity != ToolVerbosity::Full {
                            verbosity = ToolVerbosity::Compact;
                        }
                        if verbosity == ToolVerbosity::Hidden {
                            continue;
                        }

                        let preview = if verbosity == ToolVerbosity::Quiet {
                            String::new()
                        } else {
                            let wide = verbosity == ToolVerbosity::Full;
                            let pretty = pretty_tool_result_wide(content, wide);
                            let cap = if wide { 900 } else { 240 };
                            if pretty.chars().count() > cap {
                                format!("{}…", truncate_chars(&pretty, cap))
                            } else {
                                pretty
                            }
                        };
                        let preview_style = if *ok {
                            Style::default().fg(Color::White)
                        } else {
                            crate::theme::Theme::error_message()
                        };

                        let label: String = if tname.is_empty() {
                            id.chars().take(12).collect()
                        } else {
                            tname.to_string()
                        };
                        let rline = Line::from(vec![
                            Span::raw("      "),
                            Span::styled(
                                format!("{icon} {label} "),
                                Style::default().fg(col).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(preview, preview_style),
                        ]);
                        all_lines.push(rline);

                        if (tname == "write" || tname == "edit")
                            && *ok
                            && (verbosity == ToolVerbosity::Compact
                                || verbosity == ToolVerbosity::Full)
                        {
                            if let Some(d) = result_diff_text(content) {
                                let (changes, rest) = diff_card_lines(&d, 4);
                                for (is_add, text) in changes {
                                    all_lines.push(Line::from(vec![
                                        Span::raw("      "),
                                        Span::styled(
                                            format!("{} {text}", if is_add { "+" } else { "-" }),
                                            Style::default().fg(if is_add {
                                                Color::Green
                                            } else {
                                                Color::Red
                                            }),
                                        ),
                                    ]));
                                }
                                if rest > 0 {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("      … (+{rest} more — V or /diff for full)"),
                                        Style::default()
                                            .fg(Color::DarkGray)
                                            .add_modifier(Modifier::ITALIC),
                                    )));
                                }
                            }
                        }
                    }
                    Content::Reasoning(r) => {
                        let rline = Line::from(vec![
                            Span::raw("    "),
                            Span::styled(format!("… {r}"), Theme::reasoning_prefix()),
                        ]);
                        all_lines.push(rline);
                    }
                    _ => {}
                }
            }

            if self.show_thinking && m.role == "assistant" {
                if let Some(reasoning) = &m.reasoning {
                    if !reasoning.trim().is_empty() {
                        let is_expanded =
                            self.expanded_reasoning.contains(&abs_idx) || self.thinking_expanded;
                        let collapsed = !is_expanded;

                        let glyph = if collapsed { "✦" } else { "✧" };
                        let hint = if collapsed {
                            "[Ctrl+O/Ctrl+G to expand]"
                        } else {
                            "[Ctrl+O/Ctrl+G to collapse]"
                        };
                        let tline = Line::from(vec![
                            Span::styled(
                                format!(" {glyph} {}… ", self.thinking_title),
                                Theme::thinking_title_style(),
                            ),
                            Span::styled(
                                format!("· {} chars ", reasoning.len()),
                                Theme::thinking_label_style(),
                            ),
                            Span::styled(
                                hint,
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]);
                        all_lines.push(tline);
                        if !collapsed {
                            for line in reasoning.lines().take(12) {
                                all_lines.push(Line::from(vec![
                                    Span::styled(" │ ", Theme::thinking_icon_style()),
                                    Span::styled(line.to_string(), Theme::reasoning_prefix()),
                                ]));
                            }
                            if reasoning.lines().count() > 12 {
                                all_lines.push(Line::from(Span::styled(
                                    format!("   … ({} lines total)", reasoning.lines().count()),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                )));
                            }
                        }
                    }
                }
            }

            if let Some(total) = collapsed_total {
                all_lines.push(Line::from(Span::styled(
                    format!(
                        "  … (+{} lines — Ctrl+X to expand)",
                        total.saturating_sub(CHAT_COLLAPSE_HEAD)
                    ),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }

        if self.show_thinking {
            let has_thinking = !self.thinking_buf.is_empty() || self.busy;
            if has_thinking {
                let title = &self.thinking_title;
                let collapsed = !self.thinking_expanded;
                let glyph = if collapsed { "✦" } else { "✧" };
                let count = if self.thinking_buf.is_empty() {
                    "live".to_string()
                } else {
                    format!("{} chars", self.thinking_buf.len())
                };
                let tline = Line::from(vec![
                    Span::styled(format!(" {glyph} {title}… "), Theme::thinking_title_style()),
                    Span::styled(format!("· {count} "), Theme::thinking_label_style()),
                    Span::styled(
                        if collapsed {
                            "[Ctrl+O/Ctrl+G to expand]"
                        } else {
                            "[Ctrl+O/Ctrl+G to collapse]"
                        },
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]);
                all_lines.push(tline);
                if !collapsed {
                    let reasoning_preview = if self.thinking_buf.is_empty() {
                        if self.busy {
                            "Reasoning…"
                        } else {
                            "(no reasoning captured — model may not support thinking)"
                        }
                    } else {
                        &self.thinking_buf
                    };
                    for line in reasoning_preview.lines().take(6) {
                        let rline = Line::from(vec![
                            Span::styled(" │ ", Theme::thinking_icon_style()),
                            Span::styled(line.to_string(), Theme::reasoning_prefix()),
                        ]);
                        all_lines.push(rline);
                    }
                    if self.busy && self.thinking_buf.len() > 200 {
                        all_lines.push(Line::from(Span::styled(
                            "   … streaming…",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
        }

        if self.busy
            && (self.messages.is_empty()
                || self
                    .messages
                    .last()
                    .map(|m| m.role != "assistant")
                    .unwrap_or(true))
            && !self.streaming_buf.is_empty()
            && !self.show_thinking
        {
            let sraw = self.streaming_buf.clone();
            let s_lines: Vec<&str> = sraw.lines().collect();
            let s_joined = join_math_lines(&s_lines);
            let s_lines: Vec<&str> = s_joined.iter().map(|s| s.as_str()).collect();
            if s_lines.is_empty() {
                let mut spans = vec![Span::styled("● ", Theme::assistant_prefix())];
                spans.extend(markdown_inline_spans(&sraw));
                spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                all_lines.push(Line::from(spans));
            } else {
                let stream_table_w = (area.width as usize).saturating_sub(4).max(20);
                let mut stream_in_code = false;
                let mut stream_skip_until = 0;
                for (idx, seg) in s_lines.iter().enumerate() {
                    if idx < stream_skip_until {
                        continue;
                    }
                    let trimmed = seg.trim();
                    if trimmed.starts_with("```") {
                        let lang = code_lang_from_fence(trimmed);
                        let entering = !stream_in_code;
                        stream_in_code = !stream_in_code;
                        let mut spans = vec![Span::styled(
                            if idx == 0 { "● " } else { "  " },
                            Theme::assistant_prefix(),
                        )];
                        if entering {
                            spans.extend(fence_open_spans(&lang));
                        } else {
                            spans.extend(fence_close_spans());
                        }
                        if idx == s_lines.len() - 1 {
                            spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                        }
                        all_lines.push(Line::from(spans));
                        continue;
                    }

                    if !stream_in_code && trimmed.contains('|') {
                        if let Some((tbl, consumed)) =
                            render_table_block(&s_lines[idx..], stream_table_w)
                        {
                            for (ti, tline) in tbl.into_iter().enumerate() {
                                let mut spans = vec![Span::styled(
                                    if idx == 0 && ti == 0 { "● " } else { "  " },
                                    Theme::assistant_prefix(),
                                )];
                                spans.extend(tline);
                                if idx + ti + 1 >= s_lines.len() {
                                    spans.push(Span::styled(
                                        " ▌",
                                        Style::default().fg(Color::Yellow),
                                    ));
                                }
                                all_lines.push(Line::from(spans));
                            }
                            stream_skip_until = idx + consumed;
                            continue;
                        }
                    }

                    if !stream_in_code && (trimmed.starts_with("$$") || trimmed.starts_with("\\["))
                    {
                        if let Some((mrows, consumed)) =
                            render_math_block(&s_lines[idx..], stream_table_w)
                        {
                            for (mi, mrow) in mrows.into_iter().enumerate() {
                                let mut spans = vec![Span::styled(
                                    if idx == 0 && mi == 0 { "● " } else { "  " },
                                    Theme::assistant_prefix(),
                                )];
                                spans.extend(mrow);
                                if idx + mi + 1 >= s_lines.len() {
                                    spans.push(Span::styled(
                                        " ▌",
                                        Style::default().fg(Color::Yellow),
                                    ));
                                }
                                all_lines.push(Line::from(spans));
                            }
                            stream_skip_until = idx + consumed;
                            continue;
                        }
                    }
                    let mut spans = vec![Span::styled(
                        if idx == 0 { "● " } else { "  " },
                        Theme::assistant_prefix(),
                    )];
                    if stream_in_code {
                        spans.push(Span::styled("│ ", crate::theme::Theme::code_block()));
                        spans.extend(highlighted_code_spans(seg));
                    } else {
                        spans.extend(markdown_inline_spans(seg));
                    }
                    if idx == s_lines.len() - 1 {
                        spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    }
                    all_lines.push(Line::from(spans));
                }

                if stream_in_code {
                    let mut spans = vec![Span::styled("  ", Theme::assistant_prefix())];
                    spans.extend(fence_close_spans());
                    spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    all_lines.push(Line::from(spans));
                }
            }
        }

        if self.copy_pending {
            self.copy_pending = false;
            let stale = self
                .selection
                .as_ref()
                .is_some_and(|s| s.session != self.session_id);
            if stale {
                self.selection = None;
            }
            if let Some(sel) = self.selection.clone() {
                let (top, bottom) = sel.ordered();
                if top.line < all_lines.len() {
                    let text = extract_selection_text(&all_lines, top, bottom);
                    if text.trim().is_empty() {
                        self.selection = None;
                        self.status = "nothing to copy".into();
                    } else {
                        match clipboard_copy_text(&text) {
                            Ok((n, via)) => {
                                self.status = format!(
                                    "copied {n} chars via {via} — selection kept (Esc clears)"
                                );
                            }
                            Err(e) => {
                                self.status = format!("copy failed ({e}) — selection kept");
                            }
                        }
                    }
                } else {
                    self.selection = None;
                }
            }
        }

        let stale = self
            .selection
            .as_ref()
            .is_some_and(|s| s.session != self.session_id || self.messages.len() < s.msg_len);
        if stale {
            self.selection = None;
        } else if let Some(sel) = self.selection.as_mut() {
            sel.msg_len = self.messages.len();
        }
        let visible = (area.height as usize).saturating_sub(2).max(5);
        let total = all_lines.len();
        if self.scroll > 0 {
            if total > self.chat_total_lines {
                self.scroll += total - self.chat_total_lines;
            }

            let max_scroll = total.saturating_sub(visible);
            if self.scroll > max_scroll {
                self.scroll = max_scroll;
            }
        }
        self.chat_total_lines = total;
        let start = total.saturating_sub(visible + self.scroll);
        let end = (start + visible).min(total);

        self.view_start = start;
        self.view_total = total;
        let sel_range = self
            .selection
            .as_ref()
            .filter(|s| s.session == self.session_id)
            .map(|s| s.ordered());

        let inner_w = self
            .chat_inner()
            .map(|r| r.width as usize)
            .unwrap_or(80)
            .max(8);

        let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
        self.vis_rows.clear();
        for (i, line) in all_lines[start..end].iter().enumerate() {
            let li = start + i;
            let mut base: Vec<Span<'static>> = line.spans.clone();
            if let Some((top, bottom)) = sel_range {
                if li >= top.line && li <= bottom.line {
                    let s = if li == top.line { top.col } else { 0 };
                    let e = if li == bottom.line {
                        bottom.col
                    } else {
                        usize::MAX
                    };
                    if s < e {
                        base = highlight_range(&line.spans, s, e, Theme::text_selection());
                    }
                }
            }
            for (row_spans, col_start) in wrap_line_preserving(&Line::from(base), inner_w) {
                self.vis_rows.push((li, col_start));
                lines.push(Line::from(row_spans));
            }
        }

        let title = format!(
            " Chat {} msgs {} ",
            self.messages.len(),
            if self.scroll > 0 {
                format!("↑{} ", self.scroll)
            } else {
                "".to_string()
            }
        );
        let chat = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Theme::card_border())
                    .style(Theme::chat_bg()),
            )
            .wrap(Wrap { trim: false })
            .style(Theme::chat_bg());
        frame.render_widget(chat, area);
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let model_short = self.model.split('/').next_back().unwrap_or(&self.model);
        let title = if self.busy {
            format!(
                " Input — {} [{}] (busy — type ahead, Enter queues • Esc cancels turn) ",
                model_short,
                self.mode.to_uppercase()
            )
        } else if let Some(img) = &self.pending_image {
            format!(
                " Input — {} [{}] • [image {} pending] • Enter send • Esc clear ",
                model_short,
                self.mode.to_uppercase(),
                img.label
            )
        } else {
            format!(
                " Input — {} [{}] • Enter send • Tab → {} • ↑/↓ history ",
                model_short,
                self.mode.to_uppercase(),
                if self.mode == "plan" { "BUILD" } else { "PLAN" }
            )
        };
        let display_text = if self.input.text.is_empty() && !self.busy {
            "Type a prompt or /help for commands…"
        } else if self.input.text.is_empty() {
            "Type ahead — Enter queues, Esc cancels the turn…"
        } else {
            self.input.text.as_str()
        };
        let style = if self.input.text.is_empty() && !self.busy {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        } else if self.busy {
            Theme::input_border_busy()
        } else {
            Style::default()
        };
        let input = Paragraph::new(display_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .style(Theme::panel_bg())
                    .border_style(if self.busy {
                        Theme::input_border_busy()
                    } else {
                        Theme::input_border()
                    }),
            )
            .wrap(Wrap { trim: false })
            .style(Theme::panel_bg().patch(style));
        frame.render_widget(input, area);

        let cursor_x = area.x + 1 + self.input.cursor as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position((cursor_x.min(area.x + area.width - 2), cursor_y));
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        let rel = rel_path(&cwd);

        let tasks = vioraharness_core::tools::tasks::list_tasks();
        let running = tasks
            .iter()
            .filter(|t| t.status == vioraharness_core::tools::tasks::BgStatus::Running)
            .count();
        let total = tasks.len();
        let queued = self.queued_prompts.len();
        let mut suffix: Vec<Span> = Vec::new();
        if total > 0 {
            suffix.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            if running > 0 {
                suffix.push(Span::styled(
                    format!("◐ {running} running "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            suffix.push(Span::styled(
                if total == 1 {
                    "1 task".to_string()
                } else {
                    format!("{total} tasks")
                },
                Style::default().fg(if running > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            ));
            suffix.push(Span::styled(
                " [/tasks]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        if queued > 0 {
            suffix.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            suffix.push(Span::styled(
                if queued == 1 {
                    "▸1 queued".to_string()
                } else {
                    format!("▸{queued} queued")
                },
                Style::default().fg(Color::Cyan),
            ));
        }
        let suffix_len: usize = suffix.iter().map(|s| s.content.chars().count()).sum();
        let max_cwd = (area.width as usize).saturating_sub(suffix_len + 5).max(8);
        let display = if rel.chars().count() > max_cwd {
            format!(
                "…{}",
                rel.chars()
                    .skip(rel.chars().count() - max_cwd)
                    .collect::<String>()
            )
        } else {
            rel
        };

        let mut spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled(display, Style::default().fg(Color::DarkGray)),
        ];
        spans.extend(suffix);
        let footer = Paragraph::new(Line::from(spans))
            .style(Theme::footer())
            .alignment(Alignment::Left);
        frame.render_widget(footer, area);
    }

    fn draw_completions(&self, frame: &mut Frame, input_area: Rect) {
        let comps = self.input.slash_completions();
        if comps.is_empty() {
            return;
        }
        let height = (comps.len() as u16 + 2).min(8);
        let popup_area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width: 40.min(input_area.width),
            height,
        };
        frame.render_widget(Clear, popup_area);
        let items: Vec<ListItem> = comps
            .iter()
            .map(|(c, d)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        *c,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(*d, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" completions ")
                .border_style(Theme::popup_border()),
        );
        frame.render_widget(list, popup_area);
    }

    fn draw_popup(&self, frame: &mut Frame, area: Rect) {
        let popup_area = centered_rect(60, 60, area);
        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(match self.popup {
                Popup::Help => " Help — VioraHarness TUI ",
                Popup::Sessions => " Sessions (SQLite) ",
                Popup::ModelPicker => " Model Picker ",
                Popup::Permissions => " Permissions ",
                Popup::ThemePicker => " Theme Picker ",
                Popup::Providers => " Providers — API Keys ",
                Popup::Diff => " Diff — Last Write ",
                Popup::PermissionAsk => " Permission required ",
                Popup::Question => " Agent question ",
                Popup::ToolOutput => " Tool Output — Full ",
                Popup::Skills => " Skills — auto-loaded on intent ",
                Popup::Tasks => " Background Tasks ",
                Popup::None => " ",
            })
            .border_style(Theme::popup_border())
            .style(Style::default().bg(Color::Rgb(30, 30, 46)));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let content = match self.popup {
            Popup::Help => vec![
                Line::from(Span::styled("Keybinds", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  Enter      send prompt (Tab to autocomplete) • Alt+Enter newline • Enter while busy queues next"),
                Line::from("  Esc        clear selection / input / cancel busy / clear pending image"),
                Line::from("  Ctrl-C     copy selection, else quit"),
                Line::from("  Ctrl-Alt-C copy selection (never quits)"),
                Line::from("  Mouse      drag select+auto-copy (edge auto-scroll) • wheel scroll • middle-click pastes • Shift+drag native select"),
                Line::from("  Tab        autocomplete slash / toggle plan/build"),
                Line::from("  ↑/↓        history (prev/next prompt) • PageUp/Down scroll chat • Ctrl+J/K vim scroll"),
                Line::from("  Ctrl-U/K   delete to start/end • Ctrl-W del word • Alt+D del next word"),
                Line::from("  Ctrl-A/E   start/end • Ctrl-H backspace • Ctrl-D delete • Alt+B/F word move"),
                Line::from("  Ctrl-V     paste text • Ctrl+Alt+V text only • Ctrl+V image → vision (pending)"),
                Line::from("  Ctrl-G/O   toggle thinking • Ctrl-F search • Ctrl-L clear • Ctrl-T theme"),
                Line::from("  Ctrl-X     expand/collapse long message • /compact now or auto at 80%"),
                Line::from("  F1         this help • F2 providers • F9 tool output • F12 screenshot"),
                Line::from("  F9/Shift+V /output  view last bash/tool full output (scroll for very long)"),
                Line::from("  Permissions  Ask popup → Allow once (a) / Allow always (A) saves to vioraharness.json"),
                Line::from("  Running      header `running: bash …` + chat `◐ bash … (3s)` spinner when shell is executing"),
                Line::from(""),
                Line::from(Span::styled("Slash commands", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  /help      this help"),
                Line::from("  /clear     clear chat"),
                Line::from("  /sessions /chats /history /ls  list chats (SQLite)"),
                Line::from("  /resume /r /open <id>  resume chat (prefix ok)"),
                Line::from("  /new /fork /rename /archive /delete /export  chats"),
                Line::from("  /providers manage API keys (F2)"),
                Line::from("  /model <name>  switch model (e.g. provider/model-id from /model picker)"),
                Line::from("  /theme [name]  switch theme (tokyonight[-soft]/eye-comfort/warm-dark/catppuccin/dracula/gruvbox/nord/system)"),
                Line::from("  /thinking on/off  toggle thinking block (or Ctrl+O)"),
                Line::from("  /undo      undo last file snapshot"),
                Line::from("  /compact   summarize + squash history (auto at 80%)"),
                Line::from("  /output    view last tool output (bash very long, F9)"),
                Line::from("  /tasks     background tasks — list, logs, kill (bash background:true)"),
                Line::from("  /skills    list skills • /skill-new [--local] <what it should do> (AI writes it)"),
                Line::from("  /quit /q /exit  quit"),
                Line::from(""),
                Line::from(Span::styled("Markdown", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  **bold** *italic* `code` # header - list • live streaming styled"),
                Line::from(""),
                Line::from(Span::styled("Workflow", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  Netlist-first: .cir → netlist_run --measure --assert → raw_export → schematic_render"),
                Line::from(""),
                Line::from(Span::styled(format!("Theme: {}  Thinking: {} ({} / {})", crate::theme::ThemeConfig::load().theme, if self.show_thinking { "on" } else { "off" }, self.thinking_title, self.thinking_label), Style::default().fg(Color::DarkGray))),
            ],
            Popup::Skills => {
                let skills = vioraharness_core::skills::list_skills();
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(
                    format!(
                        " Skills — {} found (auto-load on intent, /skill-new to create)",
                        skills.len()
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                if skills.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No skills yet — run /skill-new to create one.",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                for sk in &skills {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("● {} ", sk.name),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(sk.description.clone()),
                    ]));
                    lines.push(Line::from(Span::styled(
                        format!("  triggers: {}  •  {}", sk.triggers.join(", "), sk.path.display()),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " any key closes • /skill-new creates (global default, --local for ./skills)",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                lines
            }
            Popup::Sessions => {
                let db = std::env::var("VIORAHARNESS_DB").unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                let mut lines: Vec<Line> = Vec::new();

                let sess_result = vioraharness_core::session::SessionStore::new(&db)
                    .and_then(|store| store.list_sessions_filtered(None, None, true, 50, 0));
                match sess_result {
                    Ok(all) if all.is_empty() => {
                        lines.push(Line::from(Span::styled("No chats yet. Run a prompt or /new to create one.", Style::default().fg(Color::Yellow))));
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(" Shortcuts: n=new  Enter=resume (when list has items)", Style::default().fg(Color::DarkGray))));
                    }
                    Ok(all) => {

                        let filter = self.session_filter.to_lowercase();
                        let filtered: Vec<_> = all.into_iter().filter(|s| {
                            if filter.is_empty() { true } else {
                                s.id.to_lowercase().contains(&filter) || s.title.as_ref().map(|t| t.to_lowercase().contains(&filter)).unwrap_or(false) || s.model.to_lowercase().contains(&filter)
                            }
                        }).collect();
                        let total = filtered.len();

                        let filter_display = if self.session_filter.is_empty() { "type to filter…".to_string() } else { self.session_filter.clone() };
                        let filter_style = if self.session_filter.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) };
                        lines.push(Line::from(vec![
                            Span::styled(" Filter: ", Style::default().fg(Color::Yellow)),
                            Span::styled(filter_display, filter_style),
                            Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                            Span::raw(format!("  ({} shown{})", total, if filter.is_empty() { "" } else { " filtered" })),
                        ]));
                        lines.push(Line::from(Span::styled(
                            format!(" Chats — {} saved • {} msgs current • {}  • Enter resume • n new • f fork • r rename • a archive • d delete • Esc close ", total, self.messages.len(), &self.session_id[..8.min(self.session_id.len())]),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                        lines.push(Line::from(Span::styled(" ───────────────────────────────────────────────────────", Style::default().fg(Color::DarkGray))));
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                        if filtered.is_empty() {
                            lines.push(Line::from(Span::styled(format!("No match for '{}'", self.session_filter), Style::default().fg(Color::Red))));
                        } else {


                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(5).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(6).max(3);
                        }
                        let cursor = self.session_cursor.min(total.saturating_sub(1));
                        let mut start = 0;
                        if total > visible {
                            if cursor >= visible {
                                start = cursor + 1 - visible;
                            }
                            if start + visible > total {
                                start = total - visible;
                            }
                        }
                        let end = (start + visible).min(total);
                        for (idx, sess) in filtered[start..end].iter().enumerate() {
                            let i = start + idx;
                            let is_selected = i == cursor;
                            let is_current = sess.id == self.session_id;
                            let style = if is_selected { Theme::selection() } else { Style::default() };
                            let star = if is_selected { "▶ " } else { "  " };
                            let cur = if is_current { "● " } else { "  " };
                            let dt = {
                                let age = now.saturating_sub(sess.updated_at);
                                if age < 60 { format!("{}s ago", age) }
                                else if age < 3600 { format!("{}m ago", age/60) }
                                else if age < 86400 { format!("{}h ago", age/3600) }
                                else { format!("{}d ago", age/86400) }
                            };
                            let title = sess.title.clone().unwrap_or_else(|| "(untitled)".into());
                            let title_short = if title.chars().count() > 28 { format!("{}…", title.chars().take(28).collect::<String>()) } else { title };
                            let model_short = sess.model.split('/').next_back().unwrap_or(&sess.model);
                            let parent_mark = if sess.parent_id.is_some() { "⑂ " } else { "" };
                            lines.push(Line::from(vec![
                                Span::raw(star),
                                Span::styled(format!("{cur}{} ", &sess.id[..8.min(sess.id.len())]), style),
                                Span::styled(format!("{:<14}", model_short), Style::default().fg(Color::Cyan)),
                                Span::styled(title_short.clone(), if is_selected { style } else { Style::default().fg(Color::White) }),
                                Span::raw(" "),
                                Span::styled(format!("{parent_mark}{dt}"), Style::default().fg(Color::DarkGray)),
                            ]));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(format!(" — {}/{} chats (showing {}-{})", end, total, start + 1, end), Style::default().fg(Color::DarkGray))));
                        }
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(" ↑/↓ • PgUp/PgDn page • Enter resume • n new • f fork • r rename • a archive • d delete • Esc close ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    }
                    }
                    Err(e) => {
                        lines.push(Line::from(Span::styled(format!("db error: {e}"), Style::default().fg(Color::Red))));
                    }
                }
                lines
            }
            Popup::ModelPicker => {
                let has_gemini = has_key("GEMINI_API_KEY");
                let has_openrouter = has_key("OPENROUTER_API_KEY");
                let has_gateway = has_key("OPENCODE_API_KEY") || has_key("ZEN_API_KEY");
                let mut lines: Vec<Line> = Vec::new();

                let open_dot = if has_openrouter { "●" } else { "○" };
                let gem_dot = if has_gemini { "●" } else { "○" };
                let gw_dot = if has_gateway { "●" } else { "○" };
                lines.push(Line::from(vec![
                    Span::styled(" Keys: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("OpenRouter {open_dot} "), Style::default().fg(if has_openrouter { Color::Green } else { Color::DarkGray })),
                    Span::styled(format!("Gemini {gem_dot} "), Style::default().fg(if has_gemini { Color::Green } else { Color::DarkGray })),
                    Span::styled(format!("Gateway {gw_dot} "), Style::default().fg(if has_gateway { Color::Green } else { Color::DarkGray })),
                    Span::styled("— /providers to add keys (free tier is public)", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                ]));
                lines.push(Line::from(""));

                let filter_display = if self.model_filter.is_empty() { "type to filter…".to_string() } else { self.model_filter.clone() };
                let filter_style = if self.model_filter.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) };
                lines.push(Line::from(vec![
                    Span::styled(" Filter: ", Style::default().fg(Color::Yellow)),
                    Span::styled(filter_display, filter_style),
                    Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("  ({} shown)", {
                        let f = self.model_filter.to_lowercase();
                        if f.is_empty() { self.available_models.len() } else { self.available_models.iter().filter(|m| m.to_lowercase().contains(&f)).count() }
                    })),
                ]));
                lines.push(Line::from(Span::styled(" ─────────────────────────────────", Style::default().fg(Color::DarkGray))));
                if self.available_models.is_empty() {
                    lines.push(Line::from(Span::styled("No models available — set a key and restart TUI", Style::default().fg(Color::Red))));
                } else {
                    let filter = self.model_filter.to_lowercase();
                    let filtered: Vec<(usize, &String)> = self.available_models.iter().enumerate().filter(|(_, m)| filter.is_empty() || m.to_lowercase().contains(&filter)).collect();
                    if filtered.is_empty() {
                        lines.push(Line::from(Span::styled(format!("No match for '{}'", self.model_filter), Style::default().fg(Color::Red))));
                    } else {



                        let total = filtered.len();
                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(6).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(7).max(3);
                        }
                        let cursor = self.model_cursor.min(total.saturating_sub(1));
                        let mut start = 0;
                        if total > visible {
                            if cursor >= visible {
                                start = cursor + 1 - visible;
                            }

                            if start + visible > total {
                                start = total - visible;
                            }
                        }
                        let end = (start + visible).min(total);
                        for (filtered_idx, (_, m)) in filtered[start..end].iter().enumerate() {
                            let actual_idx = start + filtered_idx;
                            let is_selected = actual_idx == cursor;
                            let style = if is_selected { Theme::selection() } else { Style::default() };
                            let prefix = if **m == self.model { "● " } else { "  " };
                            let star = if is_selected { "▶ " } else { "  " };
                            let provider = if m.starts_with("opencode/") {
                                "Gateway"
                            } else if m.starts_with("opencode-go/") {
                                "Gateway-Go"
                            } else if m.contains("gemini") || m.starts_with("google/") {
                                "Gemini"
                            } else if m.contains("Muse") || m.contains("anthropic") {
                                "Anthropic"
                            } else {
                                "OpenRouter"
                            };

                            let is_free = vioraharness_core::provider::opencode::is_free_model(m);
                            let needs_key = (m.starts_with("opencode/") || m.starts_with("opencode-go/")) && !is_free && !has_gateway;
                            let mut disp = format!("{prefix}{m}");
                            if needs_key {
                                disp = format!("{disp} (needs OPENCODE_API_KEY)");
                            }
                            let mut spans = vec![
                                Span::raw(star),
                                Span::styled(disp, if needs_key && !is_selected { Style::default().fg(Color::DarkGray) } else { style }),
                                Span::raw("  "),
                                Span::styled(format!("({provider})"), Style::default().fg(Color::DarkGray)),
                            ];

                            if is_free {
                                spans.push(Span::styled(" free", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)));
                            }
                            lines.push(Line::from(spans));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(
                                format!(" — {}/{} models (↑/↓ PgUp/PgDn scroll, showing {}-{})", end, total, start + 1, end),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(" Type to search • ↑/↓ • PgUp/PgDn page • Enter select • Esc clear/close • Backspace delete", Style::default().fg(Color::DarkGray))));
                lines
            }
            Popup::Providers => {
                let catalog = provider_catalog();
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(" Providers — select then paste API key", Style::default().add_modifier(Modifier::BOLD))));
                lines.push(Line::from(Span::styled(format!(" Config: {} (0600) + env var for current process — no restart needed after save", providers_env_path().display()), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                lines.push(Line::from(""));
                if self.provider_input_active {
                    if let Some(pid) = &self.provider_selected {
                        let prov = catalog.iter().find(|(id,_,_,_)| *id == pid.as_str());
                        let env_name = prov.map(|(_,_,env,_)| *env).unwrap_or("API_KEY");
                        let masked = "•".repeat(self.provider_key_input.len().min(24));
                        let shown = if self.provider_key_input.is_empty() { "paste key…".to_string() } else { masked };
                        let style = if self.provider_key_input.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Yellow) };
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {env_name}: "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                            Span::styled(shown, style),
                            Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                        ]));
                        lines.push(Line::from(Span::styled(" Enter save • Esc cancel • Ctrl-V paste (masked) ", Style::default().fg(Color::DarkGray))));
                        if self.provider_validating {
                            lines.push(Line::from(Span::styled(" Validating…", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC))));
                        }
                        if let Some((msg, ok)) = &self.provider_msg {
                            let col = if *ok { Color::Green } else { Color::Red };
                            let icon = if *ok { "✓" } else { "✗" };
                            lines.push(Line::from(Span::styled(format!(" {icon} {msg}"), Style::default().fg(col).add_modifier(Modifier::BOLD))));
                        }
                    }
                } else {
                    for (i, (_pid, name, env_name, desc)) in catalog.iter().enumerate() {
                        let is_selected = i == self.provider_cursor;
                        let style = if is_selected { Theme::selection() } else { Style::default() };
                        let star = if is_selected { "▶ " } else { "  " };
                        let has = has_key(env_name);
                        let dot = if has { "●" } else { "○" };
                        let dot_col = if has { Color::Green } else { Color::DarkGray };
                        let preview = if has {
                            let v = std::env::var(env_name).unwrap_or_default();
                            let end = v.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
                            format!("set …{end}")
                        } else { "missing".into() };
                        lines.push(Line::from(vec![
                            Span::raw(star),
                            Span::styled(format!("{dot} "), Style::default().fg(dot_col).add_modifier(Modifier::BOLD)),
                            Span::styled(format!("{name} "), style),
                            Span::styled(format!("({env_name}) "), Style::default().fg(Color::DarkGray)),
                            Span::styled(preview, Style::default().fg(if has { Color::Green } else { Color::Yellow })),
                            Span::raw("  "),
                            Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(" ↑/↓ select • Enter paste key • Esc close • Validates against the provider then writes to .env + PATH ", Style::default().fg(Color::DarkGray))));
                    if let Some((msg, ok)) = &self.provider_msg {
                        let col = if *ok { Color::Green } else { Color::Red };
                        let icon = if *ok { "✓" } else { "✗" };
                        lines.push(Line::from(Span::styled(format!(" {icon} {msg}"), Style::default().fg(col))));
                    }
                }
                lines
            }
            Popup::ThemePicker => {
                let current = crate::theme::ThemeConfig::load().theme;
                self.available_themes.iter().enumerate().map(|(i, name)| {
                    let style = if i == self.theme_cursor { Theme::selection() } else { Style::default() };
                    let prefix = if *name == current { "● " } else { "  " };
                    let star = if i == self.theme_cursor { "▶ " } else { "  " };
                    let desc = match name.as_str() {
                        "tokyonight" => "vivid blue — Tokyo Night",
                        "catppuccin" => "warm pastel — Catppuccin Mocha",
                        "dracula" => "vivid purple — Dracula",
                        "gruvbox" => "retro warm — Gruvbox",
                        "nord" => "arctic blue — Nord",
                        "system" => "adapts to terminal palette",
                        _ => "",
                    };
                    Line::from(vec![
                        Span::raw(star),
                        Span::styled(format!("{prefix}{name}"), style),
                        Span::raw("  "),
                        Span::styled(desc, Style::default().fg(Color::DarkGray)),
                    ])
                }).collect::<Vec<_>>()
            },
            Popup::Permissions => vec![
                Line::from(Span::styled("Permissions active (vioraharness.json)", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  read, glob, grep, schematic_query, symbol_search → allow"),
                Line::from("  netlist_run, task, schematic_render → allow"),
                Line::from("  write, bash → ask (auto-allow ls/pwd in headless, /tmp write)"),
                Line::from("  bash rm:* / sudo:* → deny"),
                Line::from(""),
                Line::from(Span::styled(format!("Thinking: {} ({} / {})", if self.show_thinking { "on" } else { "off" }, self.thinking_title, self.thinking_label), Style::default().fg(Color::Yellow))),
                Line::from("Press any key to close. Edit vioraharness.json to change."),
            ],
            Popup::Diff => {
                if let Some(diff) = &self.last_diff {
                    let mut lines = vec![Line::from(Span::styled(format!("Last write diff — {}", crate::widgets::diff::diff_summary(diff)), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))];
                    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(Color::DarkGray))));
                    lines.extend(crate::widgets::diff::diff_lines(diff));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled("Press Esc to close • /undo to restore", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    lines
                } else {
                    vec![
                        Line::from(Span::styled("No diff yet", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                        Line::from(""),
                        Line::from(Span::styled("Write a file first (write tool or agent turn), then /diff will show unified diff.", Style::default().fg(Color::DarkGray))),
                        Line::from(""),
                        Line::from(Span::styled("Tip: /undo restores last snapshot", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
                    ]
                }
            }
            Popup::PermissionAsk => {
                if let Some(req) = &self.pending_perm {
                    let mut lines = Vec::new();
                    lines.push(Line::from(Span::styled(
                        " Agent wants to run a tool that requires approval ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled(" Tool: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(req.tool.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    ]));
                    let args_pretty = if req.args.len() > 200 {
                        format!("{}…", &req.args[..200])
                    } else {
                        req.args.clone()
                    };

                    let args_display = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&args_pretty) {
                        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                            cmd.to_string()
                        } else if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                            format!("path={}", path)
                        } else {
                            args_pretty
                        }
                    } else {
                        args_pretty
                    };
                    let args_short = if args_display.len() > 80 { format!("{}…", &args_display[..80]) } else { args_display };
                    lines.push(Line::from(vec![
                        Span::styled(" Args: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(args_short, Style::default().fg(Color::White)),
                    ]));

                    let pat = if req.tool == "bash" {
                        let cmd = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&req.args) {
                            v.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string()
                        } else { String::new() };
                        let first = cmd.split_whitespace().next().unwrap_or("bash").split('/').next_back().unwrap_or("bash");
                        format!("bash {first}*")
                    } else { req.tool.clone() };
                    lines.push(Line::from(vec![
                        Span::styled(" Save as: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("\"{pat}\": \"allow\""), Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)),
                        Span::styled(" in vioraharness.json", Style::default().fg(Color::DarkGray)),
                    ]));
                    lines.push(Line::from(""));
                    let opts = ["Allow once", "Allow always (save)", "Deny"];
                    for (i, opt) in opts.iter().enumerate() {
                        let is_sel = i == self.perm_cursor;
                        let style = if is_sel { crate::theme::Theme::selection() } else { Style::default() };
                        let prefix = if is_sel { "▶ " } else { "  " };
                        let key_hint = match i { 0 => " (a)", 1 => " (A)", 2 => " (d)", _ => "" };
                        lines.push(Line::from(vec![
                            Span::raw(prefix),
                            Span::styled(format!("{opt}{key_hint}"), style),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " ↑/↓ select • Enter confirm • a=allow once • A=allow always • d/n/Esc deny ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines
                } else {
                    vec![Line::from(Span::styled("No pending permission", Style::default().fg(Color::DarkGray)))]
                }
            }
            Popup::Question => {
                if let Some(req) = &self.pending_q {
                    if req.questions.is_empty() {
                        vec![Line::from(Span::styled(
                            "Invalid question (no questions)",
                            Style::default().fg(Color::Red),
                        ))]
                    } else {
                    let mut lines = Vec::new();
                    lines.push(Line::from(Span::styled(
                        " Agent asks — answer to continue ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                    let qi = self.q_answered.len().min(req.questions.len().saturating_sub(1));
                    let q = &req.questions[qi];
                    if !q.header.trim().is_empty() {
                        lines.push(Line::from(Span::styled(
                            q.header.clone(),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("Q{}/{}: ", qi + 1, req.questions.len()),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            q.question.clone(),
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    if q.multi_select {
                        lines.push(Line::from(Span::styled(
                            " (multi-select: Space toggles, Enter confirms)",
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                    lines.push(Line::from(""));

                    for (i, opt) in q.options.iter().enumerate() {
                        let is_sel = i == self.q_cursor && !self.q_custom_active;
                        let style = if is_sel { crate::theme::Theme::selection() } else { Style::default() };
                        let prefix = if is_sel { "▶ " } else { "  " };
                        let mark = if q.multi_select {
                            if self.q_toggled.contains(&i) { "[x] " } else { "[ ] " }
                        } else if is_sel {
                            "○ "
                        } else {
                            "  "
                        };
                        let mut row = vec![
                            Span::raw(prefix),
                            Span::styled(mark, Style::default().fg(Color::Cyan)),
                            Span::styled(opt.label.clone(), style),
                        ];
                        if !opt.description.trim().is_empty() {
                            row.push(Span::styled(
                                format!(" — {}", opt.description.trim()),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        lines.push(Line::from(row));
                    }
                    let custom_idx = q.options.len();
                    let is_custom = self.q_cursor == custom_idx;
                    let cstyle = if is_custom && !self.q_custom_active {
                        crate::theme::Theme::selection()
                    } else {
                        Style::default()
                    };
                    let shown = if self.q_custom.is_empty() && !self.q_custom_active {
                        "Custom answer…".to_string()
                    } else {
                        format!("{}{}", self.q_custom, if self.q_custom_active { "▌" } else { "" })
                    };
                    lines.push(Line::from(vec![
                        Span::raw(if is_custom { "▶ " } else { "  " }),
                        Span::styled("✎ ", Style::default().fg(Color::Cyan)),
                        Span::styled(
                            shown,
                            if self.q_custom.is_empty() && !self.q_custom_active {
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)
                            } else {
                                cstyle
                            },
                        ),
                    ]));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        if self.q_custom_active {
                            " type answer • Enter submit • Esc back • Backspace delete "
                        } else {
                            " ↑/↓ move • Space/Enter select • type on Custom • Esc cancel "
                        },
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines
                    }
                } else {
                    vec![Line::from(Span::styled("No pending question", Style::default().fg(Color::DarkGray)))]
                }
            }
            Popup::ToolOutput => {
                if let Some((id, name, out, ok)) = &self.last_tool_output {
                    let mut lines = Vec::new();
                    let status = if *ok { "✓ ok" } else { "✗ error" };
                    let status_col = if *ok { Color::Green } else { Color::Red };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {name} "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{status} "), Style::default().fg(status_col).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("id={} ", &id[..8.min(id.len())]), Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{} chars, {} lines", out.len(), out.lines().count()), Style::default().fg(Color::DarkGray)),
                    ]));
                    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(Color::DarkGray))));
                    if out.is_empty() {
                        lines.push(Line::from(Span::styled("(empty output)", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    } else {
                        let all: Vec<&str> = out.lines().collect();
                        let total = all.len();


                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(4).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(5).max(3);
                        }
                        let start = self.tool_output_scroll.min(total.saturating_sub(1));
                        let end = (start + visible).min(total);
                        for (i, line) in all[start..end].iter().enumerate() {
                            let line_no = start + i + 1;

                            let display = if line.len() > 100 { format!("{}…", &line[..100]) } else { (*line).to_string() };
                            lines.push(Line::from(vec![
                                Span::styled(format!("{:4} │ ", line_no), Style::default().fg(Color::DarkGray)),
                                Span::raw(display),
                            ]));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(
                                format!(" — {}/{} lines (↑/↓ PageUp/PageDown scroll, Esc close)", end, total),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(" ↑/↓ • PgUp/PgDn scroll • Esc/q close • s save to /tmp ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    lines
                } else {
                    vec![Line::from(Span::styled("No tool output yet", Style::default().fg(Color::Yellow)))]
                }
            }
            Popup::Tasks => {
                use vioraharness_core::tools::tasks::{list_tasks, BgStatus};
                let all = list_tasks();
                let mut lines: Vec<Line> = Vec::new();
                if all.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No background tasks yet.",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " Esc close ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                } else {
                let total = all.len();
                let now =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                let running = all.iter().filter(|t| t.status == BgStatus::Running).count();
                let failed = all
                    .iter()
                    .filter(|t| {
                        t.status == BgStatus::Error || t.status == BgStatus::Killed
                    })
                    .count();
                let done = total.saturating_sub(running).saturating_sub(failed);

                let mut head: Vec<Span> = vec![Span::styled(
                    format!(" {running} running "),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )];
                if done > 0 {
                    head.push(Span::styled(
                        format!("• {done} done "),
                        Style::default().fg(Color::Green),
                    ));
                }
                if failed > 0 {
                    head.push(Span::styled(
                        format!("• {failed} failed "),
                        Style::default().fg(Color::Red),
                    ));
                }
                head.push(Span::styled(
                    format!("• {total} tracked"),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::from(head));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<10}", "ID"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<9}", "STATUS"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "COMMAND",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                ]));
                let rule = "─".repeat((inner.width as usize).saturating_sub(2).max(10));
                lines.push(Line::from(Span::styled(
                    format!(" {rule}"),
                    Style::default().fg(Color::DarkGray),
                )));
                let inner_h = inner.height as usize;
                let mut visible = inner_h.saturating_sub(5).max(3);
                if total > visible {
                    visible = inner_h.saturating_sub(6).max(3);
                }
                let cursor = self.task_cursor.min(total.saturating_sub(1));
                let mut start = 0;
                if total > visible {
                    if cursor >= visible {
                        start = cursor + 1 - visible;
                    }
                    if start + visible > total {
                        start = total - visible;
                    }
                }
                let end = (start + visible).min(total);
                for (idx, t) in all[start..end].iter().enumerate() {
                    let i = start + idx;
                    let is_selected = i == cursor;
                    let style = if is_selected {
                        Theme::selection()
                    } else {
                        Style::default()
                    };
                    let (icon, col) = match t.status {
                        BgStatus::Running => ("◐", Color::Yellow),
                        BgStatus::Done => ("●", Color::Green),
                        BgStatus::Error => ("✖", Color::Red),
                        BgStatus::Killed => ("○", Color::DarkGray),
                    };
                    let status_word = match t.status {
                        BgStatus::Running => "running",
                        BgStatus::Done => "done",
                        BgStatus::Error => "error",
                        BgStatus::Killed => "killed",
                    };
                    let star = if is_selected { "▶ " } else { "  " };
                    let age = now.saturating_sub(t.started_at);
                    let age_s = if age < 60 {
                        format!("{}s", age)
                    } else if age < 3600 {
                        format!("{}m", age / 60)
                    } else {
                        format!("{}h", age / 3600)
                    };
                    let (exit_s, exit_col) = match (t.status, t.exit_code) {
                        (BgStatus::Done, _) => ("exit 0".to_string(), Color::Green),
                        (_, Some(c)) => (format!("exit {c}"), Color::Red),
                        _ => ("—".to_string(), Color::DarkGray),
                    };



                    let cmd_cap = (inner.width as usize).saturating_sub(42).max(12);
                    let cmd_raw =
                        t.command.split_whitespace().collect::<Vec<_>>().join(" ");
                    let cmd = if cmd_raw.chars().count() > cmd_cap {
                        format!(
                            "{}…",
                            cmd_raw.chars().take(cmd_cap.saturating_sub(1)).collect::<String>()
                        )
                    } else {
                        cmd_raw
                    };
                    lines.push(Line::from(vec![
                        Span::raw(star),
                        Span::styled(
                            format!("{icon} {} ", short_task_id(&t.id)),
                            Style::default().fg(col).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{status_word:<9}"),
                            Style::default().fg(col),
                        ),
                        Span::styled(
                            format!("{cmd:<cmd_cap$}"),
                            if is_selected {
                                style
                            } else {
                                Style::default().fg(Color::White)
                            },
                        ),
                        Span::styled(format!(" {exit_s:<8}"), Style::default().fg(exit_col)),
                        Span::styled(age_s, Style::default().fg(Color::DarkGray)),
                    ]));
                }
                if total > visible {
                    lines.push(Line::from(Span::styled(
                        format!(" — {}/{} tasks (showing {}-{})", end, total, start + 1, end),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " ↑/↓ navigate • Enter log • k kill • Esc close ",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                }
                lines
            }
            Popup::None => vec![],
        };
        let para = Paragraph::new(content)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        frame.render_widget(para, inner);
    }

    async fn handle_key(&mut self, code: KeyCode) -> anyhow::Result<()> {
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

    fn session_filtered_len(&self) -> usize {
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

    fn model_filtered_len(&self) -> usize {
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

    fn chat_inner(&self) -> Option<Rect> {
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

    fn screen_to_content(&self, mx: u16, my: u16) -> Option<SelPos> {
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

    fn in_input(&self, mx: u16, my: u16) -> bool {
        hit_rect(self.input_area, mx, my)
    }

    fn handle_mouse(&mut self, m: MouseEvent) {
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

    fn popup_wheel(&mut self, dir: i32) {
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

    fn handle_popup_key(&mut self, key: KeyEvent) {
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
                    Self::save_tui_state(serde_json::json!({"last_theme": name}));
                    self.messages.push(Msg::new(
                        "system",
                        format!("Theme → {} (saved, restart TUI to apply palette)", name),
                    ));
                    self.popup = Popup::None;
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
                    let is_cmd =
                        matches!(c, 'n' | 'N' | 'f' | 'F' | 'a' | 'A' | 'd' | 'D' | 'r' | 'R')
                            && self.session_filter.is_empty();
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
                                            .list_sessions_filtered(None, None, true, 50, 0)
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
                                        .list_sessions_filtered(None, None, true, 50, 0)
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
                                        .list_sessions_filtered(None, None, true, 50, 0)
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
                            .list_sessions_filtered(None, None, true, 50, 0)
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

                            match store.get_messages_detailed(&id) {
                                Ok(msgs) => {
                                    self.session_id = id.clone();
                                    self.model = model.clone();
                                    if let Some(th) = theme {
                                        Self::save_tui_state(serde_json::json!({"last_theme": th}));
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
                            if let Ok(sessions) =
                                store.list_sessions_filtered(None, None, false, 50, 0)
                            {
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
                        if let Ok(sessions) = store.list_sessions_filtered(None, None, false, 50, 0)
                        {
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
                        if let Ok(sessions) = store.list_sessions_filtered(None, None, false, 50, 0)
                        {
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

    fn resolve_perm(&mut self, decision: vioraharness_core::permissions::InteractiveDecision) {
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

    fn current_question(&self) -> Option<vioraharness_core::permissions::QuestionItem> {
        let req = self.pending_q.as_ref()?;
        if req.questions.is_empty() {
            return None;
        }
        let qi = self.q_answered.len().min(req.questions.len() - 1);
        Some(req.questions[qi].clone())
    }

    fn push_answer(&mut self, answer: vioraharness_core::permissions::QuestionAnswer) {
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

    fn answer_current(&mut self, confirm: bool) {
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

    fn submit_custom(&mut self) {
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

    fn resolve_question(&mut self, result: vioraharness_core::permissions::QuestionResult) {
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

    fn submit_text(&mut self, prompt: String) -> anyhow::Result<()> {
        if self.model.trim().is_empty() {
            self.messages.push(Msg::new(
                "system",
                "no model selected — pick one with /model (or /providers to add keys first)",
            ));
            self.model_filter.clear();
            self.model_cursor = 0;
            self.popup = Popup::ModelPicker;
            self.input.text.clear();
            self.input.cursor = 0;
            return Ok(());
        }

        let history_text = prompt.clone();
        let mut send = expand_paste_chips(&prompt, &self.pending_texts);
        self.pending_texts.clear();

        let mut pending_img = self.pending_image.take();
        if let Some(img) = &pending_img {
            let chip = image_chip(&img.label);
            if send.contains(&chip) {
                send = send.replace(&chip, &format!("[image {} attached]", img.label));
            } else {
                pending_img = None;
            }
        }
        self.messages.push(Msg::new("user", send.clone()));
        self.input.push_history(history_text);
        self.input.text.clear();
        self.input.cursor = 0;
        self.start_turn(send, pending_img.map(|img| (img.mime.to_string(), img.b64)));
        Ok(())
    }

    fn start_turn(&mut self, send: String, image: Option<(String, String)>) {
        self.scroll = 0;
        self.selection = None;
        self.dragging = false;
        self.busy = true;
        self.status = if image.is_some() {
            "thinking… (with image)".into()
        } else {
            "thinking…".into()
        };
        self.streaming_buf.clear();
        self.thinking_buf.clear();
        let model = self.model.clone();
        let session_id = self.session_id.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        self.stream_rx = Some(rx);
        let handle = tokio::spawn(async move {
            std::env::set_var("VIORAHARNESS_TUI", "1");
            let loop_ = vioraharness_core::loop_mod::AgentLoop::new();
            let res = if let Some((mime, b64)) = image {
                loop_
                    .run_streaming_with_image(
                        &send,
                        &model,
                        Some(session_id),
                        tx,
                        Some((mime, b64)),
                    )
                    .await
            } else {
                loop_
                    .run_streaming(&send, &model, Some(session_id), tx)
                    .await
            };
            std::env::remove_var("VIORAHARNESS_TUI");
            res
        });
        self.pending = Some(handle);
    }

    fn reload_display_from_store(
        &mut self,
        store: &vioraharness_core::session::SessionStore,
        sid: &str,
    ) -> Option<usize> {
        let msgs = store.get_messages_detailed(sid).ok()?;
        let tool_map = store.get_tool_calls_grouped(sid).unwrap_or_default();
        self.messages.clear();
        for m in msgs {
            if m.role == "tool" {
                continue;
            }
            let ts = m.timestamp.clone().unwrap_or_else(|| {
                format!(
                    "{:02}:{:02}",
                    (m.created_at % 86400 / 3600) % 24,
                    (m.created_at % 3600) / 60
                )
            });
            if m.role == "assistant" {
                if let Some(calls) = tool_map.get(&m.seq) {
                    let mut items = Vec::new();
                    for (id, name, args, _sig, result) in calls {
                        items.push(Content::ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            args: args.clone(),
                            status: ToolStatus::Done,
                        });
                        if let Some(res_str) = result {
                            let ok = serde_json::from_str::<serde_json::Value>(res_str)
                                .ok()
                                .and_then(|v| v.get("ok").and_then(|x| x.as_bool()))
                                .unwrap_or(!res_str.contains("\"ok\":false"));
                            items.push(Content::ToolResult {
                                id: id.clone(),
                                content: res_str.clone(),
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
        Some(self.messages.len())
    }

    fn parse_compact_freed(msg: &str) -> usize {
        if let Some(rest) = msg.strip_prefix("Compacted context:") {
            if let Some(i) = rest.find("freed ~") {
                let num: String = rest[i + 7..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(k) = num.parse::<usize>() {
                    return k * 1000;
                }
            }
        }
        0
    }

    fn poll_task_completions(&mut self) -> Vec<String> {
        use vioraharness_core::tools::tasks::BgStatus;
        let mut waked = Vec::new();
        for done in vioraharness_core::tools::tasks::take_completions() {
            let (mark, detail) = match done.status {
                BgStatus::Done => ("✔", "exit 0".to_string()),
                BgStatus::Error => (
                    "✖",
                    done.exit_code
                        .map(|c| format!("exit {c}"))
                        .unwrap_or_else(|| "failed".to_string()),
                ),
                BgStatus::Killed => ("○", "killed".to_string()),
                BgStatus::Running => continue,
            };
            self.messages.push(Msg::new(
                "system",
                format!(
                    "{mark} task {} {} ({detail}) — /tasks to view log",
                    short_task_id(&done.id),
                    done.status.as_str()
                ),
            ));
            self.note_task_completion(&done);
            if !self.wake_on_tasks || self.model.trim().is_empty() {
                continue;
            }
            if !self.busy && self.pending.is_none() {
                self.wake_for_task(&done);
                waked.push(done.id.clone());
            } else {
                // Already waking/running: queue the follow-up behind the
                // live turn instead of dropping it to a bare notice.
                self.queued_prompts.push(QueuedPrompt {
                    session_id: self.session_id.clone(),
                    send: Self::task_wake_prompt(&done),
                    image: None,
                });
                self.status = format!(
                    "task {} finished — follow-up queued ({})",
                    short_task_id(&done.id),
                    self.queued_prompts.len()
                );
                waked.push(done.id.clone());
            }
        }
        waked
    }

    /// Follow-up prompt for a finished background task (log tail included).
    /// Pure constructor shared by immediate wakes and queued follow-ups.
    fn task_wake_prompt(done: &vioraharness_core::tools::tasks::BgTask) -> String {
        use vioraharness_core::tools::tasks::BgStatus;
        let tail = vioraharness_core::tools::tasks::read_task_log(&done.id).unwrap_or_default();
        let tail: String = tail
            .chars()
            .rev()
            .take(1500)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let outcome = match done.status {
            BgStatus::Done => "done (exit 0)".to_string(),
            BgStatus::Error => done
                .exit_code
                .map(|c| format!("failed (exit {c})"))
                .unwrap_or_else(|| "failed".to_string()),
            BgStatus::Killed => "killed".to_string(),
            BgStatus::Running => return String::new(),
        };
        format!(
            "[background task finished] {} (`{}`) {outcome}.\nLog tail:\n{tail}\nContinue from where you left off; do not restart the finished command. If the work is complete, summarize briefly.",
            short_task_id(&done.id),
            done.command,
        )
    }

    fn note_task_completion(&mut self, done: &vioraharness_core::tools::tasks::BgTask) {
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
            let _ = store.append_message(
                &self.session_id,
                "system",
                &format!(
                    "background task {} (`{}`) {} — log: {}",
                    short_task_id(&done.id),
                    done.command.chars().take(120).collect::<String>(),
                    done.status.as_str(),
                    done.log_path,
                ),
            );
        }
    }

    fn wake_for_task(&mut self, done: &vioraharness_core::tools::tasks::BgTask) {
        use vioraharness_core::tools::tasks::BgStatus;
        if done.status == BgStatus::Running {
            return;
        }
        self.status = format!("working on finished task {}…", short_task_id(&done.id));
        self.start_turn(Self::task_wake_prompt(done), None);
    }

    fn is_busy_safe_slash(text: &str) -> bool {
        matches!(
            text.split_whitespace().next().unwrap_or(""),
            "/help"
                | "/h"
                | "/sessions"
                | "/chats"
                | "/history"
                | "/conversations"
                | "/ls"
                | "/convs"
                | "/providers"
                | "/provider"
                | "/auth"
                | "/keys"
                | "/model"
                | "/skills"
                | "/skill"
                | "/theme"
                | "/thinking"
                | "/permissions"
                | "/perms"
                | "/verbosity"
                | "/verbose"
                | "/cards"
                | "/diff"
                | "/output"
                | "/view"
                | "/tool"
                | "/out"
                | "/tasks"
                | "/task"
                | "/bg"
                | "/jobs"
                | "/export"
                | "/compact"
                | "/quit"
                | "/q"
                | "/exit"
        )
    }

    fn queue_prompt(&mut self, prompt: String) {
        let history_text = prompt.clone();
        let mut send = expand_paste_chips(&prompt, &self.pending_texts);
        self.pending_texts.clear();
        let mut pending_img = self.pending_image.take();
        if let Some(img) = &pending_img {
            let chip = image_chip(&img.label);
            if send.contains(&chip) {
                send = send.replace(&chip, &format!("[image {} attached]", img.label));
            } else {
                pending_img = None;
            }
        }
        self.messages.push(Msg::new("user", send.clone()));
        self.input.push_history(history_text);
        self.input.text.clear();
        self.input.cursor = 0;
        self.queued_prompts.push(QueuedPrompt {
            session_id: self.session_id.clone(),
            send,
            image: pending_img.map(|img| (img.mime.to_string(), img.b64)),
        });
        let n = self.queued_prompts.len();
        self.status = if n == 1 {
            "queued — sends when the turn finishes".into()
        } else {
            format!("queued ({n} waiting) — send in order when idle")
        };
    }

    fn drain_queue(&mut self) {
        if self.busy || self.pending.is_some() || self.model.trim().is_empty() {
            return;
        }
        while let Some(head) = self.queued_prompts.first() {
            if head.session_id != self.session_id {
                self.queued_prompts.remove(0);
                self.messages.push(Msg::new(
                    "system",
                    "dropped queued prompt (session changed)",
                ));
                continue;
            }
            break;
        }
        if self.queued_prompts.is_empty() {
            return;
        }
        let q = self.queued_prompts.remove(0);
        if !self.queued_prompts.is_empty() {
            self.status = format!(
                "sending queued prompt ({} more waiting)…",
                self.queued_prompts.len()
            );
        }
        self.start_turn(q.send, q.image);
    }

    fn poll_compact(&mut self) {
        let Some((sid, mut rx)) = self.compact_rx.take() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(rep)) => {
                if rep.compacted {
                    if sid == self.session_id {
                        let db = std::env::var("VIORAHARNESS_DB")
                            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                        if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                            let n = self.reload_display_from_store(&store, &sid).unwrap_or(0);
                            self.ctx_freed_tokens = 0;
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "✂ {} ({n} msgs shown)",
                                    vioraharness_core::context::compaction::compact_notice(&rep)
                                ),
                            ));
                        }
                        self.scroll = 0;
                    } else {
                        self.messages.push(Msg::new(
                            "system",
                            format!("compacted {sid} (session switched — use /resume to see it)"),
                        ));
                    }
                    self.status = "ready".into();
                } else {
                    self.messages.push(Msg::new("system", rep.note));
                    self.status = "ready".into();
                }
            }
            Ok(Err(e)) => {
                self.messages
                    .push(Msg::new("system", format!("compact failed: {e}")));
                self.status = "ready".into();
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                self.compact_rx = Some((sid, rx));
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.messages.push(Msg::new(
                    "system",
                    "compact task ended without a result".to_string(),
                ));
                self.status = "ready".into();
            }
        }
    }

    fn is_known_slash(text: &str) -> bool {
        matches!(
            text.split_whitespace().next().unwrap_or(""),
            "/help"
                | "/h"
                | "/clear"
                | "/sessions"
                | "/chats"
                | "/history"
                | "/conversations"
                | "/ls"
                | "/convs"
                | "/providers"
                | "/provider"
                | "/auth"
                | "/keys"
                | "/new"
                | "/resume"
                | "/r"
                | "/open"
                | "/restore"
                | "/fork"
                | "/rename"
                | "/archive"
                | "/delete"
                | "/export"
                | "/model"
                | "/skills"
                | "/skill"
                | "/skill-new"
                | "/new-skill"
                | "/skill-create"
                | "/theme"
                | "/thinking"
                | "/permissions"
                | "/perms"
                | "/tasks"
                | "/task"
                | "/bg"
                | "/jobs"
                | "/verbosity"
                | "/verbose"
                | "/cards"
                | "/diff"
                | "/output"
                | "/view"
                | "/tool"
                | "/out"
                | "/undo"
                | "/compact"
                | "/quit"
                | "/q"
                | "/exit"
        )
    }

    fn handle_slash(&mut self, cmd: &str) {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        match parts.first().copied().unwrap_or("") {
            "/help" | "/h" => self.popup = Popup::Help,
            "/clear" => {
                self.messages.clear();

                let dropped = self.queued_prompts.len();
                self.queued_prompts.clear();
                self.messages.push(Msg::new(
                    "system",
                    if dropped > 0 {
                        format!(
                            "cleared (dropped {dropped} queued prompt{})",
                            if dropped == 1 { "" } else { "s" }
                        )
                    } else {
                        "cleared".to_string()
                    },
                ));
                self.scroll = 0;
            }
            "/sessions" | "/chats" | "/history" | "/conversations" | "/ls" | "/convs" => {
                self.session_filter.clear();
                self.session_cursor = 0;
                self.popup = Popup::Sessions;
            }
            "/providers" | "/provider" | "/auth" | "/keys" => {
                self.provider_cursor = 0;
                self.provider_key_input.clear();
                self.provider_input_active = false;
                self.provider_selected = None;
                self.provider_msg = None;
                self.popup = Popup::Providers;
            }
            "/model" => {
                if parts.len() > 1 {
                    let m = parts[1..].join(" ");
                    self.model = m.clone();
                    Self::save_tui_state(serde_json::json!({"last_model": self.model}));
                    self.messages
                        .push(Msg::new("system", format!("model → {m} (saved)")));
                } else {
                    self.model_filter.clear();
                    self.model_cursor = 0;
                    self.popup = Popup::ModelPicker;
                }
            }
            "/skills" | "/skill" => {
                self.popup = Popup::Skills;
            }
            "/skill-new" | "/new-skill" | "/skill-create" => {
                if self.busy {
                    self.messages.push(Msg::new(
                        "system",
                        "busy — wait for the current turn or Esc to cancel",
                    ));
                    return;
                }
                let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                match skill_new_request(&parts[1..], &cwd) {
                    Ok((description, target)) => {
                        let prompt = skill_creator_prompt(&description, &target);
                        self.input
                            .push_history(format!("/skill-new {}", parts[1..].join(" ")));
                        self.input.text.clear();
                        self.input.cursor = 0;
                        if let Err(e) = self.submit_text(prompt) {
                            self.messages.push(Msg::new(
                                "system",
                                format!("skill creator failed to start: {e:#}"),
                            ));
                        }
                    }
                    Err(usage) => {
                        self.messages.push(Msg::new("system", usage));
                    }
                }
            }
            "/theme" => {
                if parts.len() > 1 {
                    let name = parts[1].to_lowercase();

                    if self.available_themes.iter().any(|t| t == &name) {
                        Self::save_tui_state(serde_json::json!({"last_theme": name}));
                        self.messages.push(Msg::new(
                            "system",
                            format!("Theme → {} (saved, restart TUI to apply palette)", name),
                        ));
                    } else {
                        self.popup = Popup::ThemePicker;
                    }
                } else {
                    self.popup = Popup::ThemePicker;
                }
            }
            "/thinking" => {
                if parts.len() > 1 {
                    match parts[1].to_lowercase().as_str() {
                        "on" | "true" | "show" => {
                            self.show_thinking = true;
                            self.thinking_expanded = true;
                            Self::save_tui_state(serde_json::json!({"show_thinking": true}));
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Thinking ON ({} / {}) — saved",
                                    self.thinking_title, self.thinking_label
                                ),
                            ));
                        }
                        "off" | "false" | "hide" => {
                            self.show_thinking = false;
                            Self::save_tui_state(serde_json::json!({"show_thinking": false}));
                            self.messages
                                .push(Msg::new("system", "Thinking OFF — saved"));
                        }
                        _ => {
                            self.show_thinking = !self.show_thinking;
                            Self::save_tui_state(
                                serde_json::json!({"show_thinking": self.show_thinking}),
                            );
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Thinking {} — saved",
                                    if self.show_thinking { "ON" } else { "OFF" }
                                ),
                            ));
                        }
                    }
                } else {
                    self.show_thinking = !self.show_thinking;
                    self.thinking_expanded = self.show_thinking;
                    Self::save_tui_state(serde_json::json!({"show_thinking": self.show_thinking}));
                    self.messages.push(Msg::new(
                        "system",
                        format!(
                            "Thinking {} ({} / {}) — Ctrl+G to toggle expand — saved",
                            if self.show_thinking { "ON" } else { "OFF" },
                            self.thinking_title,
                            self.thinking_label
                        ),
                    ));
                }
            }
            "/permissions" | "/perms" => self.popup = Popup::Permissions,
            "/tasks" | "/task" | "/bg" | "/jobs" => {
                self.task_cursor = 0;
                self.popup = Popup::Tasks;
            }
            "/verbosity" | "/verbose" | "/cards" => {
                self.cmd_verbosity(&parts[1..]);
            }
            "/diff" => {
                self.popup = Popup::Diff;
            }
            "/output" | "/view" | "/tool" | "/out" => {
                if self.last_tool_output.is_some() {
                    self.popup = Popup::ToolOutput;
                    self.tool_output_scroll = 0;
                } else {
                    self.messages.push(Msg::new("system", "No tool output yet — run a bash/write/glob first, then /output or F9 to view"));
                }
            }
            "/undo" => {
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                let msg: String = (|| -> Result<String, String> {
                    let store = vioraharness_core::session::SessionStore::new(&db)
                        .map_err(|e| format!("undo: cannot open session db: {e}"))?;

                    match vioraharness_core::session::snapshot::restore_latest(
                        &store,
                        &self.session_id,
                    ) {
                        Ok(Some(path)) => Ok(format!(
                            "undo: restored {path} — repeat /undo to go further back"
                        )),
                        Ok(None) => Ok("undo: nothing to undo in this chat".to_string()),
                        Err(e) => Err(format!("undo failed: {e}")),
                    }
                })()
                .unwrap_or_else(|e| e);
                self.messages.push(Msg::new("system", msg));
            }
            "/compact" => {
                if self.busy {
                    self.messages.push(Msg::new(
                        "system",
                        "busy — /compact after the current turn finishes",
                    ));
                } else if self.compact_rx.is_some() {
                    self.messages
                        .push(Msg::new("system", "compaction already running…"));
                } else {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    let sid = self.session_id.clone();
                    let model = self.model.clone();
                    self.status = "Compacting context… (one summary call)".into();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    self.compact_rx = Some((sid.clone(), rx));
                    tokio::spawn(async move {
                        let out = async {
                            let store = vioraharness_core::session::SessionStore::new(&db)
                                .map_err(|e| format!("db: {e:#}"))?;
                            let provider = vioraharness_core::provider_for_model(&model);
                            let keep = vioraharness_core::context::compaction::compaction_config()
                                .keep_tail;
                            vioraharness_core::context::compaction::compact_session(
                                &store,
                                provider.as_ref(),
                                &sid,
                                &model,
                                keep,
                            )
                            .await
                            .map_err(|e| format!("{e:#}"))
                        }
                        .await;
                        let _ = tx.send(out);
                    });
                }
            }
            "/quit" | "/q" | "/exit" => self.should_quit = true,
            "/new" => {
                let new_id = new_session_id();
                self.session_id = new_id.clone();
                self.messages.clear();
                self.ctx_freed_tokens = 0;
                self.messages.push(Msg::new(
                    "system",
                    format!(
                        "New chat {new_id} — old chats kept in SQLite, use /sessions to resume"
                    ),
                ));
                self.scroll = 0;
                self.status = "ready".into();
            }
            "/resume" | "/r" | "/open" | "/restore" => {
                if parts.len() > 1 {
                    let id = parts[1].to_string();
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        let real_id = if let Ok(Some(_)) = store.get_session(&id) {
                            id.clone()
                        } else {
                            if let Ok(list) =
                                store.list_sessions_filtered(None, Some(&id), true, 10, 0)
                            {
                                list.into_iter()
                                    .find(|s| s.id.starts_with(&id))
                                    .map(|s| s.id)
                                    .unwrap_or(id.clone())
                            } else {
                                id.clone()
                            }
                        };
                        if let Ok(Some(sess)) = store.get_session(&real_id) {
                            self.session_id = real_id.clone();
                            self.model = sess.model.clone();
                            if let Some(theme) = sess.theme.clone() {
                                Self::save_tui_state(serde_json::json!({"last_theme": theme}));
                            }
                            match self.reload_display_from_store(&store, &real_id) {
                                Some(n) => {
                                    self.ctx_freed_tokens = 0;
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!(
                                            "↩︎ Resumed {} ({} msgs)",
                                            &real_id[..8.min(real_id.len())],
                                            n
                                        ),
                                    ));
                                }
                                None => {
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!("resume: no messages for {real_id}"),
                                    ));
                                }
                            }
                        } else {
                            self.messages.push(Msg::new(
                                "system",
                                format!("resume: session {real_id} not found"),
                            ));
                        }
                    } else {
                        self.messages.push(Msg::new("system", "resume: db error"));
                    }
                } else {
                    self.session_cursor = 0;
                    self.popup = Popup::Sessions;
                }
            }
            "/fork" => {
                let at_seq = parts.get(1).and_then(|s| s.parse::<i64>().ok());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    let parent = self.session_id.clone();
                    let new_id = new_session_id();
                    match store.fork_session(&parent, &new_id, at_seq) {
                        Ok(_) => {
                            self.session_id = new_id.clone();
                            self.ctx_freed_tokens = 0;
                            if let Ok(msgs) = store.get_messages_detailed(&new_id) {
                                self.messages.clear();
                                let tool_map =
                                    store.get_tool_calls_grouped(&new_id).unwrap_or_default();
                                for m in msgs {
                                    if m.role == "tool" {
                                        continue;
                                    }
                                    let ts = m.timestamp.clone().unwrap_or_else(|| {
                                        format!(
                                            "{:02}:{:02}",
                                            (m.created_at % 86400 / 3600) % 24,
                                            (m.created_at % 3600) / 60
                                        )
                                    });
                                    if m.role == "assistant" {
                                        if let Some(calls) = tool_map.get(&m.seq) {
                                            let mut items = Vec::new();
                                            for (id, name, args, _sig, result) in calls {
                                                items.push(Content::ToolCall {
                                                    id: id.clone(),
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
                                                    .unwrap_or(!res_str.contains("\"ok\":false"));
                                                    let content = res_str.clone();
                                                    items.push(Content::ToolResult {
                                                        id: id.clone(),
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
                                        "⑂ Forked {} → {} at seq {:?}",
                                        &parent[..8.min(parent.len())],
                                        &new_id[..8.min(new_id.len())],
                                        at_seq
                                    ),
                                ));
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("fork failed: {e}")));
                        }
                    }
                }
            }
            "/rename" => {
                let title = parts[1..].join(" ");
                if title.is_empty() {
                    self.messages
                        .push(Msg::new("system", "usage: /rename <title>"));
                } else {
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                        match store.rename_session(&self.session_id, &title) {
                            Ok(_) => {
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "Renamed {} → '{}'",
                                        &self.session_id[..8.min(self.session_id.len())],
                                        title
                                    ),
                                ));
                            }
                            Err(e) => {
                                self.messages
                                    .push(Msg::new("system", format!("rename failed: {e}")));
                            }
                        }
                    }
                }
            }
            "/archive" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.archive_session(&id) {
                        Ok(_) => {
                            self.messages.push(Msg::new(
                                "system",
                                format!("Archived {}", &id[..8.min(id.len())]),
                            ));
                            if id == self.session_id {
                                let new_id = new_session_id();
                                self.session_id = new_id;
                                self.messages.clear();
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("archive failed: {e}")));
                        }
                    }
                }
            }
            "/delete" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.delete_session(&id) {
                        Ok(_) => {
                            self.messages.push(Msg::new(
                                "system",
                                format!("Deleted {} (CASCADE)", &id[..8.min(id.len())]),
                            ));
                            if id == self.session_id {
                                let new_id = new_session_id();
                                self.session_id = new_id;
                                self.messages.clear();
                            }
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("delete failed: {e}")));
                        }
                    }
                }
            }
            "/export" => {
                let id = parts
                    .get(1)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| self.session_id.clone());
                let db = std::env::var("VIORAHARNESS_DB")
                    .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                if let Ok(store) = vioraharness_core::session::SessionStore::new(&db) {
                    match store.export_jsonl(&id) {
                        Ok(jsonl) => {
                            let path = format!(
                                "/tmp/vioraharness_export_{}.jsonl",
                                &id[..8.min(id.len())]
                            );
                            let _ = std::fs::write(&path, &jsonl);
                            self.messages.push(Msg::new(
                                "system",
                                format!(
                                    "Exported {} ({} lines) → {} ({} bytes)",
                                    &id[..8.min(id.len())],
                                    jsonl.lines().count(),
                                    path,
                                    jsonl.len()
                                ),
                            ));
                        }
                        Err(e) => {
                            self.messages
                                .push(Msg::new("system", format!("export failed: {e}")));
                        }
                    }
                }
            }
            _ => {
                self.messages.push(Msg::new(
                    "system",
                    format!("unknown command: {cmd} (try /help)"),
                ));
            }
        }
    }
}

fn popup_list_visible(chrome: usize) -> usize {
    let rows = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24);
    rows.saturating_mul(60)
        .saturating_div(100)
        .saturating_sub(2)
        .saturating_sub(chrome)
        .max(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn test_app() -> App {
        let mut app = App::new("unittest/test-model".to_string());
        app.busy = false;
        app
    }

    fn render_text(app: &mut App, w: u16, h: u16) -> String {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn push_lines(app: &mut App, n: usize, start_at: usize) {
        for i in 0..n {
            app.messages
                .push(Msg::new("user", format!("line {:03}", start_at + i)));
        }
    }

    #[tokio::test]
    async fn chat_pageup_scrolls_view() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let before = render_text(&mut app, 80, 24);
        assert!(before.contains("line 059"), "tail visible at bottom");
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 10);
        let after = render_text(&mut app, 80, 24);
        assert!(!after.contains("line 059"), "pageup moved tail out of view");
        assert!(after.contains("line 049"), "pageup shows earlier lines");

        app.handle_key(KeyCode::PageDown).await.unwrap();
        assert_eq!(app.scroll, 0);
        let back = render_text(&mut app, 80, 24);
        assert!(back.contains("line 059"), "pagedown returns to tail");
    }

    #[tokio::test]
    async fn chat_holds_position_on_new_content() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let _ = render_text(&mut app, 80, 24);
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 30);
        let first = render_text(&mut app, 80, 24);

        let first_row = first.lines().nth(2).unwrap_or("").to_string();

        push_lines(&mut app, 5, 60);
        let second = render_text(&mut app, 80, 24);
        let second_row = second.lines().nth(2).unwrap_or("").to_string();
        assert_eq!(
            first_row, second_row,
            "scrolled-up view holds position on new content"
        );
        assert_eq!(app.scroll, 35, "scroll compensates for new lines");
    }

    #[test]
    fn picker_cursor_stays_visible_on_short_terminal() {
        let mut app = test_app();
        app.available_models = (0..50).map(|i| format!("openai/model-{i:02}")).collect();
        app.popup = Popup::ModelPicker;
        app.model_cursor = 40;

        let text = render_text(&mut app, 80, 20);
        assert!(
            text.contains("openai/model-40"),
            "selected model visible with height-aware viewport"
        );
        assert!(text.contains("▶"), "selection marker visible");
    }

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
    fn table_parser_detects_alignments() {
        let lines = vec![
            "| ✅ Pros | ⚠️ Cons |",
            "|---|---|",
            "| Single codebase for all platforms | Larger app size than native |",
            "| Near-native performance (AOT compiled) | Less mature ecosystem for desktop/web |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 120).expect("table parses");
        assert_eq!(consumed, 4);

        assert_eq!(rendered.len(), 6);

        let widths: Vec<usize> = rendered
            .iter()
            .map(|spans| spans.iter().map(|s| s.content.width()).sum())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all table rows same width: {widths:?}"
        );

        let header_text: String = rendered[1].iter().map(|s| s.content.as_ref()).collect();
        assert!(header_text.contains("Pros") && header_text.contains("Cons"));
        let bold_cells = rendered[1]
            .iter()
            .filter(|s| {
                !s.content.trim().is_empty() && s.style.remove_modifier(Modifier::BOLD) != s.style
            })
            .count();
        assert!(bold_cells > 0, "header cells carry BOLD");
    }

    #[test]
    fn latex_translator_symbols_and_structures() {
        assert_eq!(translate_latex(r"\alpha + \beta = \gamma"), "α + β = γ");
        assert_eq!(translate_latex(r"\sum_{i=1}^{n} x_i"), "∑_i=1^n x_i");
        assert_eq!(translate_latex(r"\frac{a}{b}"), "(a)/(b)");
        assert_eq!(translate_latex(r"\sqrt{x^2}"), "√(x^2)");
        assert_eq!(translate_latex(r"x \in \mathbb{R}"), "x ∈ ℝ");
        assert_eq!(
            translate_latex(r"\left( \frac{1}{2} \right)"),
            "( (1)/(2) )"
        );
        assert_eq!(translate_latex(r"a \leq b \neq c"), "a ≤ b ≠ c");
        assert_eq!(translate_latex(r"\text{eff} = P"), "eff = P");
        assert_eq!(translate_latex(r"\infty \to \pm 1"), "∞ → ± 1");

        assert_eq!(translate_latex(r"\$100 \& 50\%"), "$100 & 50%");
        assert_eq!(translate_latex(r"{a}_{ij}"), "a_ij");

        assert_eq!(translate_latex(r"a \qquad b \quad c"), "a      b    c");
        assert_eq!(translate_latex(r"a\!b"), "ab");
        assert_eq!(translate_latex(r"a\hspace{1cm}b"), "a b");
    }

    #[test]
    fn inline_math_strips_delimiters_and_styles() {
        let spans = markdown_inline_spans("Einstein: $E = mc^2$ ok");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains('$'), "delimiters stripped: {text}");
        assert!(text.contains("E = mc^2"), "{text}");
        let math = spans
            .iter()
            .find(|s| s.content.contains("mc^2"))
            .expect("math span present");
        assert_eq!(math.style, Theme::math());

        let spans = markdown_inline_spans(r"angle \(\theta = 90^\degree\) done");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("θ = 90^°"), "{text}");
        assert!(!text.contains('\\'), "{text}");

        let spans = markdown_inline_spans("costs $100 and $200 total");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "costs $100 and $200 total", "{text}");

        let spans = markdown_inline_spans("$a*b$");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "a*b", "{text}");
    }

    #[test]
    fn display_math_block_centers_and_consumes() {
        let lines = vec!["$$", r"\frac{1}{2} + \alpha", "$$"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("block parses");
        assert_eq!(consumed, 3);
        assert_eq!(rows.len(), 1);
        let text: String = rows[0].iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("(1)/(2) + α"), "{text:?}");

        assert!(text.starts_with(&" ".repeat(14)), "{text:?}");

        let lines = vec!["$$E = mc^2$$"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("single-line");
        assert_eq!(consumed, 1);
        let text: String = rows[0].iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("E = mc^2"), "{text:?}");

        let lines = vec![r"\[x &= 1 \\", r"y &= 2\]"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("bracket form");
        assert_eq!(consumed, 2);
        assert_eq!(rows.len(), 2, "\\\\ splits equation lines");

        assert!(render_math_block(&["$$x + y"], 40).is_none());
        assert!(render_math_block(&["plain"], 40).is_none());
    }

    #[test]
    fn clipboard_holder_survives_for_paste() {
        let probe = "vioraharness-paste-probe";
        let Ok((_, _)) = clipboard_copy_text(probe) else {
            return;
        };
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return;
        };
        if let Ok(back) = cb.get_text() {
            if back == probe {
                assert_eq!(back, probe);
            }
        }
    }

    #[test]
    fn pipe_to_clipboard_success_and_failure() {
        assert!(pipe_to_clipboard("cat", &[], "hi").is_ok());
        assert!(pipe_to_clipboard("vioraharness-no-such-prog", &[], "hi").is_err());
    }

    #[test]
    fn osc52_copy_reports_chars() {
        let (n, via) = osc52_copy_text("hi ✓").expect("osc52 writes");
        assert_eq!(n, 4);
        assert!(!via);
    }

    #[test]
    fn user_inductor_snippet_renders_clean() {
        let lines = vec![
            "\\[",
            "  I{L,\\text{peak}} = IL + \\frac{Δ I_L}{2}, \\qquad",
            "  I{L,\\text{valley}} = IL - \\frac{Δ I_L}{2}",
            "\\]",
        ];
        let (rows, consumed) = render_math_block(&lines, 60).expect("block parses");
        assert_eq!(consumed, 4);
        assert_eq!(rows.len(), 2);
        let text: String = rows
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!text.contains('\\'), "{text:?}");
        assert!(text.contains("IL,peak = IL + (Δ I_L)/(2),"), "{text:?}");
        assert!(text.contains("IL,valley = IL - (Δ I_L)/(2)"), "{text:?}");
    }

    #[test]
    fn table_merges_surplus_pipes_into_last_column() {
        let lines = vec![
            "| Param | Eq |",
            "|---|---|",
            "| Start-up | |T| > 1 |",
            "| Gain | Av |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 80).expect("table parses");
        assert_eq!(consumed, 4);
        let text: String = rendered
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("|T|") && text.contains("> 1"), "{text:?}");

        let bars: usize = rendered[3]
            .iter()
            .map(|s| s.content.matches('│').count())
            .sum();
        assert_eq!(bars, 3, "{text:?}");
    }

    #[test]
    fn bare_latex_translates_only_known_commands() {
        assert_eq!(
            translate_bare_latex_chunk(r"Av \approx -gm R_C"),
            "Av ≈ -gm R_C"
        );
        assert_eq!(translate_bare_latex_chunk(r"T = A_v \beta"), "T = A_v β");
        assert_eq!(
            translate_bare_latex_chunk(r"\frac{C1}{C1 + C2}"),
            "(C1)/(C1 + C2)"
        );
        assert_eq!(translate_bare_latex_chunk(r"x \in \mathbb{R}"), "x ∈ ℝ");

        assert_eq!(translate_bare_latex_chunk(r"C:\new\temp"), r"C:\new\temp");
        assert_eq!(translate_bare_latex_chunk(r"R_C and {a}"), "R_C and {a}");
        assert_eq!(
            translate_bare_latex_chunk(r"use \tools daily"),
            r"use \tools daily"
        );
        assert_eq!(translate_bare_latex_chunk("costs $100"), "costs $100");
        assert_eq!(translate_bare_latex_chunk(r"C:\\share"), r"C:\\share");
        assert_eq!(translate_bare_latex_chunk(r"a \qquad b"), "a      b");
        assert_eq!(translate_bare_latex_chunk(r"a\!b"), "ab");
    }

    #[test]
    fn inline_math_ignores_code_spans_and_brackets() {
        let spans = markdown_inline_spans("`$HOME` and $x$");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "$HOME and x", "{text}");
        let code = spans
            .iter()
            .find(|s| s.content.contains("$HOME"))
            .expect("code span");
        assert_eq!(code.style, Theme::inline_code());

        let spans = markdown_inline_spans(r"gain \[|A_v| = g_m R_C\] ok");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("|A_v| = g_m R_C"), "{text}");
        assert!(!text.contains("\\["), "{text}");
    }

    #[test]
    fn join_math_lines_handles_tables_and_fences() {
        let lines = vec![
            "| Loop gain | T = A_v | \\[",
            "  T = Av \\frac{C1}{C1 + C2}",
            "  \\] |",
            "| next | row | here |",
        ];
        let joined = join_math_lines(&lines);
        assert_eq!(joined.len(), 2, "{joined:?}");
        assert!(
            joined[0].contains("\\[") && joined[0].contains("\\]"),
            "{joined:?}"
        );

        let lines = vec!["```sh", "echo $$", "```", "after"];
        assert_eq!(join_math_lines(&lines).len(), 4);

        let lines = vec!["$$x$$", "plain $100"];
        assert_eq!(join_math_lines(&lines), vec!["$$x$$", "plain $100"]);

        let lines = vec!["text", "$$x + y"];
        assert_eq!(join_math_lines(&lines).len(), 2);
    }

    #[test]
    fn table_with_math_cell_renders_translated() {
        let lines = vec![
            "| Param | Value |",
            "|---|---|",
            "| Loop gain | \\[ T = Av \\frac{C1}{C1 + C2} \\] |",
            "| Bare | Av \\approx -gm R_C |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 80).expect("table parses");
        assert_eq!(consumed, 4);
        let text: String = rendered
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!text.contains("\\["), "{text:?}");
        assert!(!text.contains("\\approx"), "{text:?}");
        assert!(!text.contains("\\frac"), "{text:?}");
        assert!(text.contains("(C1)/(C1 + C2)"), "{text:?}");
        assert!(text.contains("Av ≈ -gm R_C"), "{text:?}");
    }

    #[test]
    fn table_alignments_and_escapes() {
        assert_eq!(parse_delim_cell("---"), Some(TableAlign::Left));
        assert_eq!(parse_delim_cell(":---"), Some(TableAlign::Left));
        assert_eq!(parse_delim_cell("---:"), Some(TableAlign::Right));
        assert_eq!(parse_delim_cell(":--:"), Some(TableAlign::Center));
        assert_eq!(parse_delim_cell("abc"), None);
        assert_eq!(parse_delim_cell(":|"), None);

        assert_eq!(
            split_table_row("| a \\| b | c |"),
            Some(vec!["a | b".to_string(), "c".to_string()])
        );

        assert_eq!(
            split_table_row("a | b"),
            Some(vec!["a".to_string(), "b".to_string()])
        );

        let lines = vec!["| a | b |", "| c | d |"];
        assert!(render_table_block(&lines, 120).is_none());

        let lines = vec!["| n |", "|--:|", "| 42 |"];
        let (rendered, consumed) = render_table_block(&lines, 120).expect("table parses");
        assert_eq!(consumed, 3);
        let body: String = rendered[3].iter().map(|s| s.content.as_ref()).collect();
        assert!(body.ends_with("42 │"), "right-aligned: {body:?}");
    }

    #[test]
    fn table_borders_align_on_screen() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich | Dart is less common than JS/Swift/Kotlin |\n\n```bash\necho hi\n```",
        ));
        let text = render_text(&mut app, 120, 40);
        let rows: Vec<String> = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with('┌') || t.starts_with('│') || t.starts_with('├') || t.starts_with('└')
            })
            .map(|l| l.trim_end().to_string())
            .collect();
        assert!(
            rows.len() >= 8,
            "table + fence rows present: {}",
            rows.len()
        );

        let nogutter = |l: &&String| l.chars().skip(2).collect::<String>();
        let table: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).starts_with(['┌', '│', '├', '└']) && !l.contains("echo"))
            .collect();
        let widths: Vec<usize> = table.iter().map(|l| nogutter(l).width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "on-screen table rows align: {widths:?}"
        );

        let fence: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).contains('┌') || nogutter(l).starts_with('└'))
            .collect();
        assert_eq!(fence.len(), 2);
        let fence_w: Vec<usize> = fence.iter().map(|l| nogutter(l).width()).collect();
        assert_eq!(fence_w[0], fence_w[1], "fence top/bottom align");
    }

    #[test]
    fn selection_orders_and_caret() {
        let a = SelPos { line: 5, col: 3 };
        let b = SelPos { line: 2, col: 9 };
        let sel = Selection {
            anchor: a,
            cursor: b,
            session: "s".into(),
            msg_len: 0,
        };
        assert_eq!(sel.ordered(), (b, a));
        assert!(!sel.is_caret());
        let caret = Selection {
            anchor: a,
            cursor: a,
            session: "s".into(),
            msg_len: 0,
        };
        assert!(caret.is_caret());
    }

    #[test]
    fn highlight_splits_spans_and_keeps_styles() {
        let line = Line::from(vec![
            Span::raw("hello "),
            Span::styled(
                "world".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]);
        let sel_style = Style::default().bg(Color::Yellow);
        let out = highlight_range(&line.spans, 3, 8, sel_style);
        let text: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "hello world");

        let mid: String = out
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(mid, "lo wo");

        assert!(out
            .iter()
            .any(|s| s.content.as_ref() == "wo"
                && s.style.remove_modifier(Modifier::BOLD) != s.style));
    }

    #[test]
    fn highlight_snaps_wide_chars_whole() {
        let line = Line::from(vec![Span::raw("abＡcd")]);
        let out = highlight_range(&line.spans, 3, 4, Style::default().bg(Color::Yellow));
        let mid: String = out
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(mid, "Ａ");
    }

    #[test]
    fn extract_text_joins_partial_rows() {
        let lines = vec![
            Line::from("0123456789"),
            Line::from("abcdefghij"),
            Line::from("0123456789"),
        ];
        let text = extract_selection_text(
            &lines,
            SelPos { line: 0, col: 8 },
            SelPos { line: 2, col: 2 },
        );
        assert_eq!(text, "89\nabcdefghij\n01");
    }

    #[test]
    fn wrapped_continuation_maps_via_drawn_frame() {
        let mut app = test_app();
        app.show_thinking = false;
        app.messages.push(Msg::new(
            "assistant",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));
        let _ = render_text(&mut app, 40, 16);
        assert!(app.vis_rows.len() >= 2, "wraps: {:?}", app.vis_rows);

        let body: Vec<(usize, usize)> = app.vis_rows.clone();
        assert!(body.windows(2).all(|w| w[0].0 == w[1].0 || w[1].1 == 0));
        let inner = app.chat_inner().unwrap();
        let pos = app.screen_to_content(inner.x, inner.y + 1).expect("maps");
        assert_eq!(pos.line, body[1].0);
        assert_eq!(pos.col, body[1].1);
    }

    #[test]
    fn skill_new_request_parsing() {
        use std::path::PathBuf;
        let cwd = PathBuf::from("/proj");

        assert!(skill_new_request(&[], &cwd).is_err());
        assert!(skill_new_request(&["--local"], &cwd).is_err());

        let (desc, target) = skill_new_request(&["plot", "waveforms"], &cwd).unwrap();
        assert_eq!(desc, "plot waveforms");
        assert!(
            target.ends_with(".config/vioraharness/skills"),
            "{target:?}"
        );

        for flag in ["--local", "--here", "--project", "-l"] {
            let (desc, target) = skill_new_request(&[flag, "plot", "x"], &cwd).unwrap();
            assert_eq!(target, PathBuf::from("/proj/skills"), "{flag}");
            assert_eq!(desc, "plot x");
        }
        let (_, target) = skill_new_request(&["plot", "--local"], &cwd).unwrap();
        assert_eq!(target, PathBuf::from("/proj/skills"));
    }

    #[test]
    fn skill_creator_prompt_content() {
        use std::path::PathBuf;
        let p = skill_creator_prompt("plot waveforms", &PathBuf::from("/g/skills"));
        assert!(p.contains("plot waveforms"), "carries the description");
        assert!(p.contains("/g/skills"), "carries the target dir");
        assert!(
            p.contains("YOU choose a short lowercase name"),
            "model names it"
        );
        assert!(p.contains("name: waveforms"), "format example present");
        assert!(p.contains("never overwrite"), "no clobber rule");
        assert!(p.contains("read tool"), "verification step");
    }

    #[test]
    fn skill_file_renders_frontmatter() {
        let body = render_skill_file("demo", "Does demo things", &["demo".into(), "try".into()]);
        assert!(body.starts_with("---\nname: demo\n"));
        assert!(body.contains("description: Does demo things"));
        assert!(body.contains("triggers: [demo, try]"));
        assert!(body.contains("# Demo Skill"));
        let empty = render_skill_file("e", "d", &[]);
        assert!(empty.contains("triggers: []"));
    }

    #[test]
    fn skill_new_command_dispatch() {
        let mut app = test_app();
        let busy_before = app.busy;
        app.handle_slash("/skill-new");
        assert!(!busy_before && !app.busy, "usage path never submits");
        let last = app.messages.last().expect("usage message");
        assert!(
            last.content.contains("usage: /skill-new"),
            "{:?}",
            last.content
        );

        let mut app = test_app();
        app.busy = true;
        app.handle_slash("/skill-new plot things");
        let last = app.messages.last().expect("busy message");
        assert!(last.content.contains("busy"), "{:?}", last.content);
    }

    #[test]
    fn wrap_preserving_rows_fit_and_chain() {
        use unicode_width::UnicodeWidthStr;
        let line = Line::from("aaa bbbb cc dddddddd ee");
        let rows = wrap_line_preserving(&line, 10);
        assert!(rows.len() >= 3, "wraps: {rows:?}");
        for (spans, _) in &rows {
            let w: usize = spans.iter().map(|s| s.content.width()).sum();
            assert!(w <= 10, "row fits: {w}");
        }

        let total: usize = "aaa bbbb cc dddddddd ee".width();
        let last = rows.last().unwrap();
        let last_w: usize = last.0.iter().map(|s| s.content.width()).sum();
        assert_eq!(last.1 + last_w, total);

        let joined: String = rows
            .iter()
            .map(|(spans, _)| spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, "aaa bbbb cc dddddddd ee");
        let tabbed = wrap_line_preserving(&Line::from("ab\tcd"), 20);
        let t: String = tabbed[0].0.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(t, "ab  cd", "tab expands to 4-stop");
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
    fn wrapped_drag_copy_extracts_right_text() {
        let lines = vec![Line::from("0123456789abcdef"), Line::from("gh")];

        let mut rows: Vec<(usize, usize)> = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            for (_, col) in wrap_line_preserving(line, 10) {
                rows.push((li, col));
            }
        }
        assert_eq!(rows, vec![(0, 0), (0, 10), (1, 0)]);

        let text = extract_selection_text(
            &lines,
            SelPos { line: 0, col: 10 },
            SelPos { line: 0, col: 16 },
        );
        assert_eq!(text, "abcdef");
    }

    fn mouse_app() -> App {
        let mut app = test_app();
        app.chat_area = Rect::new(0, 1, 100, 20);
        app.view_start = 10;
        app.view_total = 100;

        app.vis_rows = (0..100usize).map(|i| (i, 0)).collect();
        app
    }

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

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

    fn pending_question() -> (
        vioraharness_core::permissions::PendingQuestion,
        tokio::sync::oneshot::Receiver<vioraharness_core::permissions::QuestionResult>,
    ) {
        use vioraharness_core::permissions::{PendingQuestion, QuestionItem, QuestionOption};
        let (tx, rx) = tokio::sync::oneshot::channel();
        let q = PendingQuestion {
            id: "q1".into(),
            questions: vec![
                QuestionItem {
                    question: "Pick one".into(),
                    header: "H1".into(),
                    multi_select: false,
                    options: vec![
                        QuestionOption {
                            label: "A".into(),
                            description: "first".into(),
                        },
                        QuestionOption {
                            label: "B".into(),
                            description: String::new(),
                        },
                    ],
                },
                QuestionItem {
                    question: "Pick many".into(),
                    header: String::new(),
                    multi_select: true,
                    options: vec![
                        QuestionOption {
                            label: "X".into(),
                            description: String::new(),
                        },
                        QuestionOption {
                            label: "Y".into(),
                            description: String::new(),
                        },
                    ],
                },
            ],
            tx,
        };
        (q, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
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
    fn list_marker_and_title_detection() {
        assert_eq!(
            split_list_marker("- item"),
            Some(("-".to_string(), "item".to_string()))
        );
        assert_eq!(
            split_list_marker("2. **Declarative UI**"),
            Some(("2.".to_string(), "**Declarative UI**".to_string()))
        );
        assert_eq!(split_list_marker("3.14 is pi"), None);
        assert_eq!(split_list_marker("word - x"), None);
        assert_eq!(
            split_list_marker("- "),
            Some(("-".to_string(), "".to_string()))
        );
        assert!(is_section_title("**Declarative UI**"));
        assert!(!is_section_title("plain item"));
        assert!(!is_section_title("**unclosed"));
    }

    #[test]
    fn headers_parse_inline_markdown() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "### 2. **Declarative UI**\nBody with **bold** and `code`.\n## Plain Header",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("**"), "no raw markers leak:\n{text}");
        assert!(text.contains("Declarative UI"));
        assert!(text.contains("Plain Header"));
    }

    #[test]
    fn unclosed_fence_gets_closing_rule() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "```bash\necho oops, never closed\n- a bullet after",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains('└'), "auto-closed fence renders bottom rule");
    }

    #[test]
    fn bullet_wraps_with_hanging_indent() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "- Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));

        let text = render_text(&mut app, 40, 20);
        let rows: Vec<&str> = text.lines().collect();

        let first = rows.iter().find(|l| l.contains("Alpha")).unwrap();
        assert!(first.contains('-'), "marker on first row: {first:?}");
        let strip_border = |l: &&str| {
            l.trim_start()
                .strip_prefix('│')
                .unwrap_or(l.trim_start())
                .to_string()
        };
        let cont = rows
            .iter()
            .skip_while(|l| !l.contains("Alpha"))
            .nth(1)
            .unwrap();
        let inner = strip_border(cont);
        assert!(
            inner.trim_start().starts_with(|c: char| c.is_alphabetic()),
            "continuation has no marker: {cont:?}"
        );
        assert!(
            inner.starts_with("    "),
            "continuation hangs under item text: {cont:?}"
        );
    }

    #[test]
    fn fence_rules_align_and_code_bg_uniform() {
        let open = fence_open_spans("bash");
        let close = fence_close_spans();
        let open_w: usize = open.iter().map(|s| s.content.width()).sum();
        let close_w: usize = close.iter().map(|s| s.content.width()).sum();
        assert_eq!(open_w, close_w, "fence top/bottom rules align");
        assert_eq!(open_w, FENCE_WIDTH);

        let code_bg = Theme::code_block().bg;
        for line in [
            "# a comment",
            "export PATH=\"$PATH:/x\"",
            "flutter run",
            "let x = \"s\"; // done",
        ] {
            for s in highlighted_code_spans(line) {
                assert_eq!(s.style.bg, code_bg, "uniform bg for {line:?}");
            }
        }
    }

    #[test]
    fn selection_paints_full_range_including_gutter() {
        let sel_bg = Theme::text_selection().bg;
        let gutter = Span::styled("  ", Style::default());
        let rest = highlighted_code_spans("  Widget build(BuildContext context) {");
        let mut spans = vec![gutter];
        spans.extend(rest);
        spans.insert(1, Span::styled("│ ", Theme::code_block()));
        let out = highlight_range(&spans, 0, usize::MAX, Theme::text_selection());
        assert!(!out.is_empty());
        for s in &out {
            assert_eq!(s.style.bg, sel_bg, "all cols selected: {:?}", s.content);
        }

        let out = highlight_range(&spans, 4, 8, Theme::text_selection());
        let painted: String = out
            .iter()
            .filter(|s| s.style.bg == sel_bg)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(painted.chars().count(), 4);
    }

    #[test]
    fn blank_lines_render_singly() {
        let mut app = test_app();
        app.messages.push(Msg::new("assistant", "aaa\n\nbbb"));
        let text = render_text(&mut app, 100, 14);
        let chat: Vec<&str> = text.lines().skip(1).take(7).collect();
        let joined = chat.join("\n");
        let aaa_at = chat.iter().position(|l| l.contains("aaa")).unwrap();
        let bbb_at = chat.iter().position(|l| l.contains("bbb")).unwrap();
        assert_eq!(
            bbb_at,
            aaa_at + 2,
            "exactly one blank row between paragraphs:\n{joined}"
        );
    }

    #[test]
    fn full_render_stays_fast_on_large_chats() {
        let mut app = test_app();
        let body = "## Section\n\n### 2. **Declarative UI**\nBody with **bold**, *italic* and `code` spans across several words.\n\n- Alpha bullet with enough words to wrap across rows on narrow screens\n- Beta bullet\n\n| A | B |\n|---|---|\n| one | two |\n| three | four |\n\n```rust\nfn main() {\n    // comment\n    let s = \"hi\";\n    println!(\"{s}\");\n}\n```\n";
        for i in 0..60 {
            app.messages
                .push(Msg::new("user", format!("prompt number {i}")));
            app.messages.push(Msg::new("assistant", body));
        }
        let t0 = std::time::Instant::now();
        let text = render_text(&mut app, 120, 40);
        let ms = t0.elapsed().as_millis();
        println!("full render of {} msgs took {ms}ms", app.messages.len());
        assert!(text.contains("Declarative UI"));
        assert!(
            ms < 3000,
            "render stall: {ms}ms for a large chat (theme cache? memo?)"
        );
    }

    #[test]
    fn visual_dump_table_and_code() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "## Core Concepts\n\n### 2. **Declarative UI**\nYou describe **what** the UI should look like for a given state, not **how** to transition between states. Flutter handles the rendering pipeline.\n\n### 4. **Rendering Engine (Skia)**\nFlutter draws everything using **Skia**. This means:\n- **Consistent** look and feel across platforms with pixel-perfect control over every pixel on the screen.\n- No bridge overhead unlike React Native and other cross-platform frameworks.\n\n| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich, customizable widgets | Dart is less common than JS/Swift/Kotlin |\n\n## Quick Start\n\n```bash\ngit clone https://github.com/flutter/flutter.git\nexport PATH=\"$PATH:pwd/flutter/bin\"\nflutter create my_app\n```",
        ));
        let text = render_text(&mut app, 100, 48);
        println!("=== VISUAL DUMP START ===\n{text}\n=== VISUAL DUMP END ===");
        assert!(text.contains('┌') && text.contains('┐'));
        assert!(text.contains('└') && text.contains('┘'));
        assert!(!text.contains("|---|"), "no raw delimiter row leaks");
        assert!(!text.contains("**"), "no raw bold markers leak");
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
    fn thinking_title_style_is_demoted_from_labels() {
        use ratatui::style::Modifier;
        let s = Theme::thinking_title_style();
        assert!(
            s.add_modifier.contains(Modifier::ITALIC),
            "thinking header should be italic like modern agents"
        );
        assert!(
            !s.add_modifier.contains(Modifier::BOLD),
            "thinking header must not use the bold of real labels"
        );
    }

    #[test]
    fn thinking_header_renders_distinct_marker() {
        let mut app = test_app();
        app.show_thinking = true;
        let mut m = Msg::new("assistant", "hello");
        m.reasoning = Some("private chain of thought".into());
        app.messages.push(m);
        let text = render_text(&mut app, 80, 30);
        assert!(
            text.contains("✦"),
            "collapsed thinking header carries the distinct marker"
        );
        assert!(text.contains("Thinking"), "thinking title renders");
        assert!(
            !text.contains("(Reasoning)"),
            "redundant parenthetical label is gone from chat lines"
        );
    }

    #[test]
    fn tool_call_cards_are_human_readable() {
        let bash_args =
            serde_json::json!({"command": "ls -la . 2>&1; cat ~/web/webos/app.js 2>&1 | wc -l"})
                .to_string();
        let bash = pretty_tool_args("bash", &bash_args);
        assert!(
            !bash.contains("2>&1"),
            "shell redirect noise stripped: {bash}"
        );
        assert!(bash.contains("ls -la"), "command kept: {bash}");

        let write_args =
            serde_json::json!({"path": "./index.html", "content": "<!doctype html>\n<html>\n"})
                .to_string();
        assert_eq!(
            pretty_tool_args("write", &write_args),
            "./index.html · 2 lines"
        );

        let todos_args = serde_json::json!({"todos": [
            {"content": "Scaffold project", "status": "completed", "priority": "high"},
            {"content": "Build OS core", "status": "in_progress", "priority": "high"},
            {"content": "Implement apps", "status": "pending", "priority": "medium"},
        ]})
        .to_string();
        let todos = pretty_tool_args("todowrite", &todos_args);
        assert_eq!(todos, "1/3 todos done • now: Build OS core");
        assert!(!todos.contains("{content"), "no raw todo blob: {todos}");

        let write_res = serde_json::json!({
            "ok": true, "path": "./index.html",
            "old_lines": 0, "new_lines": 136,
            "diff": "--- a/./index.html\n+++ b/./index.html\n@@",
            "diff_preview": "@@ preview", "bytes": 5082,
        })
        .to_string();
        assert_eq!(
            pretty_tool_result_wide(&write_res, false),
            "./index.html · 0→136 lines · 5082 bytes"
        );
    }

    #[test]
    fn truncated_results_salvage_key_facts() {
        let mut broken = serde_json::json!({
            "ok": true, "path": "./app.js",
            "old_lines": 0, "new_lines": 1922, "bytes": 48160,
            "diff": "--- a/./app.js\n+++ b/./app.js\n@@ -0,0 +1,1922 @@\n+/* ELECTRON OS",
        })
        .to_string();
        broken.truncate(220);
        let pretty = pretty_tool_result_wide(&broken, false);
        assert!(
            pretty.contains("./app.js") && pretty.contains("1922 lines"),
            "write facts salvaged: {pretty}"
        );
        assert!(!pretty.contains("{\"bytes\""), "no raw JSON head: {pretty}");

        let mut bash_broken = String::from(
            "{\"ok\":true,\"code\":0,\"stdout\":\"total 8\\ndrwxrwxr-x 2 jnd jnd 4096 Sep 6\\nmore and more",
        );
        bash_broken.truncate(bash_broken.len() - 10);
        let bash = pretty_tool_result_wide(&bash_broken, false);
        assert!(bash.contains("total 8"), "stdout head salvaged: {bash}");
        assert!(!bash.contains("\\n"), "no escaped newlines leak: {bash}");
    }

    #[test]
    fn tool_verbosity_levels_parse() {
        assert_eq!(parse_tool_verbosity("hidden"), ToolVerbosity::Hidden);
        assert_eq!(parse_tool_verbosity("off"), ToolVerbosity::Hidden);
        assert_eq!(parse_tool_verbosity("quiet"), ToolVerbosity::Quiet);
        assert_eq!(parse_tool_verbosity("name"), ToolVerbosity::Quiet);
        assert_eq!(parse_tool_verbosity("compact"), ToolVerbosity::Compact);
        assert_eq!(parse_tool_verbosity("full"), ToolVerbosity::Full);
        assert_eq!(parse_tool_verbosity("verbose"), ToolVerbosity::Full);
        assert_eq!(parse_tool_verbosity("bogus"), ToolVerbosity::Compact);
        assert_eq!(parse_tool_verbosity(""), ToolVerbosity::Compact);

        let mut app = test_app();
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Compact);
        app.tool_display_default = "quiet".into();
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Quiet);
        app.tool_display.insert("bash".into(), "hidden".into());
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Hidden);
        assert_eq!(app.tool_verbosity("write"), ToolVerbosity::Quiet);
    }

    #[test]
    fn tool_cards_respect_verbosity() {
        fn card_app(level: &str) -> App {
            let mut app = test_app();
            app.tool_display.insert("bash".into(), level.into());
            let mut m = Msg::new("assistant", "working");
            m.items.push(Content::ToolCall {
                id: "call_hidden_test_1".into(),
                name: "bash".into(),
                args: "{\"command\": \"ls -la myproject\"}".into(),
                status: ToolStatus::Done,
            });
            m.items.push(Content::ToolResult {
                id: "call_hidden_test_1".into(),
                content: "{\"ok\":true,\"code\":0,\"stdout\":\"total 8\"}".into(),
                ok: true,
            });
            app.messages.push(m);
            app
        }

        let hidden = render_text(&mut card_app("hidden"), 100, 30);
        assert!(
            !hidden.contains("bash"),
            "hidden tool leaves no card: {hidden}"
        );

        let quiet = render_text(&mut card_app("quiet"), 100, 30);
        assert!(quiet.contains("bash"), "quiet keeps the name");
        assert!(
            !quiet.contains("ls -la"),
            "quiet drops the preview: {quiet}"
        );
        assert!(
            !quiet.contains("total 8"),
            "quiet drops the output: {quiet}"
        );

        let compact = render_text(&mut card_app("compact"), 100, 30);
        assert!(compact.contains("ls -la"), "compact shows the summary");
    }

    #[test]
    fn tool_full_verbosity_widens_previews() {
        let filler = "x".repeat(180);
        let cmd = format!("ls -la STARTMARKER {filler} ENDMARKER");
        let args = serde_json::json!({"command": cmd}).to_string();
        let narrow = pretty_tool_args_wide("bash", &args, false);
        let wide = pretty_tool_args_wide("bash", &args, true);
        assert!(!narrow.contains("ENDMARKER"), "compact caps long commands");
        assert!(narrow.ends_with('…'), "compact marks truncation");
        assert!(wide.contains("ENDMARKER"), "full keeps long commands");
        assert!(
            !wide.contains("2>&1") || !cmd.contains("2>&1"),
            "no redirect noise"
        );
    }

    #[test]
    fn read_card_shows_lines_not_content() {
        let paged = serde_json::json!({
            "ok": true, "path": "README.md",
            "content": "# Title\nbody text here\nmore\n",
            "offset": 10, "limit": 20, "total_lines": 100,
        })
        .to_string();
        assert_eq!(
            pretty_tool_result_wide(&paged, false),
            "README.md (offset 10, limit 20, total 100)"
        );

        let full = serde_json::json!({
            "ok": true, "path": "README.md",
            "content": "# Title\nbody\n", "total_lines": 2,
        })
        .to_string();
        assert_eq!(pretty_tool_result_wide(&full, false), "README.md (total 2)");

        let skill =
            serde_json::json!({"ok": true, "path": "netlist", "content": "skill body text"})
                .to_string();
        let preview = pretty_tool_result_wide(&skill, false);
        assert!(
            preview.contains("skill body"),
            "un-paged content keeps preview: {preview}"
        );
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
    fn edit_card_shows_inline_mini_diff() {
        let diff = "--- a/AGENTS.md\n+++ b/AGENTS.md\n@@ -43 +43 @@\n-old line here\n+new line here\n context\n+second add\n+third add\n+fourth add\n+fifth add\n";
        let (shown, rest) = diff_card_lines(diff, 4);
        assert_eq!(shown.len(), 4, "caps shown lines");
        assert_eq!(rest, 2, "counts the remainder");
        assert_eq!(shown[0], (false, "old line here".to_string()));
        assert_eq!(shown[1], (true, "new line here".to_string()));

        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_edit_diff_1".into(),
            name: "edit".into(),
            args: "{\"path\": \"AGENTS.md\"}".into(),
            status: ToolStatus::Done,
        });
        let res = serde_json::json!({
            "ok": true, "path": "AGENTS.md",
            "old_lines": 69, "new_lines": 70, "bytes": 3000,
            "diff_preview": diff,
        })
        .to_string();
        m.items.push(Content::ToolResult {
            id: "call_edit_diff_1".into(),
            content: res,
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("+ new line here"), "added line on card");
        assert!(text.contains("- old line here"), "removed line on card");
        assert!(text.contains("+2 more"), "remainder hint on card");
    }

    #[test]
    fn compact_notice_keeps_header_gauge_honest() {
        assert_eq!(App::parse_compact_freed("waiting 4s for rate limit"), 0);
        assert_eq!(
            App::parse_compact_freed("Compacted context: 130→20 msgs, freed ~42k tokens"),
            42_000
        );
        assert_eq!(
            App::parse_compact_freed("Compacted context: 40→20 msgs, freed ~3k tokens"),
            3_000
        );

        let mut app = test_app();
        app.ctx_freed_tokens +=
            App::parse_compact_freed("Compacted context: 40→20 msgs, freed ~3k tokens");
        assert_eq!(app.ctx_freed_tokens, 3_000);
    }

    #[test]
    fn compact_refuses_while_busy() {
        let mut app = test_app();
        app.busy = true;
        app.handle_slash("/compact");
        assert!(app.compact_rx.is_none(), "no task while busy");
        let last = app.messages.last().expect("notice");
        assert!(
            last.content.contains("busy"),
            "told to wait: {}",
            last.content
        );
    }

    #[tokio::test]
    async fn compact_poll_reports_not_needed_on_small_session() {
        let _env_guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let db = std::env::temp_dir().join(format!("vh_compact_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let prev = std::env::var("VIORAHARNESS_DB").ok();
        std::env::set_var("VIORAHARNESS_DB", &db);

        let mut app = test_app();
        app.handle_slash("/compact");
        assert!(app.compact_rx.is_some(), "task spawned");
        for _ in 0..200 {
            app.poll_compact();
            if app.compact_rx.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(app.compact_rx.is_none(), "task completed");
        let last = app.messages.last().expect("report");
        assert!(
            last.content.contains("not needed"),
            "planner note shown: {}",
            last.content
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn shrink_rgba_averages_and_caps() {
        let px: Vec<u8> = vec![0, 0, 0, 255, 100, 0, 0, 255, 0, 100, 0, 255, 0, 0, 100, 255];
        let (w, h, out) = shrink_rgba(2, 2, &px, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![25, 25, 25, 255]);

        let (w, h, out) = shrink_rgba(2, 2, &px, 1024);
        assert_eq!((w, h), (2, 2));
        assert_eq!(out, px);

        let big = vec![128u8; 200 * 100 * 4];
        let (w, h, out) = shrink_rgba(200, 100, &big, 100);
        assert_eq!((w, h), (100, 50));
        assert_eq!(out.len(), 100 * 50 * 4);
    }

    #[test]
    fn png_encode_decode_roundtrip() {
        let px: Vec<u8> = (0..(8 * 6 * 4)).map(|i| (i % 251) as u8).collect();
        let png = png_from_rgba(8, 6, &px).expect("encodes");
        assert!(png.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        let (w, h, back) = rgba_from_png(&png).expect("decodes");
        assert_eq!((w, h), (8, 6));
        assert_eq!(back, px);

        let rgb: Vec<u8> = vec![10, 20, 30, 40, 50, 60];
        let mut rgb_png: Vec<u8> = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut rgb_png, 2, 1);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        let (w, h, back) = rgba_from_png(&rgb_png).expect("rgb decodes");
        assert_eq!((w, h), (2, 1));
        assert_eq!(back, vec![10, 20, 30, 255, 40, 50, 60, 255]);

        assert!(rgba_from_png(b"not a png").is_none());
        assert!(rgba_from_png(&[]).is_none());
    }

    #[test]
    fn tool_result_line_shows_tool_name_not_call_id() {
        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            name: "write".into(),
            args: "{}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            content: "{\"ok\":true,\"path\":\"./index.html\",\"old_lines\":0,\"new_lines\":136,\"bytes\":5082}".into(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("✔ write"), "result line uses tool name");
        assert!(
            !text.contains("call_01a07"),
            "raw provider id hidden from result line"
        );
        assert!(
            text.contains("136 lines"),
            "write summary renders on the card"
        );
    }
    #[test]
    fn slash_routing_sends_unknown_slash_as_prompt() {
        for known in [
            "/help",
            "/h",
            "/clear",
            "/sessions",
            "/ls",
            "/providers",
            "/new",
            "/resume",
            "/r",
            "/fork",
            "/rename",
            "/archive",
            "/delete",
            "/export",
            "/model",
            "/skills",
            "/skill-new",
            "/theme",
            "/thinking",
            "/permissions",
            "/perms",
            "/verbosity",
            "/diff",
            "/output",
            "/view",
            "/undo",
            "/compact",
            "/quit",
            "/q",
            "/exit",
        ] {
            assert!(App::is_known_slash(known), "{known} routes to dispatcher");
            assert!(
                App::is_known_slash(&format!("{known} some args")),
                "{known} with args routes too"
            );
        }
        for prompt in [
            "/home/jnd/Pictures/shot.png",
            "/usr/bin/python3 --version",
            "/",
            "/viewx",
            "/undo2",
            "run /tmp/x.cir",
            "",
        ] {
            assert!(!App::is_known_slash(prompt), "{prompt:?} sends as prompt");
        }
    }

    #[test]
    fn paste_chips_expand_and_drop_orphans() {
        let long: String = (0..47)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(is_long_paste(&long));
        assert!(!is_long_paste("short paste"));
        let chip = paste_chip(3, 47);
        assert_eq!(chip, "[paste #3 · 47 lines]");

        let with_chip = format!("look at this {chip} please");
        assert_eq!(
            expand_paste_chips(&with_chip, &[(3, long.clone())]),
            format!("look at this {long} please")
        );
        assert_eq!(
            expand_paste_chips("no chips here", &[(3, long.clone())]),
            "no chips here"
        );

        let img = "[image · shot.png · 800×600]";
        assert_eq!(expand_paste_chips(img, &[(3, long)]), img);
        assert_eq!(image_chip("shot.png · 800×600"), img);
    }

    #[test]
    fn image_path_detection() {
        let dir = std::env::temp_dir().join(format!("vh_paste_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        let txt = dir.join("notes.txt");
        std::fs::write(&png, b"fakepng").unwrap();
        std::fs::write(&txt, b"hi").unwrap();
        let ps = png.to_string_lossy().to_string();
        assert_eq!(detect_image_path(&ps).unwrap(), png);
        assert_eq!(
            detect_image_path(&format!("\"{ps}\"")).unwrap(),
            png,
            "quoted drag-drop"
        );
        assert_eq!(
            detect_image_path(&format!("file://{ps}")).unwrap(),
            png,
            "file:// prefix"
        );
        assert!(
            detect_image_path(&txt.to_string_lossy()).is_none(),
            "txt rejected"
        );
        assert!(detect_image_path(&dir.join("missing.png").to_string_lossy()).is_none());
        assert!(
            detect_image_path("two\nlines.png").is_none(),
            "multiline rejected"
        );
        assert!(detect_image_path("").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_file_loading() {
        let dir = std::env::temp_dir().join(format!("vh_imgload_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let px: Vec<u8> = (0..(16 * 16 * 4)).map(|i| (i % 251) as u8).collect();
        let png_path = dir.join("a.png");
        std::fs::write(&png_path, png_from_rgba(16, 16, &px).unwrap()).unwrap();
        let (b64, mime, label) = load_image_file(&png_path).expect("png loads");
        assert_eq!(mime, "image/png");
        assert!(
            label.contains("a.png") && label.contains("16×16"),
            "{label}"
        );
        assert!(!b64.is_empty());

        let jpg_path = dir.join("b.jpg");
        std::fs::write(&jpg_path, vec![7u8; 1000]).unwrap();
        let (_, mime, _) = load_image_file(&jpg_path).expect("jpg loads");
        assert_eq!(mime, "image/jpeg");

        let bmp_path = dir.join("c.bmp");
        std::fs::write(&bmp_path, vec![7u8; 100]).unwrap();
        assert!(load_image_file(&bmp_path).is_err(), "bmp refused");
        let big_path = dir.join("d.jpg");
        std::fs::write(&big_path, vec![7u8; 700_000]).unwrap();
        assert!(load_image_file(&big_path).is_err(), "oversize refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_chat_messages_collapse_with_hint() {
        let short = "hello";
        let (t, h) = collapse_long_content(short, false);
        assert_eq!(t, short);
        assert!(h.is_none());
        let long: String = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (t, h) = collapse_long_content(&long, false);
        assert_eq!(h, Some(20));
        assert_eq!(t.lines().count(), CHAT_COLLAPSE_HEAD);
        assert!(t.contains("line 0") && !t.contains("line 19"));
        let (t, h) = collapse_long_content(&long, true);
        assert!(h.is_none() && t == long, "expanded shows all");

        let code = "```rust\n".to_string()
            + &(0..20)
                .map(|i| format!("l{i}"))
                .collect::<Vec<_>>()
                .join("\n");
        let (t, _) = collapse_long_content(&code, false);
        assert_eq!(
            t.lines().filter(|l| l.trim().starts_with("```")).count() % 2,
            0,
            "fence balanced"
        );

        let mut app = test_app();
        let mut m = Msg::new("user", long.clone());
        m.timestamp = "04:10".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 40);
        assert!(text.contains("line 0"), "head shown");
        assert!(!text.contains("line 19"), "tail hidden");
        assert!(text.contains("Ctrl+X to expand"), "hint shown");
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "expanded reveals all");
        assert!(!text.contains("Ctrl+X to expand"), "hint gone when open");

        let mut app = test_app();
        let mut m = Msg::new("assistant", long.clone());
        m.timestamp = "04:11".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "model answer fully visible");
        assert!(!text.contains("Ctrl+X to expand"), "no hint on responses");
    }

    #[test]
    fn image_result_card_shows_vision_summary() {
        let fresh = serde_json::json!({
            "ok": true, "path": "./shot.png", "mime": "image/jpeg",
            "width": 1024, "height": 768, "bytes": 245760,
            "base64": "AAAA",
        })
        .to_string();
        assert_eq!(
            pretty_tool_result_wide(&fresh, false),
            "./shot.png · 1024×768 · 240KB → vision"
        );

        let stripped = serde_json::json!({
            "ok": true, "path": "./shot.png", "mime": "image/jpeg",
            "width": 1024, "height": 768, "bytes": 245760,
            "base64_len": 327680, "base64_preview": "AAAA... (327680 chars total)",
        })
        .to_string();
        assert_eq!(
            pretty_tool_result_wide(&stripped, false),
            "./shot.png · 1024×768 · 240KB → vision"
        );
        assert!(
            !pretty_tool_result_wide(&fresh, false).contains("AAAA"),
            "no pixel dump on the card"
        );
    }

    #[test]
    fn short_task_ids_use_low_entropy_bits() {
        assert_eq!(short_task_id("task_d2e3b023c7f919"), "23c7f919");
        assert_eq!(short_task_id("task_abc"), "abc");
        assert_eq!(short_task_id("plainid"), "plainid");
        assert_eq!(short_task_id(""), "");
        assert_ne!(
            short_task_id("task_aaaa000011112222"),
            short_task_id("task_aaaa000033334444"),
            "same-ms spawns still distinct"
        );
    }

    #[test]
    fn background_launch_card_names_task() {
        let res = serde_json::json!({
            "ok": true, "background": true, "task_id": "task_abc123",
            "status": "running", "log": "/tmp/x.log",
        })
        .to_string();
        assert_eq!(
            pretty_tool_result_wide(&res, false),
            "task abc123 · running — log /tmp/x.log"
        );

        let args = serde_json::json!({"command": "sleep 60", "background": true}).to_string();
        assert_eq!(pretty_tool_args("bash", &args), "sleep 60 ↩ background");
        let args = serde_json::json!({"command": "ls"}).to_string();
        assert_eq!(pretty_tool_args("bash", &args), "ls");
    }

    static DB_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    fn with_temp_db(tag: &str) -> (std::path::PathBuf, Option<String>) {
        let db = std::env::temp_dir().join(format!("vh_tuidb_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let prev = std::env::var("VIORAHARNESS_DB").ok();
        std::env::set_var("VIORAHARNESS_DB", &db);
        (db, prev)
    }

    fn restore_db_env(prev: Option<String>, db: &std::path::Path) {
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_file(db);
    }

    async fn wait_task_done(id: &str) {
        use vioraharness_core::tools::tasks;
        let start = std::time::Instant::now();
        loop {
            if let Some(cur) = tasks::get_task(id) {
                if cur.status != tasks::BgStatus::Running {
                    break;
                }
            }
            assert!(start.elapsed().as_secs() < 10, "task {id} finished");
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn task_completion_wakes_idle_chat() {
        use vioraharness_core::tools::tasks;
        let _env_guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (db, prev) = with_temp_db("wake");
        let mut app = test_app();
        vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .create_session(&app.session_id, "m", None)
            .expect("session");
        let t = tasks::spawn_task("echo wake-probe-xyz", "/tmp");
        let t2 = tasks::spawn_task("echo wake-probe-second", "/tmp");
        wait_task_done(&t.id).await;
        wait_task_done(&t2.id).await;
        let waked = app.poll_task_completions();
        for probe in [&t.id, &t2.id] {
            assert!(waked.contains(probe), "every completion tracked: {waked:?}");
        }
        assert!(app.busy, "follow-up turn started");
        // First completion starts the turn now, the rest queue behind it —
        // none degrade to a bare notice.
        assert_eq!(
            app.queued_prompts.len() + 1,
            waked.len(),
            "exactly the live wake starts, the rest queue"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t.id)) && m.content.contains("done")),
            "notice names the finished task"
        );

        let rows = vioraharness_core::session::SessionStore::new(db.to_string_lossy().as_ref())
            .expect("temp store")
            .get_messages_detailed(&app.session_id)
            .expect("rows");
        assert!(
            rows.iter()
                .any(|m| m.content.contains(&short_task_id(&t.id))),
            "notice row stored"
        );

        if let Some(h) = app.pending.take() {
            h.abort();
        }
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn task_completion_stays_quiet_when_busy_or_off() {
        use vioraharness_core::tools::tasks;
        let _env_guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (db, prev) = with_temp_db("quiet");

        let mut app = test_app();
        let t = tasks::spawn_task("echo quiet-probe-xyz", "/tmp");
        wait_task_done(&t.id).await;
        app.busy = true;
        let waked = app.poll_task_completions();
        assert!(
            waked.contains(&t.id),
            "busy completion tracked for later: {waked:?}"
        );
        assert!(app.pending.is_none(), "no turn started while busy");
        assert!(
            app.queued_prompts
                .iter()
                .any(|q| q.send.contains(&short_task_id(&t.id))),
            "own follow-up queued behind live turn (siblings may queue too)"
        );
        assert!(
            app.queued_prompts[0]
                .send
                .contains("[background task finished]"),
            "queued payload is the wake prompt"
        );
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t.id))),
            "notice still posted"
        );

        let t2 = tasks::spawn_task("echo quiet-probe-abc", "/tmp");
        wait_task_done(&t2.id).await;
        app.busy = false;
        app.wake_on_tasks = false;
        let waked = app.poll_task_completions();
        assert!(waked.is_empty(), "no wake when disabled");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains(&short_task_id(&t2.id))),
            "notice still posted"
        );
        assert!(!app.busy, "stays idle");
        restore_db_env(prev, &db);
    }

    #[test]
    fn busy_safe_slash_lists() {
        for safe in [
            "/help",
            "/sessions",
            "/providers",
            "/model",
            "/skills",
            "/theme",
            "/thinking",
            "/permissions",
            "/verbosity",
            "/diff",
            "/output",
            "/tasks",
            "/export",
            "/compact",
            "/quit",
        ] {
            assert!(App::is_busy_safe_slash(safe), "{safe} runs while busy");
        }
        for unsafe_ in [
            "/clear",
            "/new",
            "/resume",
            "/r",
            "/fork",
            "/rename",
            "/archive",
            "/delete",
            "/undo",
            "/skill-new",
        ] {
            assert!(
                !App::is_busy_safe_slash(unsafe_),
                "{unsafe_} waits for idle"
            );
        }
        assert!(
            !App::is_busy_safe_slash("/home/x/y.png"),
            "pasted path not slash"
        );
        assert!(!App::is_busy_safe_slash("hello"), "plain text not slash");
    }

    #[tokio::test]
    async fn typing_and_queue_work_while_busy() {
        let mut app = test_app();
        app.busy = true;

        app.handle_key(KeyCode::Char('h')).await.unwrap();
        app.handle_key(KeyCode::Char('i')).await.unwrap();
        assert_eq!(app.input.text, "hi");

        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.input.text, "", "input cleared on queue");
        assert_eq!(app.queued_prompts.len(), 1);
        assert_eq!(app.queued_prompts[0].send, "hi");
        assert_eq!(app.queued_prompts[0].session_id, app.session_id);
        assert!(app.busy, "running turn untouched");
        assert!(
            app.messages
                .iter()
                .any(|m| m.role == "user" && m.content == "hi"),
            "queued prompt echoed"
        );

        app.input.text = "second".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.queued_prompts.len(), 2);
    }

    #[tokio::test]
    async fn busy_enter_routes_slash_by_safety() {
        let mut app = test_app();
        app.busy = true;

        app.input.text = "/tasks".into();
        app.input.cursor = 6;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.popup, Popup::Tasks, "/tasks opens while busy");
        app.popup = Popup::None;

        let sid = app.session_id.clone();
        app.input.text = "/new".into();
        app.input.cursor = 4;
        app.handle_key(KeyCode::Enter).await.unwrap();
        assert_eq!(app.session_id, sid, "session untouched");
        assert_eq!(app.input.text, "/new", "input kept");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("waits for the current turn")),
            "defer notice shown"
        );
        assert!(app.queued_prompts.is_empty(), "commands never queue");
    }

    #[tokio::test]
    async fn queue_drains_on_idle_and_drops_on_mismatch() {
        let (db, prev) = with_temp_db("queue");
        let _env_guard = DB_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = test_app();

        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "drained-next".into(),
            image: None,
        });
        app.drain_queue();
        assert!(app.busy, "turn started");
        assert!(app.queued_prompts.is_empty(), "slot consumed");
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        app.busy = false;

        app.queued_prompts.push(QueuedPrompt {
            session_id: "other-session".into(),
            send: "stale".into(),
            image: None,
        });
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "fresh".into(),
            image: None,
        });
        app.drain_queue();
        assert!(app.busy, "fresh slot fired after dropping stale");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("session changed")),
            "drop notice shown"
        );
        if let Some(h) = app.pending.take() {
            h.abort();
        }
        restore_db_env(prev, &db);
    }

    #[tokio::test]
    async fn esc_cancel_drops_queue() {
        let mut app = test_app();
        app.busy = true;
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "doomed".into(),
            image: None,
        });
        app.handle_key(KeyCode::Esc).await.unwrap();
        assert!(!app.busy, "turn cancelled");
        assert!(app.queued_prompts.is_empty(), "queue dropped");
        assert!(
            app.messages
                .iter()
                .any(|m| m.content.contains("dropped 1 queued")),
            "drop notice shown"
        );
    }

    #[test]
    fn busy_input_renders_typeahead_hint() {
        let mut app = test_app();
        app.busy = true;
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Enter queues"), "title advertises queueing");
        assert!(text.contains("Type ahead"), "placeholder invites typing");
    }

    #[test]
    fn footer_shows_task_and_queue_counts() {
        use vioraharness_core::tools::tasks;

        let mut app = test_app();
        app.queued_prompts.push(QueuedPrompt {
            session_id: app.session_id.clone(),
            send: "later".into(),
            image: None,
        });
        let text = render_text(&mut app, 100, 30);
        let footer = text.lines().last().unwrap_or("").to_string();
        assert!(footer.contains("1 queued"), "queued count: {footer:?}");
    }

    #[tokio::test]
    async fn footer_shows_running_tasks() {
        use vioraharness_core::tools::tasks;
        let live = tasks::spawn_task("sleep 30", "/tmp");
        let mut app = test_app();
        let text = render_text(&mut app, 120, 30);
        let footer = text.lines().last().unwrap_or("").to_string();
        assert!(
            footer.contains("running") && footer.contains("task"),
            "running count: {footer:?}"
        );
        assert!(footer.contains("/tasks"), "panel hint: {footer:?}");
        assert!(tasks::kill_task(&live.id), "cleanup");
    }

    #[test]
    fn tasks_panel_renders_title_with_or_without_tasks() {
        let mut app = test_app();
        app.handle_slash("/tasks");
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("Background Tasks"), "panel title");
        assert!(
            text.contains("No background tasks") || text.contains("STATUS"),
            "empty state or column list"
        );
    }

    #[tokio::test]
    async fn tasks_panel_lists_and_kills() {
        use vioraharness_core::tools::tasks;

        let t = tasks::spawn_task("echo panel-probe-xyz", "/tmp");
        let start = std::time::Instant::now();
        loop {
            if let Some(cur) = tasks::get_task(&t.id) {
                if cur.status != tasks::BgStatus::Running {
                    break;
                }
            }
            assert!(start.elapsed().as_secs() < 10, "task finished");
            tokio::task::yield_now().await;
        }
        let mut app = test_app();
        app.handle_slash("/tasks");

        let text = render_text(&mut app, 160, 40);
        assert!(text.contains("Background Tasks"), "panel title");
        assert!(text.contains(&short_task_id(&t.id)), "task id shown");
        assert!(text.contains("panel-probe-xyz"), "command shown");

        let narrow = render_text(&mut app, 100, 30);
        for (i, line) in narrow.lines().enumerate() {
            assert!(line.chars().count() <= 100, "row {i} fits: {line:?}");
        }

        let live = tasks::spawn_task("sleep 30", "/tmp");
        let mut app = test_app();
        app.handle_slash("/tasks");

        app.task_cursor = vioraharness_core::tools::tasks::list_tasks()
            .iter()
            .position(|t| t.id == live.id)
            .expect("own task listed");

        app.handle_popup_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty()));
        assert_eq!(
            vioraharness_core::tools::tasks::get_task(&live.id)
                .expect("still tracked")
                .status,
            vioraharness_core::tools::tasks::BgStatus::Killed,
            "panel K kills the highlighted task"
        );
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
