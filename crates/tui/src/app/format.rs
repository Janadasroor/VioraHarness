use super::msg::truncate_chars;

pub(crate) const PASTE_CHIP_LINES: usize = 8;
pub(crate) const PASTE_CHIP_CHARS: usize = 600;

pub(crate) fn paste_chip(id: usize, lines: usize) -> String {
    format!("[paste #{id} · {lines} lines]")
}

pub(crate) fn image_chip(label: &str) -> String {
    format!("[image · {label}]")
}

pub(crate) fn short_task_id(id: &str) -> String {
    let hex = id.strip_prefix("task_").unwrap_or(id);
    let chars: Vec<char> = hex.chars().collect();
    if chars.len() > 8 {
        chars[chars.len() - 8..].iter().collect()
    } else {
        hex.to_string()
    }
}

/// Display id for registry errors (`err_` + 12 hex) — short 8 like tasks.
pub(crate) fn short_err_id(id: &str) -> String {
    let hex = id.strip_prefix("err_").unwrap_or(id);
    let chars: Vec<char> = hex.chars().collect();
    if chars.len() > 8 {
        chars[chars.len() - 8..].iter().collect()
    } else {
        hex.to_string()
    }
}

/// Short display id for tool calls (`call_abc123…`, `toolu_xyz…`, `c1`).
/// Strips the provider prefix and keeps the last 6 alphanumerics so chat
/// cards read `● bash · npm test · #a1b2c3` — unique enough for display,
/// never used as a key.
pub(crate) fn short_tool_id(id: &str) -> String {
    let mut s = id;
    for prefix in ["call_", "toolu_", "tool_", "task_", "err_"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    let clean: String = s.chars().filter(|c| c.is_alphanumeric()).collect();
    if clean.is_empty() {
        return s.chars().take(6).collect();
    }
    if clean.len() > 6 {
        clean[clean.len() - 6..].to_string()
    } else {
        clean
    }
}

pub(crate) fn expand_paste_chips(prompt: &str, texts: &[(usize, String)]) -> String {
    let mut out = prompt.to_string();
    for (id, text) in texts {
        let chip = paste_chip(*id, text.lines().count());
        if out.contains(&chip) {
            out = out.replace(&chip, text);
        }
    }
    out
}

pub(crate) fn is_long_paste(txt: &str) -> bool {
    txt.lines().count() > PASTE_CHIP_LINES || txt.chars().count() > PASTE_CHIP_CHARS
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ToolVerbosity {
    Hidden,
    Quiet,
    Compact,
    Full,
}

pub(crate) fn parse_tool_verbosity(s: &str) -> ToolVerbosity {
    match s.trim().to_lowercase().as_str() {
        "hidden" | "off" | "none" => ToolVerbosity::Hidden,
        "quiet" | "name" | "minimal" => ToolVerbosity::Quiet,
        "full" | "verbose" | "detail" => ToolVerbosity::Full,
        _ => ToolVerbosity::Compact,
    }
}

pub(crate) fn tool_verbosity_name(v: ToolVerbosity) -> &'static str {
    match v {
        ToolVerbosity::Hidden => "hidden",
        ToolVerbosity::Quiet => "quiet",
        ToolVerbosity::Compact => "compact",
        ToolVerbosity::Full => "full",
    }
}

pub(crate) fn load_tool_display_config() -> (String, std::collections::HashMap<String, String>) {
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

pub(crate) fn home_prefix() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

pub(crate) fn rel_path(p: &str) -> String {
    let home = home_prefix();
    let mut s = p.to_string();
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if s.starts_with(&cwd_str) {
            let rel = s[cwd_str.len()..].trim_start_matches('/').to_string();
            s = if rel.is_empty() { ".".into() } else { rel };
        }
    }
    if s.starts_with(&format!("{home}/")) {
        s = s.replacen(&format!("{home}/"), "~/", 1);
    }
    if s.len() > 150 {
        format!("…{}", &s[s.len() - 150..])
    } else {
        s
    }
}

pub(crate) fn rel_in_str(s: &str) -> String {
    let home = home_prefix();
    let mut out = s.to_string();
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        if out.contains(&cwd_str) {
            out = out.replace(&cwd_str, ".");
        }
    }
    out.replace(&format!("{home}/"), "~/")
}

pub(crate) fn summarize_todos(todos: &[serde_json::Value], done_override: Option<u64>) -> String {
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

pub(crate) fn pretty_tool_args(name: &str, args: &str) -> String {
    pretty_tool_args_wide(name, args, false)
}

pub(crate) fn pretty_tool_args_wide(name: &str, args: &str, wide: bool) -> String {
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
            "browser_screenshot" | "browser_dom" | "browser_pdf" | "browser_open" => {
                let target = v.get("target").and_then(|x| x.as_str()).unwrap_or("");
                if target.is_empty() {
                    return String::new();
                }
                let short = rel_in_str(target);
                if name == "browser_screenshot" {
                    let w = v.get("width").and_then(|x| x.as_u64());
                    let h = v.get("height").and_then(|x| x.as_u64());
                    if let (Some(w), Some(h)) = (w, h) {
                        return format!("{short} · {w}x{h}");
                    }
                }
                return short;
            }
            "dev_serve" => {
                let dir = v.get("dir").and_then(|x| x.as_str()).unwrap_or(".");
                let short = rel_in_str(dir);
                if let Some(p) = v.get("port").and_then(|x| x.as_u64()) {
                    if p != 0 {
                        return format!("{short} :{p}");
                    }
                }
                return short;
            }
            "netlist_run"
            | "netlist_validate"
            | "schematic_render"
            | "schematic_validate"
            | "schematic_query"
            | "schematic_netlist"
            | "schematic_bom"
            | "netlist_compare"
            | "netlist_to_schematic"
            | "pcb_render"
            | "pcb_query"
            | "pcb_validate"
            | "pcb_netlist"
            | "pcb_sync"
            | "pcb_export"
            | "pcb_autoroute"
            | "pcb_cleanup"
            | "pcb_compose"
            | "pcb_init"
            | "erc"
            | "drc"
            | "autofix"
            | "raw_info"
            | "raw_stats"
            | "raw_export"
            | "symbol_validate"
            | "footprint_import" => {
                let file = v
                    .get("file")
                    .or_else(|| v.get("path"))
                    .or_else(|| v.get("schematic"))
                    .or_else(|| v.get("netlist"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let short = if file.is_empty() {
                    String::new()
                } else {
                    rel_in_str(file)
                };
                let mut flags: Vec<String> = Vec::new();
                for (k, label) in [
                    ("analysis", ""),
                    ("measure", "measure"),
                    ("assert", "assert"),
                    ("export_raw", "export"),
                    ("format", ""),
                    ("auto_route", "auto-route"),
                    ("ripup", "ripup"),
                ] {
                    if let Some(val) = v.get(k) {
                        if val.as_bool() == Some(true) {
                            flags.push(label.to_string());
                        } else if let Some(s) = val.as_str() {
                            if !s.is_empty() {
                                if label.is_empty() {
                                    flags.push(s.to_string());
                                } else {
                                    flags.push(format!("{label}={s}"));
                                }
                            }
                        } else if val.is_array() {
                            flags.push(format!(
                                "{label}×{}",
                                val.as_array().map(|a| a.len()).unwrap_or(0)
                            ));
                        }
                    }
                }
                if short.is_empty() && flags.is_empty() {
                    return String::new();
                }
                if flags.is_empty() {
                    return short;
                }
                if short.is_empty() {
                    return flags.join(" ");
                }
                let flag_str = flags.join(" ");
                let cap = if wide { 600 } else { 160 };
                let combined = format!("{short} {flag_str}");
                if combined.chars().count() > cap {
                    return format!("{}…", truncate_chars(&combined, cap));
                }
                return combined;
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

pub(crate) fn pretty_tool_result_wide(content: &str, wide: bool) -> String {
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

pub(crate) fn scan_json_str(hay: &str, key: &str) -> Option<String> {
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

pub(crate) fn scan_json_num(hay: &str, key: &str) -> Option<u64> {
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

pub(crate) fn diff_card_lines(diff: &str, max: usize) -> (Vec<(bool, String)>, usize) {
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

pub(crate) fn result_diff_text(content: &str) -> Option<String> {
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

pub(crate) fn salvage_broken_result(content: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn short_tool_ids_keep_last_six() {
        assert_eq!(short_tool_id("call_abc123def456"), "def456");
        assert_eq!(short_tool_id("toolu_xyz789"), "xyz789");
        assert_eq!(short_tool_id("c1"), "c1");
        assert_eq!(short_tool_id("call_bash_1"), "bash1");
        assert_eq!(short_tool_id(""), "");
    }

    #[test]
    fn browser_cards_summarize_target() {
        let shot =
            serde_json::json!({"target": "https://example.com/", "width": 1280, "height": 800})
                .to_string();
        assert_eq!(
            pretty_tool_args("browser_screenshot", &shot),
            "https://example.com/ · 1280x800"
        );
        let dom = serde_json::json!({"target": "https://example.com/"}).to_string();
        assert_eq!(
            pretty_tool_args("browser_dom", &dom),
            "https://example.com/"
        );
        let serve = serde_json::json!({"dir": ".", "port": 8000}).to_string();
        assert_eq!(pretty_tool_args("dev_serve", &serve), ". :8000");
        let serve_auto = serde_json::json!({"dir": "public"}).to_string();
        assert_eq!(pretty_tool_args("dev_serve", &serve_auto), "public");
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
}
