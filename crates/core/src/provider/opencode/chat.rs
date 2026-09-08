use super::super::{ChatRequest, Provider, ProviderEvent};
use super::routing::send_with_retry;
use super::*;
use futures::StreamExt;
use serde_json::Value;

impl OpenCodeProvider {
    pub(crate) async fn stream_chat(
        &self,
        req: ChatRequest,
        bare_model: String,
        key: String,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut body = serde_json::to_value(&req)?;
        if let Value::Object(ref mut map) = body {
            map.insert("model".into(), Value::String(bare_model.clone()));
            map.insert("stream".into(), Value::Bool(true));
            map.insert("include_reasoning".into(), Value::Bool(true));
            if !map.contains_key("max_tokens") {
                map.insert(
                    "max_tokens".into(),
                    Value::Number(serde_json::Number::from(1024)),
                );
            }
        }

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
                    .json(&body)
            },
            "chat",
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
                    anyhow::bail!("Gateway {} error {code}: {detail}\nHint: check OPENCODE_API_KEY at https://opencode.ai/auth — free-tier models can use key=public but need to be enabled, or set OPENCODE_API_KEY", self.name());
                }
                if status == Some(429)
                    || detail.to_lowercase().contains("rate")
                    || detail.to_lowercase().contains("limit")
                {
                    anyhow::bail!("Gateway {} error {code}: {} (after {attempts} attempts, waited {waited}s)\nHint: rate/limit still hit — try a different model or wait a minute", self.name(), detail.chars().take(400).collect::<String>());
                }
                anyhow::bail!(
                    "Gateway {} error {code}: {} (after {attempts} attempts)",
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

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("gateway stream error: {e}");
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
                    if line.starts_with("data: ") {
                        let data = line.trim_start_matches("data: ").trim();
                        if data == "[DONE]" {
                            let _ = tx.send(ProviderEvent::Done).await;
                            break;
                        }
                        if let Ok(v) = serde_json::from_str::<Value>(data) {
                            if tracing::enabled!(tracing::Level::DEBUG)
                                && v.get("choices").is_some()
                            {
                                tracing::debug!("gateway delta raw: {data}");
                            }
                            if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                                for choice in choices {
                                    let delta = choice.get("delta");
                                    if let Some(delta) = delta {
                                        if let Some(reasoning) = delta
                                            .get("reasoning")
                                            .and_then(|r| r.as_str())
                                            .or_else(|| {
                                                delta.get("reasoning_details").and_then(|r| {
                                                    r.as_array().and_then(|a| {
                                                        a.first().and_then(|o| {
                                                            o.get("text").and_then(|t| t.as_str())
                                                        })
                                                    })
                                                })
                                            })
                                        {
                                            let _ = tx
                                                .send(ProviderEvent::ReasoningDelta(
                                                    reasoning.to_string(),
                                                ))
                                                .await;
                                        }
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
                                        if let Some(tool_calls) =
                                            delta.get("tool_calls").and_then(|t| t.as_array())
                                        {
                                            for tc in tool_calls {
                                                let idx = tc
                                                    .get("index")
                                                    .and_then(|i| i.as_u64())
                                                    .unwrap_or(0)
                                                    as usize;
                                                let id = tc
                                                    .get("id")
                                                    .and_then(|s| s.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                let func = tc.get("function");
                                                let name = func
                                                    .and_then(|f| f.get("name"))
                                                    .and_then(|n| n.as_str())
                                                    .unwrap_or("")
                                                    .to_string();
                                                let args = func
                                                    .and_then(|f| f.get("arguments"))
                                                    .map(|a| {
                                                        if a.is_string() {
                                                            a.as_str().unwrap_or("").to_string()
                                                        } else {
                                                            a.to_string()
                                                        }
                                                    })
                                                    .unwrap_or_default();
                                                let entry = pending_tool_calls
                                                    .entry(idx)
                                                    .or_insert_with(|| {
                                                        (
                                                            String::new(),
                                                            String::new(),
                                                            String::new(),
                                                        )
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
                                    if let Some(fr) =
                                        choice.get("finish_reason").and_then(|r| r.as_str())
                                    {
                                        if fr == "tool_calls" {
                                            for (idx, (id, name, args)) in
                                                pending_tool_calls.drain()
                                            {
                                                if name.is_empty() {
                                                    tracing::warn!(
                                                        "gateway pending idx {idx} empty name skip"
                                                    );
                                                    continue;
                                                }
                                                let args_valid = if serde_json::from_str::<Value>(
                                                    &args,
                                                )
                                                .is_ok()
                                                {
                                                    args
                                                } else if args.trim().is_empty() {
                                                    "{}".into()
                                                } else {
                                                    tracing::warn!("gateway invalid JSON args for {name}: {args}");
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
                                                        name: name.clone(),
                                                        args: args_valid,
                                                        thought_signature: None,
                                                    })
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(err) = v.get("error") {
                                tracing::error!("Gateway error payload: {err}");
                            }
                        }
                    }
                }
            }
            for (idx, (id, name, args)) in pending_tool_calls {
                if name.is_empty() {
                    continue;
                }
                let args_valid = if serde_json::from_str::<Value>(&args).is_ok() {
                    args.clone()
                } else if args.trim().is_empty() {
                    "{}".into()
                } else {
                    tracing::warn!("gateway flush fallback invalid JSON for {name}: {args}");
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
            let _ = tx.send(ProviderEvent::Done).await;
        });

        Ok(rx)
    }
}
