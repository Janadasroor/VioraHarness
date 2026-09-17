// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::*;

pub fn config_candidates() -> Vec<String> {
    let mut cands = vec![
        "vioraharness.json".to_string(),
        ".vioraharness/vioraharness.json".to_string(),
    ];
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("vioraharness.json").to_string_lossy().to_string());

        let mut cur = cwd.clone();
        loop {
            let cand = cur.join("vioraharness.json");
            let s = cand.to_string_lossy().to_string();
            if !cands.contains(&s) {
                cands.push(s);
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
            if cur.to_string_lossy().len() < 2 {
                break;
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        cands.push(format!("{home}/.config/vioraharness/vioraharness.json"));
    }
    if let Ok(p) = std::env::var("VIORAHARNESS_CONFIG") {
        if !p.trim().is_empty() {
            cands.insert(0, p);
        }
    }
    cands
}

pub(crate) fn load_rules_from_config() -> Vec<Rule> {
    for cand in config_candidates() {
        if let Ok(content) = std::fs::read_to_string(cand) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                if let Some(perms) = val.get("permissions") {
                    return rules_from_json(perms);
                }
            }
        }
    }

    vec![
        Rule::new(Decision::Allow, "read"),
        Rule::new(Decision::Allow, "glob"),
        Rule::new(Decision::Allow, "grep"),
        Rule::new(Decision::Allow, "schematic_query"),
        Rule::new(Decision::Allow, "symbol_search"),
        Rule::new(Decision::Allow, "footprint_list"),
        Rule::new(Decision::Allow, "netlist_validate"),
        Rule::new(Decision::Allow, "schematic_render"),
        Rule::new(Decision::Allow, "pcb_render"),
        Rule::new(Decision::Allow, "task"),
        Rule::new(Decision::Allow, "skill"),
        Rule::new(Decision::Ask, "write"),
        Rule::new(Decision::Allow, "bash"),
        Rule::new(Decision::Deny, "bash rm -rf*"),
        Rule::new(Decision::Deny, "bash sudo:*"),
    ]
}

pub(crate) fn is_unproductive_call(name: &str, result: &Value) -> bool {
    if result.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return true;
    }
    match name {
        "bash" => result.get("bytes").and_then(|v| v.as_u64()) == Some(0),
        "grep" => result
            .get("hits")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "glob" => result
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn sanitize_tool_contiguity(messages: &mut Vec<ChatMessage>) {
    use std::collections::HashSet;
    let mut declared: HashSet<String> = HashSet::new();
    for m in messages.iter() {
        if m.role == "assistant" {
            if let Some(tcs) = &m.tool_calls {
                for t in tcs {
                    declared.insert(t.id.clone());
                }
            }
        }
    }
    let mut answered: HashSet<String> = HashSet::new();
    for m in messages.iter() {
        if m.role == "tool" {
            if let Some(id) = &m.tool_call_id {
                if declared.contains(id) {
                    answered.insert(id.clone());
                }
            }
        }
    }
    messages.retain(|m| {
        if m.role != "tool" {
            return true;
        }
        match &m.tool_call_id {
            Some(id) => declared.contains(id),
            None => false,
        }
    });
    for m in messages.iter_mut() {
        if m.role == "assistant" {
            if let Some(tcs) = m.tool_calls.as_mut() {
                tcs.retain(|t| answered.contains(&t.id));
                if tcs.is_empty() {
                    m.tool_calls = None;
                }
            }
        }
    }
}

pub(crate) struct ApprovalGuard {
    prev: Option<String>,
}

impl ApprovalGuard {
    pub(crate) fn hold() -> Self {
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
        Self { prev }
    }
}

impl Drop for ApprovalGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }
}

pub(crate) fn auto_allow_on() -> bool {
    std::env::var("VIORAHARNESS_AUTO_ALLOW")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

pub(crate) async fn execute_approved(name: &str, args: Value) -> Value {
    let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
    std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
    let out = tools::execute_tool(name, args).await;
    match prev {
        Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
        None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
    }
    out
}

pub(crate) fn normalize_tool_args(args: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(args).is_ok() {
        args.to_string()
    } else {
        tracing::warn!("normalizing invalid tool-call args to {{}}: {args}");
        "{}".into()
    }
}

pub(crate) fn loop_guard_notes(
    name: &str,
    repeats: usize,
    unproductive_streak: usize,
) -> Option<String> {
    if repeats >= 3 {
        return Some(format!(
            "[harness: you already ran this exact `{name}` call {repeats} times — do NOT run it again; answer from what you have or try a different approach]"
        ));
    }
    if unproductive_streak >= 4 {
        return Some(format!(
            "[harness: {unproductive_streak} consecutive unproductive tool calls (empty results / errors) — stop circling; answer the user with what you already found]"
        ));
    }
    None
}

pub(crate) fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "glob"
            | "grep"
            | "schematic_query"
            | "pcb_query"
            | "symbol_search"
            | "footprint_list"
            | "netlist_validate"
            | "netlist_run"
            | "netlist_to_schematic"
            | "erc"
            | "pcb_validate"
            | "raw_export"
            | "viora"
            | "task"
            | "skill"
            | "question"
            | "todowrite"
            | "webfetch"
            | "websearch"
            | "schematic_render"
            | "pcb_render"
    )
}

pub(crate) fn persist_allow_always(tool: &str, args_str: &str) {
    let pattern = if tool == "bash" {
        let cmd = if args_str.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(args_str) {
                v.get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        } else {
            args_str.to_string()
        };

        let first = cmd
            .trim()
            .split([' ', '|', '&', ';'])
            .next()
            .unwrap_or(tool)
            .trim()
            .trim_matches(['"', '\'', '`']);
        let base = first.split('/').next_back().unwrap_or(first);
        if base.is_empty() {
            "bash".to_string()
        } else {
            format!("bash {base}*")
        }
    } else if tool == "write" {
        "write".to_string()
    } else {
        tool.to_string()
    };
    for cand in config_candidates() {
        if let Ok(content) = std::fs::read_to_string(&cand) {
            if let Ok(mut val) = serde_json::from_str::<Value>(&content) {
                if let Some(obj) = val.get_mut("permissions").and_then(|p| p.as_object_mut()) {
                    obj.insert(pattern.clone(), Value::String("allow".into()));
                    if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                        let _ = std::fs::write(&cand, pretty);
                        tracing::info!("persist_allow_always: {} -> allow in {}", pattern, cand);
                    }
                    break;
                }
            }
        }
    }
}

pub(crate) fn simple_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:010x}", nanos & 0xffffffffff)
}
