use super::{ChatRequest, Provider, ProviderEvent};
use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

pub struct GeminiProvider {
    api_key: String,
    base_url: String,
    client: Client,
}

impl GeminiProvider {
    pub fn from_env() -> Self {
        let api_key = std::env::var("GEMINI_API_KEY").unwrap_or_default();
        let base_url = std::env::var("GEMINI_BASE_URL")
            .unwrap_or_else(|_| "https://generativelanguage.googleapis.com/v1beta".into());
        Self {
            api_key,
            base_url,
            client: super::http_client(),
        }
    }
}

fn openai_to_gemini(req: &ChatRequest) -> Value {
    let mut contents = Vec::new();
    let mut system_instruction = None;

    for m in &req.messages {
        let content_str = match &m.content {
            Value::String(s) => s.clone(),
            other => {
                if let Some(arr) = other.as_array() {
                    arr.iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    other.to_string()
                }
            }
        };
        if m.role == "system" {
            system_instruction = Some(json!({ "parts": [{"text": content_str}] }));
            continue;
        }

        let role = match m.role.as_str() {
            "assistant" => "model",
            "tool" => "user",
            _ => "user",
        };
        let mut parts = Vec::new();
        if let Some(tcs) = &m.tool_calls {
            for tc in tcs {
                let args: Value = serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                let mut part = json!({
                    "functionCall": {
                        "name": tc.function.name,
                        "args": args,
                        "id": tc.id
                    }
                });
                if let Some(sig) = &tc.thought_signature {
                    if !sig.trim().is_empty() {
                        part["thoughtSignature"] = json!(sig);
                    }
                } else {
                    tracing::warn!("openai_to_gemini: missing thought_signature for tool {} id {} — will likely 400, start new session", tc.function.name, tc.id);
                }
                parts.push(part);
            }
            if !content_str.is_empty() {
                parts.push(json!({"text": content_str}));
            }
        } else if m.role == "tool" {
            let name = m.name.clone().unwrap_or_else(|| "unknown".into());
            let resp: Value =
                serde_json::from_str(&content_str).unwrap_or(json!({"result": content_str}));
            parts.push(json!({
                "functionResponse": {
                    "name": name,
                    "response": resp
                }
            }));
        } else {
            parts.push(json!({"text": content_str}));
        }
        contents.push(json!({"role": role, "parts": parts}));
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature": 0.7
        }
    });

    let has_tools = req.tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
    if !has_tools {
        body["generationConfig"]["thinkingConfig"] = json!({ "includeThoughts": true });
    }

    if let Some(sys) = system_instruction {
        body["systemInstruction"] = sys;
    }

    if let Some(tools) = &req.tools {
        let decls: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": t.function.parameters
                })
            })
            .collect();
        body["tools"] = json!([{ "functionDeclarations": decls }]);
    }

    body
}

fn gemini_chunk_to_events(chunk: &Value, tx: &tokio::sync::mpsc::Sender<ProviderEvent>) {
    let candidates = match chunk.get("candidates").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return,
    };
    for cand in candidates {
        let content = match cand.get("content") {
            Some(c) => c,
            None => continue,
        };
        let parts = match content.get("parts").and_then(|p| p.as_array()) {
            Some(p) => p,
            None => continue,
        };
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                let is_thought = part
                    .get("thought")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let ev = if is_thought {
                    ProviderEvent::ReasoningDelta(text.to_string())
                } else {
                    ProviderEvent::TextDelta(text.to_string())
                };
                let _ = tx.try_send(ev);
            }
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = fc.get("args").cloned().unwrap_or(json!({}));
                let args_str = serde_json::to_string(&args).unwrap_or_else(|_| "{}".into());

                let id = fc
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("call_{}", uuid_simple()));

                let thought_signature = part
                    .get("thoughtSignature")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let _ = tx.try_send(ProviderEvent::ToolCallDelta {
                    id,
                    name,
                    args: args_str,
                    thought_signature,
                });
            }
        }
    }
}

fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{ns:x}")
}

fn pretty_gemini_error(status: reqwest::StatusCode, text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(err) = v.get("error") {
            let code = err
                .get("code")
                .and_then(|c| c.as_i64())
                .unwrap_or(status.as_u16() as i64);
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or(text);

            let short = msg
                .lines()
                .next()
                .unwrap_or(msg)
                .chars()
                .take(220)
                .collect::<String>();
            let retry = v
                .get("error")
                .and_then(|e| e.get("details"))
                .and_then(|d| d.as_array())
                .and_then(|arr| {
                    for d in arr {
                        if let Some(rd) = d.get("retryDelay").and_then(|x| x.as_str()) {
                            return Some(rd.to_string());
                        }

                        if let Some(obj) = d.as_object() {
                            for (_, vv) in obj {
                                if let Some(rd) = vv.as_str() {
                                    if rd.ends_with('s') {
                                        return Some(rd.to_string());
                                    }
                                }
                            }
                        }
                    }
                    None
                })
                .unwrap_or_default();

            if status.as_u16() == 429 || short.contains("Quota exceeded") || short.contains("quota")
            {
                let model = err
                    .get("details")
                    .and_then(|d| d.as_array())
                    .and_then(|arr| {
                        for d in arr {
                            if let Some(q) = d.get("quotaMetric").and_then(|x| x.as_str()) {
                                return Some(q.to_string());
                            }
                        }
                        None
                    })
                    .unwrap_or_default();
                let retry_hint = if retry.is_empty() {
                    "".to_string()
                } else {
                    format!(" — retry in {}", retry)
                };
                return format!(
                    "Gemini {code} Quota exceeded{}: {}{} — tip: /model <another id> or wait",
                    if model.is_empty() {
                        "".into()
                    } else {
                        format!(" ({})", model)
                    },
                    short,
                    retry_hint
                );
            }
            return format!("Gemini {code}: {}", short);
        }
    }

    let short = text
        .lines()
        .next()
        .unwrap_or(text)
        .chars()
        .take(220)
        .collect::<String>();
    format!("Gemini {}: {}", status, short)
}

#[async_trait::async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    async fn stream(
        &self,
        req: ChatRequest,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        if self.api_key.is_empty() {
            anyhow::bail!(
                "GEMINI_API_KEY not set (model '{}' requires native Gemini).\n\
                Fix:\n\
                • export GEMINI_API_KEY=... to use Google Gemini models, or\n\
                • pick another model with --model provider/model-id (or /model in the TUI)\n\
                (Gemini models are hidden in the picker when GEMINI_API_KEY is not set — strict routing, no silent OpenRouter fallback)",
                req.model
            );
        }

        let model = req
            .model
            .trim_start_matches("google/")
            .trim_start_matches("gemini-");
        let model = if model.starts_with("gemini-") {
            model.to_string()
        } else {
            format!("gemini-{model}")
        };

        let url = format!(
            "{}/models/{}:streamGenerateContent?key={}&alt=sse",
            self.base_url, model, self.api_key
        );

        let mut gemini_body = openai_to_gemini(&req);

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&gemini_body)
            .send()
            .await?;

        let resp = if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 400 && text.contains("thought_signature") {
                tracing::warn!("Gemini thought_signature 400 — retrying without thinkingConfig");

                if let Some(gc) = gemini_body.get_mut("generationConfig") {
                    if let Some(obj) = gc.as_object_mut() {
                        obj.remove("thinkingConfig");
                    }
                }
                let retry = self
                    .client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .json(&gemini_body)
                    .send()
                    .await?;
                if !retry.status().is_success() {
                    let status2 = retry.status();
                    let text2 = retry.text().await.unwrap_or_default();
                    anyhow::bail!(
                        "{} (retry without thought also failed, original: {})",
                        pretty_gemini_error(status2, &text2),
                        pretty_gemini_error(status, &text)
                    );
                }
                retry
            } else {
                anyhow::bail!("{}", pretty_gemini_error(status, &text));
            }
        } else {
            resp
        };

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let mut stream = resp.bytes_stream();

        tokio::spawn(async move {
            let mut buf = String::new();

            while let Some(chunk) = stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::error!("gemini stream error: {e}");
                        break;
                    }
                };
                let text = String::from_utf8_lossy(&bytes);
                buf.push_str(&text);

                if buf.len() > super::MAX_SSE_BUF {
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
                    } else {
                        line.as_str()
                    };
                    if data == "[DONE]" {
                        let _ = tx.send(ProviderEvent::Done).await;
                        break;
                    }

                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        gemini_chunk_to_events(&v, &tx);
                    }
                }
            }

            for line in buf.lines() {
                let line = line.trim();
                if line.is_empty() || line == "[DONE]" {
                    continue;
                }
                let data = if line.starts_with("data: ") {
                    line.trim_start_matches("data: ").trim()
                } else {
                    line
                };
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    gemini_chunk_to_events(&v, &tx);
                }
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
