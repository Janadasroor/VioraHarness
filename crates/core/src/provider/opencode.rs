use super::{ChatRequest, Provider, ProviderEvent};
use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};

const ZEN_BASE: &str = "https://opencode.ai/zen/v1";
const GO_BASE: &str = "https://opencode.ai/zen/go/v1";

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;

    if let Ok(s) = v.trim().parse::<u64>() {
        return Some(std::time::Duration::from_secs(s.min(300)));
    }
    None
}

fn is_free_quota_body(body: &str) -> bool {
    let b = body.to_lowercase();
    b.contains("freeusage")
        || b.contains("free usage")
        || b.contains("free tier")
        || b.contains("quota")
        || b.contains("exhausted")
}

fn is_auth_body(body: &str) -> bool {
    body.contains("Invalid API key")
        || body.contains("AuthError")
        || body.contains("Invalid")
        || body.contains("Auth")
}

fn http_retry_delays(
    status: u16,
    body: &str,
    retry_after: Option<std::time::Duration>,
) -> Vec<std::time::Duration> {
    if status == 401 || is_auth_body(body) {
        return Vec::new();
    }
    let mut delays: Vec<std::time::Duration> = if status == 429 {
        if is_free_quota_body(body) {
            vec![15, 30, 60, 90]
                .into_iter()
                .map(std::time::Duration::from_secs)
                .collect()
        } else {
            vec![2, 4, 8, 16, 32]
                .into_iter()
                .map(std::time::Duration::from_secs)
                .collect()
        }
    } else if matches!(status, 408 | 500 | 502 | 503 | 504 | 529) {
        vec![1, 2, 4, 8]
            .into_iter()
            .map(std::time::Duration::from_secs)
            .collect()
    } else {
        return Vec::new();
    };
    if let Some(ra) = retry_after {
        if let Some(first) = delays.first_mut() {
            *first = ra.min(std::time::Duration::from_secs(300));
        }
    }
    delays
}

fn transport_retry_delays() -> Vec<std::time::Duration> {
    vec![1, 2, 4, 8]
        .into_iter()
        .map(std::time::Duration::from_secs)
        .collect()
}

fn jitter_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % 1000)
        .unwrap_or(0)
}

async fn send_with_retry(
    build: impl Fn() -> reqwest::RequestBuilder,
    kind: &str,
    notices: Option<&tokio::sync::mpsc::Sender<super::ProviderEvent>>,
) -> Result<reqwest::Response, (Option<u16>, String, u32, u64)> {
    let mut waited: u64 = 0;
    let mut attempt: u32 = 0;
    loop {
        match build().send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    return Ok(resp);
                }
                let status = resp.status().as_u16();
                let ra = parse_retry_after(resp.headers());
                let text = resp.text().await.unwrap_or_default();
                let delays = http_retry_delays(status, &text, ra);
                if attempt as usize >= delays.len() {
                    let terminal = if delays.is_empty() {
                        "fatal"
                    } else {
                        "exhausted"
                    };
                    tracing::warn!(
                        "gateway {kind} {terminal} after {} attempt(s): HTTP {status}",
                        attempt + 1
                    );
                    return Err((Some(status), text, attempt + 1, waited));
                }
                let mut d = delays[attempt as usize];
                d += std::time::Duration::from_millis(jitter_ms());
                let msg = format!(
                    "gateway {kind} HTTP {status} — waiting {}s before retry {}/{}",
                    d.as_secs(),
                    attempt + 1,
                    delays.len()
                );
                tracing::warn!("{msg}");
                if let Some(tx) = notices {
                    let _ = tx.send(super::ProviderEvent::Notice(msg)).await;
                }
                tokio::time::sleep(d).await;
                waited += d.as_secs();
                attempt += 1;
            }
            Err(e) => {
                let delays = transport_retry_delays();
                if attempt as usize >= delays.len() {
                    tracing::warn!(
                        "gateway {kind} request failed after {} attempt(s): {e}",
                        attempt + 1
                    );
                    return Err((None, format!("request failed: {e}"), attempt + 1, waited));
                }
                let mut d = delays[attempt as usize];
                d += std::time::Duration::from_millis(jitter_ms());
                let msg = format!(
                    "gateway {kind} connection error — retrying in {}s ({}/{})",
                    d.as_secs(),
                    attempt + 1,
                    delays.len()
                );
                tracing::warn!("{msg}: {e}");
                if let Some(tx) = notices {
                    let _ = tx.send(super::ProviderEvent::Notice(msg)).await;
                }
                tokio::time::sleep(d).await;
                waited += d.as_secs();
                attempt += 1;
            }
        }
    }
}

fn coalesce_pending(mut entries: Vec<(String, String, String)>) -> Vec<(String, String, String)> {
    let needy: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, (_, n, a))| !n.is_empty() && a.is_empty())
        .map(|(i, _)| i)
        .collect();
    let orphans: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, (_, n, a))| n.is_empty() && !a.is_empty())
        .map(|(i, _)| i)
        .collect();
    if needy.len() == 1 && orphans.len() == 1 {
        let (ni, oi) = (needy[0], orphans[0]);
        let args = std::mem::take(&mut entries[oi].2);
        entries[ni].2 = args;
        if entries[ni].0.is_empty() {
            let id = std::mem::take(&mut entries[oi].0);
            entries[ni].0 = id;
        }
        entries.remove(oi);
    }
    entries
}

pub fn is_free_model_dynamic(bare: &str) -> bool {
    bare.to_lowercase().ends_with("-free")
}

fn opencode_api_key() -> String {
    for k in [
        "OPENCODE_API_KEY",
        "ZEN_API_KEY",
        "OPENCODE_ZEN_API_KEY",
        "OPENCODE_GO_API_KEY",
    ] {
        if let Ok(v) = std::env::var(k) {
            if !v.trim().is_empty() {
                return v;
            }
        }
    }
    String::new()
}

fn opencode_base_for_model(model: &str) -> String {
    if let Ok(b) = std::env::var("OPENCODE_BASE_URL") {
        if !b.trim().is_empty() {
            return b.trim_end_matches('/').to_string();
        }
    }
    if let Ok(b) = std::env::var("OPENCODE_GO_BASE_URL") {
        if !b.trim().is_empty() && model.to_lowercase().starts_with("opencode-go/") {
            return b.trim_end_matches('/').to_string();
        }
    }
    let m = model.to_lowercase();
    if m.starts_with("opencode-go/")
        || m.starts_with("opencode_go/")
        || m.starts_with("go/")
        || m.starts_with("opencode/go/")
    {
        return GO_BASE.to_string();
    }
    ZEN_BASE.to_string()
}

fn strip_model_prefix(model: &str) -> String {
    let lower = model.to_lowercase();
    for pref in [
        "opencode-go/",
        "opencode_go/",
        "opencode/go/",
        "opencode:",
        "opencode/",
        "zen/",
        "go/",
    ] {
        if lower.starts_with(pref) {
            return model[pref.len()..].to_string();
        }
    }
    model.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointKind {
    Chat,
    Responses,
    Messages,
    Google,
}

fn endpoint_for_model(bare: &str, base_is_go: bool) -> EndpointKind {
    let b = bare.to_lowercase();

    if b.starts_with("gemini-") || b.starts_with("gemini_") {
        return EndpointKind::Google;
    }

    if b.starts_with("gpt-")
        || b.starts_with("gpt5")
        || b.starts_with("gpt-5")
        || b.starts_with("grok-")
        || b.starts_with("grok")
        || b.starts_with("muse-spark")
        || b.starts_with("muse_spark")
    {
        return EndpointKind::Responses;
    }

    if b.starts_with("claude-") || b.starts_with("claude") || b.starts_with("qwen") {
        return EndpointKind::Messages;
    }
    if base_is_go && (b.starts_with("minimax-") || b.starts_with("minimax")) {
        return EndpointKind::Messages;
    }

    EndpointKind::Chat
}

pub fn is_opencode_model(model: &str) -> bool {
    let m = model.to_lowercase();
    if m.starts_with("opencode/")
        || m.starts_with("opencode-go/")
        || m.starts_with("opencode_go/")
        || m.starts_with("opencode:")
        || m.starts_with("zen/")
        || m.starts_with("go/")
    {
        return true;
    }

    if is_free_model_dynamic(&m) {
        return true;
    }
    false
}

pub fn is_free_model(model: &str) -> bool {
    let bare = strip_model_prefix(model).to_lowercase();
    is_free_model_dynamic(&bare)
}

pub struct OpenCodeProvider {
    api_key: String,
    base_url: String,
    client: Client,

    _orig_model: String,
}

impl OpenCodeProvider {
    pub fn from_env_for_model(model: &str) -> Self {
        let api_key = opencode_api_key();
        let base_url = opencode_base_for_model(model);
        Self {
            api_key,
            base_url,
            client: super::http_client(),
            _orig_model: model.to_string(),
        }
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            client: super::http_client(),
            _orig_model: String::new(),
        }
    }

    fn effective_key(&self, model: &str) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }

        if is_free_model(model) {
            return "public".to_string();
        }
        String::new()
    }

    fn session_header() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("vioraharness-{:x}", ns & 0xffffff)
    }
}

fn chat_to_responses_body(req: &ChatRequest, bare_model: &str) -> Value {
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

    let mut body = json!({
        "model": bare_model,
        "input": input,
        "stream": true,
        "store": false,
        "parallel_tool_calls": true,
        "reasoning": {"effort": "medium"},
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

fn chat_to_messages_body(req: &ChatRequest, bare_model: &str) -> Value {
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

    let mut body = json!({
        "model": bare_model,
        "messages": messages,
        "stream": true,
        "max_tokens": req.max_tokens.unwrap_or(1024)
    });
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

#[async_trait::async_trait]
impl Provider for OpenCodeProvider {
    fn name(&self) -> &str {
        if self.base_url.contains("/go/") {
            "gateway-go"
        } else {
            "gateway"
        }
    }

    async fn stream(
        &self,
        req: ChatRequest,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
        let bare_model = strip_model_prefix(&req.model);
        let key = self.effective_key(&req.model);
        if key.is_empty() {
            anyhow::bail!(
                "OPENCODE_API_KEY (or ZEN_API_KEY) not set for model '{}' (bare '{}').\nFix: export OPENCODE_API_KEY=... from https://opencode.ai/auth (see the gateway pricing page) or use a subscription catalog.\nFree-tier models (any *-free id from the live catalog) also work with key=public, but setting a key is recommended. Run `cargo run -- doctor` to check.",
                req.model,
                bare_model,
            );
        }
        let base_is_go = self.base_url.contains("/go/");
        let kind = endpoint_for_model(&bare_model, base_is_go);

        match kind {
            EndpointKind::Google => {
                anyhow::bail!(
                    "Model '{}' is a Gemini model via the managed gateway (endpoint /models/gemini-*).\nHint: use the native Gemini provider instead: model 'google/{}' with GEMINI_API_KEY.",
                    req.model, bare_model
                );
            }
            EndpointKind::Chat => self.stream_chat(req, bare_model, key).await,
            EndpointKind::Responses => self.stream_responses(req, bare_model, key).await,
            EndpointKind::Messages => self.stream_messages(req, bare_model, key).await,
        }
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

impl OpenCodeProvider {
    async fn stream_chat(
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

    async fn stream_responses(
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

                if buf.len() > super::MAX_SSE_BUF {
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

    async fn stream_messages(
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
                if status == Some(401) || detail.contains("Invalid") || detail.contains("Auth") {
                    anyhow::bail!("Gateway {} messages error {code}: {detail}\nHint: check OPENCODE_API_KEY at https://opencode.ai/auth", self.name());
                }
                if status == Some(429)
                    || detail.to_lowercase().contains("rate")
                    || detail.to_lowercase().contains("limit")
                {
                    anyhow::bail!("Gateway {} messages error {code}: {} (after {attempts} attempts, waited {waited}s)\nHint: rate/limit still hit — try a different model or wait a minute", self.name(), detail.chars().take(400).collect::<String>());
                }
                anyhow::bail!(
                    "Gateway {} messages error {code}: {} (after {attempts} attempts)",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatMessage;

    async fn replay_sse(raw: Vec<u8>, chunk: usize) -> Vec<crate::provider::ProviderEvent> {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 8192];
            let _ = sock.read(&mut req);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                raw.len()
            );
            sock.write_all(head.as_bytes()).unwrap();
            for piece in raw.chunks(chunk) {
                sock.write_all(piece).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        let prov = OpenCodeProvider::new("public", format!("http://127.0.0.1:{port}"));
        let req = crate::provider::ChatRequest {
            model: "unit-muse-spark-replay".into(),
            messages: vec![crate::provider::ChatMessage::text("user", "hi")],
            tools: None,
            tool_choice: None,
            max_tokens: None,
            temperature: None,
        };

        let mut rx = prov
            .stream_responses(req, "unit-muse-spark-replay".into(), "public".into())
            .await
            .expect("stream opens");
        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            let done = matches!(ev, crate::provider::ProviderEvent::Done);
            out.push(ev);
            if done {
                break;
            }
        }
        server.join().unwrap();
        out
    }

    /// Regression: the responses parser must be chunk-size invariant.

    fn event_signature(evs: &[crate::provider::ProviderEvent]) -> Vec<String> {
        evs.iter()
            .map(|e| match e {
                crate::provider::ProviderEvent::TextDelta(t) => format!("T{}", t.len()),
                crate::provider::ProviderEvent::ReasoningDelta(r) => format!("R{}", r.len()),
                crate::provider::ProviderEvent::ToolCallDelta { name, args, .. } => {
                    format!("C{name}:{}", args.len())
                }
                crate::provider::ProviderEvent::Done => "D".into(),
                _ => "?".into(),
            })
            .collect()
    }

    fn replay_text(evs: &[crate::provider::ProviderEvent]) -> (String, String) {
        let mut text = String::new();
        let mut reasoning = String::new();
        for e in evs {
            match e {
                crate::provider::ProviderEvent::TextDelta(t) => text.push_str(t),
                crate::provider::ProviderEvent::ReasoningDelta(r) => reasoning.push_str(r),
                _ => {}
            }
        }
        (text, reasoning)
    }

    #[tokio::test]
    async fn responses_parser_is_chunk_invariant() {
        let raw = b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n\
event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"\"}}\n\n\
event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"command\\\":\\\"ls\\\"}\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}]}}\n\n".to_vec();
        let mut reference = None;
        for chunk in [1, 7, 64, 4096, 1_000_000] {
            let sig = event_signature(&replay_sse(raw.clone(), chunk).await);

            assert!(
                sig.iter().filter(|s| s.starts_with("Cbash")).count() == 1,
                "chunk {chunk}: exactly one call, got {sig:?}"
            );
            assert!(
                sig.last().is_some_and(|s| s == "D"),
                "chunk {chunk}: Done last"
            );
            match &reference {
                None => reference = Some(sig),
                Some(r) => assert_eq!(&sig, r, "chunk {chunk} diverges"),
            }
        }

        let evs = replay_sse(raw.clone(), 7).await;
        let args = evs.iter().find_map(|e| match e {
            crate::provider::ProviderEvent::ToolCallDelta { args, .. } => Some(args.clone()),
            _ => None,
        });
        assert_eq!(args.as_deref(), Some("{\"command\":\"ls\"}"));
    }

    #[tokio::test]
    async fn responses_completed_snapshot_not_resent_after_deltas() {
        let raw = b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"PO\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"NG\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"PONG\"}]}]}}\n\n".to_vec();
        for chunk in [1, 7, 4096] {
            let evs = replay_sse(raw.clone(), chunk).await;
            let (text, _) = replay_text(&evs);
            assert_eq!(text, "PONG", "chunk {chunk}: no doubled text, got {text:?}");
        }
    }

    #[tokio::test]
    async fn responses_completed_snapshot_is_fallback_without_deltas() {
        let raw = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"HELLO\"}]}]}}\n\n".to_vec();
        let evs = replay_sse(raw.clone(), 64).await;
        let (text, _) = replay_text(&evs);
        assert_eq!(text, "HELLO", "snapshot fallback works: {text:?}");
    }

    #[tokio::test]
    async fn responses_reasoning_summary_reaches_label() {
        let raw = b"event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"Checking\"}\n\n\
event: response.reasoning_summary_text.delta\ndata: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\" files\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Checking files\"}]},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"done\"}]}]}}\n\n".to_vec();
        let evs = replay_sse(raw.clone(), 64).await;
        let (text, reasoning) = replay_text(&evs);
        assert_eq!(text, "done", "message text once: {text:?}");
        assert_eq!(
            reasoning, "Checking files",
            "live summary deltas, no completed dup: {reasoning:?}"
        );

        let raw2 = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Late summary\"}]}]}}\n\n".to_vec();
        let evs2 = replay_sse(raw2.clone(), 64).await;
        let (_, reasoning2) = replay_text(&evs2);
        assert_eq!(
            reasoning2, "Late summary",
            "completed fallback: {reasoning2:?}"
        );
    }

    #[test]
    fn coalesce_pending_merges_split_entries() {
        let out = coalesce_pending(vec![
            ("call-1".into(), "websearch".into(), "".into()),
            ("".into(), "".into(), "{\"q\":\"x\"}".into()),
        ]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].0, "call-1");
        assert_eq!(out[0].1, "websearch");
        assert_eq!(out[0].2, "{\"q\":\"x\"}");

        let out = coalesce_pending(vec![
            ("a".into(), "t1".into(), "".into()),
            ("b".into(), "t2".into(), "".into()),
            ("".into(), "".into(), "{}".into()),
        ]);
        assert_eq!(out.len(), 3, "{out:?}");

        let out = coalesce_pending(vec![("c".into(), "t".into(), "{}".into())]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn retry_plans() {
        use std::time::Duration;

        assert!(http_retry_delays(401, "Invalid API key", None).is_empty());
        assert!(http_retry_delays(400, "bad request", None).is_empty());
        assert!(http_retry_delays(403, "forbidden", None).is_empty());
        assert!(http_retry_delays(404, "not found", None).is_empty());

        let d = http_retry_delays(429, "too many requests", None);
        assert_eq!(
            d,
            vec![2, 4, 8, 16, 32]
                .into_iter()
                .map(Duration::from_secs)
                .collect::<Vec<_>>()
        );

        let d = http_retry_delays(429, "FreeUsageLimitError: rate limit exceeded", None);
        assert_eq!(d.len(), 4);
        assert!(d.iter().map(|x| x.as_secs()).sum::<u64>() <= 210);

        assert_eq!(http_retry_delays(503, "overloaded", None).len(), 4);
        assert_eq!(http_retry_delays(504, "gateway timeout", None).len(), 4);
        assert_eq!(http_retry_delays(529, "overloaded", None).len(), 4);
        assert_eq!(transport_retry_delays().len(), 4);

        let d = http_retry_delays(429, "slow down", Some(Duration::from_secs(45)));
        assert_eq!(d[0], Duration::from_secs(45));
        let d = http_retry_delays(429, "slow down", Some(Duration::from_secs(9999)));
        assert_eq!(d[0], Duration::from_secs(300));

        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("20"),
        );
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(20)));
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&h), None);
    }

    #[test]
    fn routing_prefixes() {
        assert!(is_opencode_model("opencode/gpt-test-1"));
        assert!(is_opencode_model("opencode-go/kimi-test"));
        assert!(is_opencode_model("zen/gpt-test"));
        assert!(is_opencode_model("go/mimo-test"));
        assert!(is_opencode_model("unit-test-free"));
        assert!(!is_opencode_model("openai/gpt-test-mini"));
        assert!(!is_opencode_model("google/gemini-test-flash"));
    }

    #[test]
    fn strip_prefix() {
        assert_eq!(strip_model_prefix("opencode/gpt-test-1"), "gpt-test-1");
        assert_eq!(strip_model_prefix("opencode-go/kimi-test"), "kimi-test");
        assert_eq!(strip_model_prefix("zen/gpt-test"), "gpt-test");
        assert_eq!(strip_model_prefix("no-prefix-here"), "no-prefix-here");
    }

    #[test]
    fn free_detection() {
        assert!(is_free_model("opencode/mimo-test-free"));
        assert!(is_free_model("mimo-test-free"));
        assert!(is_free_model("opencode/MIMO-TEST-FREE"));
        assert!(!is_free_model("opencode/gpt-test-1"));
        assert!(!is_free_model("openai/gpt-test-mini"));
    }

    #[test]
    fn base_selection() {
        assert_eq!(opencode_base_for_model("opencode/gpt-test-1"), ZEN_BASE);
        assert_eq!(opencode_base_for_model("opencode-go/kimi-test"), GO_BASE);
        assert_eq!(opencode_base_for_model("go/mimo-test"), GO_BASE);
    }

    #[test]
    fn endpoint_detection() {
        assert_eq!(
            endpoint_for_model("gpt-test-1", false),
            EndpointKind::Responses
        );
        assert_eq!(
            endpoint_for_model("grok-test-1", false),
            EndpointKind::Responses
        );
        assert_eq!(
            endpoint_for_model("muse-spark-unit-contributor-free", false),
            EndpointKind::Responses
        );
        assert_eq!(
            endpoint_for_model("claude-test-4-5", false),
            EndpointKind::Messages
        );
        assert_eq!(
            endpoint_for_model("qwen-test-plus", false),
            EndpointKind::Messages
        );
        assert_eq!(
            endpoint_for_model("gemini-test-flash", false),
            EndpointKind::Google
        );
        assert_eq!(endpoint_for_model("kimi-test", false), EndpointKind::Chat);
        assert_eq!(endpoint_for_model("kimi-test", true), EndpointKind::Chat);
        assert_eq!(
            endpoint_for_model("minimax-test", false),
            EndpointKind::Chat
        );
        assert_eq!(
            endpoint_for_model("minimax-test", true),
            EndpointKind::Messages
        );
        assert_eq!(
            endpoint_for_model("deepseek-test-flash", false),
            EndpointKind::Chat
        );
    }

    #[test]
    fn chat_to_responses_body_smoke() {
        let req = ChatRequest {
            model: "opencode/gpt-test-1".into(),
            messages: vec![ChatMessage::text("user", "hello")],
            tools: None,
            tool_choice: None,
            max_tokens: Some(20),
            temperature: Some(0.7),
        };
        let body = chat_to_responses_body(&req, "gpt-test-1");
        assert_eq!(body["model"], json!("gpt-test-1"));
        assert!(body["input"].is_array());

        assert_eq!(body["reasoning"]["effort"], json!("medium"));
        assert_eq!(body["reasoning"]["summary"], json!("auto"));
        let grok_body = chat_to_responses_body(&req, "grok-test-1");
        assert_eq!(grok_body["reasoning"]["effort"], json!("medium"));
        assert!(
            grok_body["reasoning"].get("summary").is_none(),
            "grok excluded (unverified — a 400 would break the model)"
        );
    }

    #[test]
    fn chat_to_messages_body_smoke() {
        let req = ChatRequest {
            model: "opencode/claude-test-4-5".into(),
            messages: vec![ChatMessage::text("user", "hello")],
            tools: None,
            tool_choice: None,
            max_tokens: Some(20),
            temperature: Some(0.7),
        };
        let body = chat_to_messages_body(&req, "claude-test-4-5");
        assert_eq!(body["model"], json!("claude-test-4-5"));
        assert!(body["messages"].is_array());
    }
}
