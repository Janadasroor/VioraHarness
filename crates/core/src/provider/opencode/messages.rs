use super::super::{ChatRequest, Provider, ProviderEvent};
use super::convert::chat_to_messages_body;
use super::routing::send_with_retry;
use super::*;
use futures::StreamExt;
use serde_json::Value;

impl OpenCodeProvider {
    pub(crate) async fn stream_messages(
        &self,
        req: ChatRequest,
        bare_model: String,
        key: String,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        let url = format!("{}/messages", self.base_url);
        let body = chat_to_messages_body(&req, &bare_model);

        let (tx, rx) = tokio::sync::mpsc::channel(128);

        let resp = match send_with_retry(
            || {
                self.client
                    .post(&url)
                    .header("x-api-key", &key)
                    .header("Authorization", format!("Bearer {}", key))
                    .header("anthropic-version", "2023-06-01")
                    .header(
                        "anthropic-beta",
                        "interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14",
                    )
                    .header("HTTP-Referer", "https://github.com/Janadasroor/VioraEDA")
                    .header("X-Title", "VioraHarness")
                    .header("x-opencode-session", Self::session_header())
                    .header("Content-Type", "application/json")
                    .header("Accept", "text/event-stream")
                    .json(&body)
            },
            "messages",
            Some(&tx),
        )
        .await
        {
            Ok(r) => r,
            Err((status, detail, attempts, waited)) => {
                let code = status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "transport".into());
                let human = super::super::humanize_error_body(&detail, 400);
                if status == Some(401) || detail.contains("Invalid") || detail.contains("Auth") {
                    anyhow::bail!("Gateway {} messages error {code}: {human}\nHint: check OPENCODE_API_KEY at https://opencode.ai/auth", self.name());
                }
                if status == Some(429)
                    || detail.to_lowercase().contains("rate")
                    || detail.to_lowercase().contains("limit")
                {
                    anyhow::bail!("Gateway {} messages error {code}: {human} (after {attempts} attempts, waited {waited}s)\nHint: rate/limit still hit — try a different model or wait a minute", self.name());
                }
                anyhow::bail!(
                    "Gateway {} messages error {code}: {} (after {attempts} attempts)",
                    self.name(),
                    super::super::humanize_error_body(&detail, 800)
                );
            }
        };

        let mut stream = resp.bytes_stream();

        tokio::spawn(async move {
            let mut buf = String::new();
            let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();
            let mut current_block_idx: usize = 0;
            let mut current_tool_name: String = String::new();
            let mut current_tool_id: String = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("gateway messages stream error: {e}");
                        break;
                    }
                };
                let text = String::from_utf8_lossy(&bytes);
                buf.push_str(&text);

                if buf.len() > super::super::MAX_SSE_BUF {
                    tracing::error!("SSE buffer exceeded cap, aborting stream");
                    break;
                }

                while let Some(nl) = buf.find('\n') {
                    let line = buf[..nl].trim().to_string();
                    buf = buf[nl + 1..].to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let data = if line.starts_with("data: ") {
                        line.trim_start_matches("data: ").trim()
                    } else if line.starts_with("event: ") {
                        continue;
                    } else {
                        line.as_str()
                    };
                    if data == "[DONE]" {
                        let _ = tx.send(ProviderEvent::Done).await;
                        break;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match typ {
                            "content_block_start" => {
                                if let Some(cb) = v.get("content_block") {
                                    if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                        let name = cb
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let id = cb
                                            .get("id")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        current_tool_name = name.clone();
                                        current_tool_id = id.clone();
                                        current_block_idx =
                                            v.get("index").and_then(|i| i.as_u64()).unwrap_or(0)
                                                as usize;
                                        pending_tool_calls
                                            .entry(current_block_idx)
                                            .or_insert_with(|| (id, name, String::new()));
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = v.get("delta") {
                                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                                        let _ = tx
                                            .send(ProviderEvent::TextDelta(text.to_string()))
                                            .await;
                                    }
                                    if let Some(partial) =
                                        delta.get("partial_json").and_then(|p| p.as_str())
                                    {
                                        let entry = pending_tool_calls
                                            .entry(current_block_idx)
                                            .or_insert_with(|| {
                                                (
                                                    current_tool_id.clone(),
                                                    current_tool_name.clone(),
                                                    String::new(),
                                                )
                                            });
                                        entry.2.push_str(partial);
                                    }
                                    if let Some(thinking) =
                                        delta.get("thinking").and_then(|t| t.as_str())
                                    {
                                        let _ = tx
                                            .send(ProviderEvent::ReasoningDelta(
                                                thinking.to_string(),
                                            ))
                                            .await;
                                    }
                                }
                            }
                            "content_block_stop" => {
                                let idx = v
                                    .get("index")
                                    .and_then(|i| i.as_u64())
                                    .unwrap_or(current_block_idx as u64)
                                    as usize;
                                if let Some((id, name, args)) = pending_tool_calls.remove(&idx) {
                                    if !name.is_empty() {
                                        let args_valid =
                                            if serde_json::from_str::<Value>(&args).is_ok() {
                                                args
                                            } else if args.trim().is_empty() {
                                                "{}".into()
                                            } else {
                                                args
                                            };
                                        let final_id = if id.is_empty() {
                                            format!("call_{idx}")
                                        } else {
                                            id
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
                            "message_delta" => {
                                if let Some(stop) = v
                                    .get("delta")
                                    .and_then(|d| d.get("stop_reason"))
                                    .and_then(|s| s.as_str())
                                {
                                    if stop == "tool_use" {
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
                            "message_stop" => {
                                for (idx, (id, name, args)) in pending_tool_calls.drain() {
                                    if name.is_empty() {
                                        continue;
                                    }
                                    let final_id = if id.is_empty() {
                                        format!("call_{idx}")
                                    } else {
                                        id
                                    };
                                    let args_valid = if serde_json::from_str::<Value>(&args).is_ok()
                                    {
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
                            _ => {
                                if let Some(content) = v
                                    .get("delta")
                                    .and_then(|d| d.get("text"))
                                    .and_then(|t| t.as_str())
                                {
                                    let _ = tx
                                        .send(ProviderEvent::TextDelta(content.to_string()))
                                        .await;
                                }
                            }
                        }
                    }
                }
            }
            for (idx, (id, name, args)) in pending_tool_calls {
                if name.is_empty() {
                    continue;
                }
                let final_id = if id.is_empty() {
                    format!("call_{idx}")
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
