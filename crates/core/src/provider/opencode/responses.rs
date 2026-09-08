use super::super::{ChatRequest, Provider, ProviderEvent};
use super::convert::chat_to_responses_body;
use super::routing::{coalesce_pending, send_with_retry};
use super::*;
use futures::StreamExt;
use serde_json::Value;

impl OpenCodeProvider {
    pub(crate) async fn stream_responses(
        &self,
        req: ChatRequest,
        bare_model: String,
        key: String,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        let url = format!("{}/responses", self.base_url);
        let body = chat_to_responses_body(&req, &bare_model);

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let resp = match send_with_retry(
            || {
                self.client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", key))
                    .header("HTTP-Referer", "https://github.com/Janadasroor/VioraEDA")
                    .header("X-Title", "VioraHarness")
                    .header("x-opencode-session", Self::session_header())
                    .header("Content-Type", "application/json")
                    .header("Accept", "text/event-stream")
                    .json(&body)
            },
            "responses",
            Some(&tx),
        )
        .await
        {
            Ok(r) => r,
            Err((status, detail, attempts, waited)) => {
                let code = status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "transport".into());
                if status == Some(401)
                    || detail.contains("Invalid API key")
                    || detail.contains("AuthError")
                {
                    anyhow::bail!("Gateway {} responses error {code}: {detail}\nHint: check OPENCODE_API_KEY at https://opencode.ai/auth", self.name());
                }
                if detail.to_lowercase().contains("rate") || detail.to_lowercase().contains("limit")
                {
                    anyhow::bail!("Gateway {} responses error {code}: {} (after {attempts} attempts, waited {waited}s)\nHint: rate/limit still hit — try a different model or wait a minute", self.name(), detail.chars().take(400).collect::<String>());
                }
                anyhow::bail!(
                    "Gateway {} responses error {code}: {} (after {attempts} attempts)",
                    self.name(),
                    detail.chars().take(800).collect::<String>()
                );
            }
        };

        let mut stream = resp.bytes_stream();

        tokio::spawn(async move {
            let mut buf = String::new();
            let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();

            let mut saw_text = false;
            let mut saw_reasoning = false;

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("gateway responses stream error: {e}");
                        break;
                    }
                };
                let text = String::from_utf8_lossy(&bytes);
                buf.push_str(&text);

                if buf.len() > super::super::MAX_SSE_BUF {
                    tracing::error!("SSE buffer exceeded cap, aborting stream");
                    break;
                }

                while let Some(delim) = buf.find("\n\n") {
                    let block = buf[..delim].trim().to_string();
                    buf = buf[delim + 2..].to_string();
                    if block.is_empty() {
                        continue;
                    }
                    let mut event = "";
                    let mut data = "";
                    for line in block.lines() {
                        let l = line.trim();
                        if l.starts_with("event: ") {
                            event = l.trim_start_matches("event: ").trim();
                        } else if l.starts_with("data: ") {
                            data = l.trim_start_matches("data: ").trim();
                        } else if l.starts_with("data:") {
                            data = l.trim_start_matches("data:").trim();
                        }
                    }

                    if data.is_empty() {
                        if let Some(idx) = block.find("data:") {
                            data = block[idx + 5..].trim();
                        } else {
                            continue;
                        }
                    }
                    if data == "[DONE]" {
                        let _ = tx.send(ProviderEvent::Done).await;
                        continue;
                    }

                    let v_opt = serde_json::from_str::<Value>(data).ok();
                    let Some(v) = v_opt else {
                        if !data.is_empty() && data != "[DONE]" {
                            let _ = tx.send(ProviderEvent::TextDelta(data.to_string())).await;
                        }
                        continue;
                    };

                    match event {
                        "response.reasoning_summary_text.delta" => {
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                if !delta.is_empty() {
                                    saw_reasoning = true;
                                    let _ = tx
                                        .send(ProviderEvent::ReasoningDelta(delta.to_string()))
                                        .await;
                                }
                            }
                            continue;
                        }
                        "response.output_text.delta" => {
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                saw_text = true;
                                let _ = tx.send(ProviderEvent::TextDelta(delta.to_string())).await;
                            }
                            continue;
                        }
                        "response.output_item.added" => {
                            if let Some(item) = v.get("item") {
                                if item.get("type").and_then(|t| t.as_str())
                                    == Some("function_call")
                                {
                                    let idx = v
                                        .get("output_index")
                                        .and_then(|i| i.as_u64())
                                        .unwrap_or_else(|| {
                                            item.get("output_index")
                                                .and_then(|i| i.as_u64())
                                                .unwrap_or(0)
                                        }) as usize;

                                    let idx = item
                                        .get("output_index")
                                        .and_then(|i| i.as_u64())
                                        .map(|x| x as usize)
                                        .unwrap_or(idx);
                                    let name = item
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let id = item
                                        .get("id")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let call_id = item
                                        .get("call_id")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or(id.as_str())
                                        .to_string();
                                    let args = item
                                        .get("arguments")
                                        .and_then(|a| a.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let entry =
                                        pending_tool_calls.entry(idx).or_insert_with(|| {
                                            (String::new(), String::new(), String::new())
                                        });
                                    if !call_id.is_empty() {
                                        entry.0 = call_id;
                                    } else if !id.is_empty() {
                                        entry.0 = id;
                                    }
                                    if !name.is_empty() {
                                        entry.1 = name;
                                    }
                                    if !args.is_empty() {
                                        entry.2.push_str(&args);
                                    }
                                }
                            }
                            continue;
                        }
                        "response.function_call_arguments.delta" => {
                            let idx = v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0)
                                as usize;
                            if let Some(delta) = v
                                .get("delta")
                                .and_then(|d| d.as_str())
                                .or_else(|| v.get("arguments_delta").and_then(|d| d.as_str()))
                            {
                                let entry = pending_tool_calls.entry(idx).or_insert_with(|| {
                                    (String::new(), String::new(), String::new())
                                });
                                entry.2.push_str(delta);
                            }
                            continue;
                        }
                        "response.completed" => {
                            let mut seen_ids: std::collections::HashSet<String> =
                                std::collections::HashSet::new();
                            let mut seen_calls: std::collections::HashSet<(String, String)> =
                                std::collections::HashSet::new();
                            if let Some(output) = v
                                .get("response")
                                .and_then(|r| r.get("output"))
                                .and_then(|o| o.as_array())
                                .or_else(|| v.get("output").and_then(|o| o.as_array()))
                            {
                                for item in output {
                                    if item.get("type").and_then(|t| t.as_str())
                                        != Some("function_call")
                                    {
                                        continue;
                                    }
                                    let name = item
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if name.is_empty() {
                                        continue;
                                    }
                                    let id = item
                                        .get("call_id")
                                        .and_then(|c| c.as_str())
                                        .map(|s| s.to_string())
                                        .unwrap_or_default();
                                    let id2 = item
                                        .get("id")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let args = item
                                        .get("arguments")
                                        .and_then(|a| a.as_str())
                                        .unwrap_or("{}")
                                        .to_string();
                                    if !id.is_empty() {
                                        seen_ids.insert(id.clone());
                                    }
                                    if !id2.is_empty() {
                                        seen_ids.insert(id2.clone());
                                    }
                                    let final_id = if id.is_empty() {
                                        if id2.is_empty() {
                                            format!("call_{}", seen_ids.len())
                                        } else {
                                            id2
                                        }
                                    } else {
                                        id
                                    };
                                    seen_calls.insert((name.clone(), args.clone()));
                                    let _ = tx
                                        .send(ProviderEvent::ToolCallDelta {
                                            id: final_id,
                                            name,
                                            args,
                                            thought_signature: None,
                                        })
                                        .await;
                                }
                            }

                            let merged = coalesce_pending(
                                pending_tool_calls.drain().map(|(_, t)| t).collect(),
                            );
                            for (num, (id, name, args)) in merged.into_iter().enumerate() {
                                if name.is_empty() {
                                    continue;
                                }

                                if seen_ids.contains(&id)
                                    || seen_calls.contains(&(name.clone(), args.clone()))
                                {
                                    continue;
                                }
                                let final_id = if id.is_empty() {
                                    format!("call_{num}")
                                } else {
                                    id
                                };
                                let args_valid = if serde_json::from_str::<Value>(&args).is_ok() {
                                    args
                                } else if args.trim().is_empty() {
                                    "{}".into()
                                } else {
                                    args
                                };
                                let _ = tx
                                    .send(ProviderEvent::ToolCallDelta {
                                        id: final_id,
                                        name,
                                        args: args_valid,
                                        thought_signature: None,
                                    })
                                    .await;
                            }

                            if !saw_text {
                                if let Some(output) = v
                                    .get("response")
                                    .and_then(|r| r.get("output"))
                                    .and_then(|o| o.as_array())
                                    .or_else(|| v.get("output").and_then(|o| o.as_array()))
                                {
                                    for item in output {
                                        if item.get("type").and_then(|t| t.as_str())
                                            == Some("message")
                                        {
                                            if let Some(content) =
                                                item.get("content").and_then(|c| c.as_array())
                                            {
                                                for part in content {
                                                    if let Some(text) =
                                                        part.get("text").and_then(|t| t.as_str())
                                                    {
                                                        let _ = tx
                                                            .send(ProviderEvent::TextDelta(
                                                                text.to_string(),
                                                            ))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            if !saw_reasoning {
                                if let Some(output) = v
                                    .get("response")
                                    .and_then(|r| r.get("output"))
                                    .and_then(|o| o.as_array())
                                    .or_else(|| v.get("output").and_then(|o| o.as_array()))
                                {
                                    for item in output {
                                        if item.get("type").and_then(|t| t.as_str())
                                            != Some("reasoning")
                                        {
                                            continue;
                                        }
                                        if let Some(summary) =
                                            item.get("summary").and_then(|s| s.as_array())
                                        {
                                            for part in summary {
                                                let is_text = part
                                                    .get("type")
                                                    .and_then(|t| t.as_str())
                                                    .map(|t| t == "summary_text" || t == "text")
                                                    .unwrap_or(true);
                                                if is_text {
                                                    if let Some(text) =
                                                        part.get("text").and_then(|t| t.as_str())
                                                    {
                                                        if !text.is_empty() {
                                                            let _ = tx
                                                                .send(
                                                                    ProviderEvent::ReasoningDelta(
                                                                        text.to_string(),
                                                                    ),
                                                                )
                                                                .await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            continue;
                        }
                        _ => {
                            if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                                let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                if (typ == "response.output_text.delta" || typ.is_empty())
                                    && v.get("choices").is_none()
                                {
                                    saw_text = true;
                                    let _ =
                                        tx.send(ProviderEvent::TextDelta(delta.to_string())).await;
                                    continue;
                                }
                                if typ == "response.function_call_arguments.delta" {
                                    let idx =
                                        v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0)
                                            as usize;
                                    let entry =
                                        pending_tool_calls.entry(idx).or_insert_with(|| {
                                            (String::new(), String::new(), String::new())
                                        });
                                    entry.2.push_str(delta);
                                    continue;
                                }
                            }

                            if let Some(item) = v.get("item") {
                                if item.get("type").and_then(|t| t.as_str())
                                    == Some("function_call")
                                {
                                    let idx =
                                        v.get("output_index").and_then(|i| i.as_u64()).unwrap_or(0)
                                            as usize;
                                    let name = item
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let id = item
                                        .get("id")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let call_id = item
                                        .get("call_id")
                                        .and_then(|c| c.as_str())
                                        .unwrap_or(id.as_str())
                                        .to_string();
                                    let args = item
                                        .get("arguments")
                                        .and_then(|a| a.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let entry =
                                        pending_tool_calls.entry(idx).or_insert_with(|| {
                                            (String::new(), String::new(), String::new())
                                        });
                                    if !call_id.is_empty() {
                                        entry.0 = call_id;
                                    }
                                    if !name.is_empty() {
                                        entry.1 = name;
                                    }
                                    entry.2.push_str(&args);
                                    continue;
                                }
                            }
                        }
                    }

                    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                        for choice in choices {
                            if let Some(delta) = choice.get("delta") {
                                if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                                {
                                    if !content.is_empty() {
                                        let _ = tx
                                            .send(ProviderEvent::TextDelta(content.to_string()))
                                            .await;
                                    }
                                }
                                if let Some(reasoning) =
                                    delta.get("reasoning").and_then(|r| r.as_str())
                                {
                                    let _ = tx
                                        .send(ProviderEvent::ReasoningDelta(reasoning.to_string()))
                                        .await;
                                }
                                if let Some(tool_calls) =
                                    delta.get("tool_calls").and_then(|t| t.as_array())
                                {
                                    for tc in tool_calls {
                                        let idx =
                                            tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0)
                                                as usize;
                                        let id = tc
                                            .get("id")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = tc
                                            .get("function")
                                            .and_then(|f| f.get("name"))
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let args = tc
                                            .get("function")
                                            .and_then(|f| f.get("arguments"))
                                            .map(|a| {
                                                if a.is_string() {
                                                    a.as_str().unwrap_or("").to_string()
                                                } else {
                                                    a.to_string()
                                                }
                                            })
                                            .unwrap_or_default();
                                        let entry =
                                            pending_tool_calls.entry(idx).or_insert_with(|| {
                                                (String::new(), String::new(), String::new())
                                            });
                                        if !id.is_empty() {
                                            entry.0 = id;
                                        }
                                        if !name.is_empty() {
                                            entry.1 = name;
                                        }
                                        entry.2.push_str(&args);
                                    }
                                }
                            }
                            if let Some(fr) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                                if fr == "tool_calls" {
                                    for (idx, (id, name, args)) in pending_tool_calls.drain() {
                                        if name.is_empty() {
                                            continue;
                                        }
                                        let final_id = if id.is_empty() {
                                            format!("call_{idx}")
                                        } else {
                                            id
                                        };
                                        let args_valid =
                                            if serde_json::from_str::<Value>(&args).is_ok() {
                                                args
                                            } else if args.trim().is_empty() {
                                                "{}".into()
                                            } else {
                                                args
                                            };
                                        let _ = tx
                                            .send(ProviderEvent::ToolCallDelta {
                                                id: final_id,
                                                name,
                                                args: args_valid,
                                                thought_signature: None,
                                            })
                                            .await;
                                    }
                                }
                            }
                        }
                    }

                    if let Some(delta) = v.get("delta").and_then(|d| d.as_str()) {
                        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if (typ == "response.output_text.delta"
                            || (event.is_empty() && !delta.is_empty()))
                            && event.is_empty()
                        {
                            saw_text = true;
                            let _ = tx.send(ProviderEvent::TextDelta(delta.to_string())).await;
                        }
                    }
                }
            }

            if buf.contains("data:") {
                let lines: Vec<String> = buf.lines().map(|l| l.trim().to_string()).collect();
                for line in lines {
                    if line.starts_with("data: ") {
                        let data = line.trim_start_matches("data: ").trim();
                        if data == "[DONE]" {
                            let _ = tx.send(ProviderEvent::Done).await;
                            continue;
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(data) {
                            if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                                for choice in choices {
                                    if let Some(delta) = choice.get("delta") {
                                        if let Some(content) =
                                            delta.get("content").and_then(|c| c.as_str())
                                        {
                                            if !content.is_empty() {
                                                let _ = tx
                                                    .send(ProviderEvent::TextDelta(
                                                        content.to_string(),
                                                    ))
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                buf.clear();
            }

            let merged: Vec<(String, String, String)> =
                coalesce_pending(pending_tool_calls.into_values().collect());
            for (num, (id, name, args)) in merged.into_iter().enumerate() {
                if name.is_empty() {
                    continue;
                }
                let final_id = if id.is_empty() {
                    format!("call_{num}")
                } else {
                    id
                };
                let args_valid = if serde_json::from_str::<Value>(&args).is_ok() {
                    args
                } else if args.trim().is_empty() {
                    "{}".into()
                } else {
                    args
                };
                let _ = tx
                    .send(ProviderEvent::ToolCallDelta {
                        id: final_id,
                        name,
                        args: args_valid,
                        thought_signature: None,
                    })
                    .await;
            }
            let _ = tx.send(ProviderEvent::Done).await;
        });

        Ok(rx)
    }
}
