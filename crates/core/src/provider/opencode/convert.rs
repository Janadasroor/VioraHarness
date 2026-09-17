// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::super::ChatRequest;
use serde_json::{json, Value};

pub(crate) fn chat_to_responses_body(req: &ChatRequest, bare_model: &str) -> Value {
    let mut input: Vec<Value> = Vec::new();
    let mut instructions: Option<String> = None;

    for m in &req.messages {
        let content_str = match &m.content {
            Value::String(s) => s.clone(),
            other => {
                if let Some(arr) = other.as_array() {
                    arr.iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    other.to_string()
                }
            }
        };
        match m.role.as_str() {
            "system" => {
                if instructions.is_none() {
                    instructions = Some(content_str);
                } else {
                    input.push(json!({"role":"system","content": content_str}));
                }
            }
            "user" => {
                if m.content.is_array() {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(arr) = m.content.as_array() {
                        for p in arr {
                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                parts.push(json!({"type":"input_text","text":t}));
                            } else if p.get("type").and_then(|v| v.as_str()) == Some("image_url") {
                                if let Some(url) = p
                                    .get("image_url")
                                    .and_then(|v| v.get("url"))
                                    .and_then(|v| v.as_str())
                                {
                                    parts.push(json!({"type":"input_image","image_url": url}));
                                }
                            }
                        }
                    }
                    if parts.is_empty() {
                        input.push(json!({"role":"user","content":[{"type":"input_text","text": content_str}]}));
                    } else {
                        input.push(json!({"role":"user","content": parts}));
                    }
                } else {
                    input.push(json!({"role":"user","content":[{"type":"input_text","text": content_str}]}));
                }
            }
            "assistant" => {
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        input.push(json!({
                            "type":"function_call",
                            "call_id": tc.id,
                            "name": tc.function.name,
                            "arguments": tc.function.arguments
                        }));
                    }
                    if !content_str.is_empty() {
                        input.push(json!({"role":"assistant","content":[{"type":"output_text","text": content_str}]}));
                    }
                } else if !content_str.is_empty() {
                    input.push(json!({"role":"assistant","content":[{"type":"output_text","text": content_str}]}));
                }
            }
            "tool" => {
                let call_id = m.tool_call_id.clone().unwrap_or_else(|| "call_0".into());
                let output = if content_str.is_empty() {
                    "{}".into()
                } else {
                    content_str
                };
                input.push(json!({
                    "type":"function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            }
            _ => {
                input.push(
                    json!({"role":"user","content":[{"type":"input_text","text": content_str}]}),
                );
            }
        }
    }

    let level = crate::thinking::normalize_thinking_level(
        req.thinking_level.as_deref().unwrap_or("medium"),
    );
    let effort: &str = if level == "off" {
        "none"
    } else {
        level.as_str()
    };
    let mut body = json!({
        "model": bare_model,
        "input": input,
        "stream": true,
        "store": false,
        "parallel_tool_calls": true,
        "reasoning": {"effort": effort},
        "text": {"verbosity": "low"}
    });

    if !bare_model.to_lowercase().starts_with("grok") {
        body["reasoning"]["summary"] = json!("auto");
    }
    if let Some(instr) = instructions {
        body["instructions"] = json!(instr);
    }
    if let Some(tools) = &req.tools {
        let resp_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type":"function",
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters,
                    "strict": false
                })
            })
            .collect();
        body["tools"] = json!(resp_tools);
        body["tool_choice"] = json!("auto");
    }

    let max_out = req.max_tokens.unwrap_or(1024).max(16384);
    body["max_output_tokens"] = json!(max_out);
    if let Some(tmp) = req.temperature {
        body["temperature"] = json!(tmp);
    }
    body
}

pub(crate) fn chat_to_messages_body(req: &ChatRequest, bare_model: &str) -> Value {
    let mut system: Option<String> = None;
    let mut messages: Vec<Value> = Vec::new();

    for m in &req.messages {
        let content_str = match &m.content {
            Value::String(s) => s.clone(),
            other => {
                if let Some(arr) = other.as_array() {
                    arr.iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    other.to_string()
                }
            }
        };
        match m.role.as_str() {
            "system" => {
                if system.is_none() {
                    system = Some(content_str);
                } else {
                    system = Some(format!("{}\n{}", system.unwrap(), content_str));
                }
            }
            "user" => {
                if m.content.is_array() {
                    let mut parts: Vec<Value> = Vec::new();
                    if let Some(arr) = m.content.as_array() {
                        for p in arr {
                            if let Some(t) = p.get("text").and_then(|v| v.as_str()) {
                                parts.push(json!({"type":"text","text": t}));
                            } else if p.get("type").and_then(|v| v.as_str()) == Some("image_url") {
                                parts.push(json!({"type":"text","text": "[image]"}));
                            }
                        }
                    }
                    if parts.is_empty() {
                        messages.push(
                            json!({"role":"user","content":[{"type":"text","text": content_str}]}),
                        );
                    } else {
                        messages.push(json!({"role":"user","content": parts}));
                    }
                } else {
                    messages.push(
                        json!({"role":"user","content":[{"type":"text","text": content_str}]}),
                    );
                }
            }
            "assistant" => {
                if let Some(tcs) = &m.tool_calls {
                    let mut content: Vec<Value> = Vec::new();
                    if !content_str.is_empty() {
                        content.push(json!({"type":"text","text": content_str}));
                    }
                    for tc in tcs {
                        let args: Value =
                            serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                        content.push(json!({
                            "type":"tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": args
                        }));
                    }
                    messages.push(json!({"role":"assistant","content": content}));
                } else {
                    messages.push(
                        json!({"role":"assistant","content":[{"type":"text","text": content_str}]}),
                    );
                }
            }
            "tool" => {
                let tid = m.tool_call_id.clone().unwrap_or_else(|| "toolu_0".into());

                messages.push(json!({
                    "role":"user",
                    "content": [{
                        "type":"tool_result",
                        "tool_use_id": tid,
                        "content": content_str
                    }]
                }));
            }
            _ => {
                messages
                    .push(json!({"role":"user","content":[{"type":"text","text": content_str}]}));
            }
        }
    }

    let level = crate::thinking::normalize_thinking_level(
        req.thinking_level.as_deref().unwrap_or("medium"),
    );
    // Extended thinking needs headroom the 1024-token turn default
    // cannot give (budget must sit below the output limit), so the
    // ceiling grows with the budget — same idea as the Responses
    // 16k floor. `off` keeps the exact old shape.
    let mut max_out = req.max_tokens.unwrap_or(1024);
    let mut thinking: Option<Value> = None;
    if level != "off" {
        if let Some(budget) = crate::thinking::thinking_budget(max_out.max(4096), &level) {
            max_out = max_out.max(budget + 1024);
            thinking = Some(json!({"type": "enabled", "budget_tokens": budget}));
        }
    }
    let mut body = json!({
        "model": bare_model,
        "messages": messages,
        "stream": true,
        "max_tokens": max_out
    });
    if let Some(t) = thinking {
        body["thinking"] = t;
    }
    if let Some(s) = system {
        body["system"] = json!(s);
    }
    if let Some(tools) = &req.tools {
        let anth_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "input_schema": t.function.parameters
                })
            })
            .collect();
        body["tools"] = json!(anth_tools);
        if req.tool_choice.is_some() {
            body["tool_choice"] = json!({"type":"auto"});
        }
    }
    if let Some(tmp) = req.temperature {
        body["temperature"] = json!(tmp);
    }
    body
}
