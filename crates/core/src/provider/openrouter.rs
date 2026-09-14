use super::{ChatRequest, Provider, ProviderEvent};
use futures::StreamExt;
use reqwest::Client;
use serde_json::Value;

fn parse_afford_limit(text: &str) -> Option<u32> {
    let lower = text.to_lowercase();
    if let Some(idx) = lower.find("can only afford") {
        let after = &lower[idx + "can only afford".len()..];

        let mut num = String::new();
        for c in after.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else if !num.is_empty() {
                break;
            }
        }
        if let Ok(n) = num.parse::<u32>() {
            return Some(n);
        }
    }
    None
}

pub struct OpenRouterProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl OpenRouterProvider {
    pub fn from_env() -> Self {
        let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        let base_url = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".into());
        Self {
            api_key,
            base_url,
            client: super::http_client(),
        }
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: super::http_client(),
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenRouterProvider {
    fn name(&self) -> &str {
        "openrouter"
    }

    async fn stream(
        &self,
        req: ChatRequest,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "OPENROUTER_API_KEY not set (model '{}').\nFix: export OPENROUTER_API_KEY=... or use a Gemini model with GEMINI_API_KEY.\nAvailable models: /model to list, or run `cargo run -- doctor` to check keys.",
                req.model
            );
        }

        let url = format!("{}/chat/completions", self.base_url);
        let mut body = serde_json::to_value(&req)?;
        if let Value::Object(ref mut map) = body {
            map.insert("stream".into(), Value::Bool(true));
            map.insert("include_reasoning".into(), Value::Bool(true));
            let level = crate::thinking::normalize_thinking_level(
                req.thinking_level.as_deref().unwrap_or("medium"),
            );
            let effort: &str = if level == "off" {
                "none"
            } else {
                level.as_str()
            };
            map.insert("reasoning".into(), serde_json::json!({"effort": effort}));

            if !map.contains_key("max_tokens") {
                map.insert(
                    "max_tokens".into(),
                    Value::Number(serde_json::Number::from(1024)),
                );
            }
        }

        let mut resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://github.com/Janadasroor/VioraEDA")
            .header("X-Title", "VioraHarness")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if resp.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
            let text = resp.text().await.unwrap_or_default();

            if let Some(afford) = parse_afford_limit(&text) {
                let retry_tokens = afford.clamp(128, 512);
                tracing::warn!(
                    "OpenRouter 402 afford {afford}, retrying with max_tokens={retry_tokens}"
                );
                if let Some(obj) = body.as_object_mut() {
                    obj.insert(
                        "max_tokens".into(),
                        Value::Number(serde_json::Number::from(retry_tokens)),
                    );
                }
                resp = self
                    .client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("HTTP-Referer", "https://github.com/Janadasroor/VioraEDA")
                    .header("X-Title", "VioraHarness")
                    .header("Content-Type", "application/json")
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let text2 = resp.text().await.unwrap_or_default();
                    anyhow::bail!("OpenRouter error {status} (after retry max_tokens={retry_tokens}): {text2}\nHint: add credits at https://openrouter.ai/settings/credits or pick a cheaper model via /model");
                }
            } else {
                anyhow::bail!("OpenRouter error 402 Payment Required: {text}\nHint: add credits at https://openrouter.ai/settings/credits (current afford limit may be very low)");
            }
        } else if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter error {status}: {text}");
        }

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let mut stream = resp.bytes_stream();

        tokio::spawn(async move {
            let mut buf = String::new();

            let mut pending_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
                std::collections::HashMap::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("stream error: {e}");
                        break;
                    }
                };
                let text = String::from_utf8_lossy(&bytes);
                buf.push_str(&text);

                if buf.len() > super::MAX_SSE_BUF {
                    tracing::error!("SSE buffer exceeded cap, aborting stream");
                    break;
                }

                while let Some(newline) = buf.find('\n') {
                    let line = buf[..newline].trim().to_string();
                    buf = buf[newline + 1..].to_string();

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
                                tracing::debug!("openrouter delta raw: {data}");
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
                                                        "pending idx {idx} has empty name, skip"
                                                    );
                                                    continue;
                                                }
                                                let args_valid =
                                                    if serde_json::from_str::<Value>(&args).is_ok()
                                                    {
                                                        args
                                                    } else if args.trim().is_empty() {
                                                        "{}".into()
                                                    } else {
                                                        tracing::warn!(
                                                            "invalid JSON args for {name}: {args}"
                                                        );
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
                                tracing::error!("OpenRouter error payload: {err}");
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
                    tracing::warn!("flush fallback invalid JSON for {name}: {args}");
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

    async fn complete(&self, req: ChatRequest) -> anyhow::Result<String> {
        let mut rx = self.stream(req).await?;
        let mut out = String::new();
        while let Some(ev) = rx.recv().await {
            match ev {
                ProviderEvent::TextDelta(t) => out.push_str(&t),
                ProviderEvent::Done => break,
                _ => {}
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn afford_limit_parsing() {
        assert_eq!(
            parse_afford_limit("you can only afford 128 tokens"),
            Some(128)
        );
        assert_eq!(
            parse_afford_limit("Error: Can Only Afford 4096 max"),
            Some(4096)
        );
        assert_eq!(parse_afford_limit("insufficient credits"), None);
        assert_eq!(parse_afford_limit(""), None);
        assert_eq!(parse_afford_limit("can only afford nothing here"), None);
    }

    #[test]
    fn constructors_do_not_require_env() {
        let p = OpenRouterProvider::from_env();
        assert_eq!(Provider::name(&p), "openrouter");
        let p2 = OpenRouterProvider::new("k", "https://example.invalid");
        assert_eq!(Provider::name(&p2), "openrouter");
    }
}
