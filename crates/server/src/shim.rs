// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

//! Free-tier gateway shim for clients that only accept URL + API key.
//!
//! Forwards OpenAI-compatible requests to the Zen gateway while injecting
//! the session header the free tier requires and stripping provider id
//! prefixes (`zen/`, `opencode/`, …) down to the bare model id.
//! Response bodies (including SSE streams) pass through untouched.

use axum::{
    body::Body,
    extract::{OriginalUri, Request, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use serde_json::Value;

const UPSTREAM: &str = "https://opencode.ai/zen/v1";
const MAX_BODY: usize = 32 * 1024 * 1024;

/// Bare model id: the gateway rejects prefixed ids with ModelError.
pub fn bare_model_id(model: &str) -> String {
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

/// IDE plugins append `/v1` themselves; drop it before upstreaming.
pub fn upstream_path(request_path: &str) -> String {
    let rel = request_path
        .strip_prefix("/v1")
        .unwrap_or(request_path)
        .trim_start_matches('/')
        .trim_end_matches('/');
    rel.to_string()
}

/// Session marker for the free tier (presence-checked, value free-form).
pub fn shim_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("vioraharness-shim-{:x}", ns & 0xffffff)
}

/// Rewrite `"model"` in a JSON request body, if present.
pub fn rewrite_model(body: &[u8]) -> Option<Vec<u8>> {
    let mut data: Value = serde_json::from_slice(body).ok()?;
    if let Some(model) = data.get("model").and_then(Value::as_str) {
        data["model"] = Value::String(bare_model_id(model));
        return serde_json::to_vec(&data).ok();
    }
    None
}

fn is_hop_header(name: &str) -> bool {
    matches!(
        name,
        "host" | "connection" | "content-length" | "transfer-encoding" | "keep-alive" | "upgrade"
    )
}

/// Models in this family are served by the Responses endpoint; a chat
/// request for them fails upstream, so the shim translates instead.
pub fn is_responses_model(bare: &str) -> bool {
    let b = bare.to_lowercase();
    b.starts_with("gpt-")
        || b.starts_with("gpt5")
        || b.starts_with("gpt-5")
        || b.starts_with("grok-")
        || b.starts_with("grok")
        || b.starts_with("muse-spark")
        || b.starts_with("muse_spark")
}

fn message_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Chat payload → Responses payload (text only; tools stay on chat-native
/// models). Reasoning-heavy models burn the whole budget on thinking, so
/// the output cap is floored like the main provider does.
pub fn responses_body_from_chat(chat: &Value) -> Value {
    let transcript = chat
        .get("messages")
        .and_then(Value::as_array)
        .map(|msgs| {
            msgs.iter()
                .map(|m| {
                    let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
                    let text = m.get("content").map(message_text).unwrap_or_default();
                    if role == "user" {
                        text
                    } else {
                        format!("{role}: {text}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let mut out = serde_json::json!({
        "model": chat.get("model").and_then(Value::as_str).map(bare_model_id).unwrap_or_default(),
        "input": transcript,
        "max_output_tokens": chat
            .get("max_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(16384)
            .max(16384),
        "stream": chat.get("stream").and_then(Value::as_bool).unwrap_or(false),
    });
    for key in ["temperature", "top_p"] {
        if let Some(v) = chat.get(key) {
            out[key] = v.clone();
        }
    }
    out
}

/// Responses payload → chat completion payload.
pub fn chat_from_responses(model: &str, resp: &Value) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    if let Some(items) = resp.get("output").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                                if let Some(t) = part.get("text").and_then(Value::as_str) {
                                    text.push_str(t);
                                }
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    if let Some(sums) = item.get("summary").and_then(Value::as_array) {
                        for s in sums {
                            if let Some(t) = s.get("text").and_then(Value::as_str) {
                                reasoning.push_str(t);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let usage = resp.get("usage");
    let u = |k: &str| {
        usage
            .and_then(|u| u.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let mut msg = serde_json::json!({"role": "assistant", "content": text});
    if !reasoning.is_empty() {
        msg["reasoning_content"] = Value::String(reasoning);
    }
    serde_json::json!({
        "id": resp.get("id").cloned().unwrap_or(Value::String(format!("chatcmpl-{}", shim_session_id()))),
        "object": "chat.completion",
        "created": resp.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        "model": model,
        "choices": [{"index": 0, "message": msg, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": u("input_tokens"),
            "completion_tokens": u("output_tokens"),
            "total_tokens": u("total_tokens"),
        },
    })
}

/// Streaming translator state: Responses SSE events → chat chunks.
pub struct SseChatMap {
    model: String,
    id: String,
    created: u64,
    started: bool,
}

impl SseChatMap {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            id: format!("chatcmpl-{}", shim_session_id()),
            created: 0,
            started: false,
        }
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> String {
        let choice = match finish {
            Some(f) => serde_json::json!({"index": 0, "delta": {}, "finish_reason": f}),
            None => serde_json::json!({"index": 0, "delta": delta, "finish_reason": null}),
        };
        format!(
            "data: {}\n\n",
            serde_json::json!({
                "id": self.id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [choice],
            })
        )
    }

    /// Map one Responses SSE event payload to zero or more chat `data:` lines.
    pub fn map_event(&mut self, event: &Value) -> Vec<String> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "response.created" => {
                if let Some(r) = event.get("response") {
                    if let Some(id) = r.get("id").and_then(Value::as_str) {
                        self.id = id.to_string();
                    }
                    if let Some(c) = r.get("created_at").and_then(Value::as_u64) {
                        self.created = c;
                    }
                }
                vec![]
            }
            "response.output_text.delta" => {
                let mut out = vec![];
                if !self.started {
                    self.started = true;
                    out.push(self.chunk(serde_json::json!({"role": "assistant"}), None));
                }
                if let Some(d) = event.get("delta").and_then(Value::as_str) {
                    if !d.is_empty() {
                        out.push(self.chunk(serde_json::json!({"content": d}), None));
                    }
                }
                out
            }
            "response.completed" => {
                let incomplete = event
                    .get("response")
                    .and_then(|r| r.get("incomplete_details"))
                    .is_some_and(|v| !v.is_null());
                vec![self.chunk(
                    Value::Null,
                    Some(if incomplete { "length" } else { "stop" }),
                )]
            }
            "response.incomplete" => vec![self.chunk(Value::Null, Some("length"))],
            "response.failed" => vec![self.chunk(Value::Null, Some("stop"))],
            _ => vec![],
        }
    }
}

#[derive(Clone)]
struct ShimState {
    /// Shared client: keeps TLS/TCP connections to the gateway alive
    /// across requests instead of re-handshaking every call.
    client: reqwest::Client,
}

fn forward_headers(parts: &axum::http::request::Parts) -> reqwest::header::HeaderMap {
    let mut fwd = reqwest::header::HeaderMap::new();
    for (name, value) in &parts.headers {
        if is_hop_header(name.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            fwd.insert(n, v);
        }
    }
    if let Ok(v) = HeaderValue::from_str(&shim_session_id()) {
        if let (Ok(n), Ok(vv)) = (
            reqwest::header::HeaderName::from_bytes(b"x-opencode-session"),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            fwd.insert(n, vv);
        }
    }
    fwd
}

/// Chat request for a Responses-family model: POST a translated body to
/// `/responses` and shape the answer back into chat format (JSON or SSE).
async fn forward_translated(
    state: &ShimState,
    parts: &axum::http::request::Parts,
    chat: &Value,
    model: &str,
    stream: bool,
) -> Response {
    let model_bare = bare_model_id(model);
    let body = responses_body_from_chat(chat);
    let upstream = match state
        .client
        .post(format!("{UPSTREAM}/responses"))
        .headers(forward_headers(parts))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => {
            tracing::info!("shim POST /responses ({}) -> {}", model_bare, r.status());
            r
        }
        Err(e) => {
            tracing::warn!("shim responses upstream failed: {e:#}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    if !stream {
        let status =
            StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        let bytes = upstream.bytes().await.unwrap_or_default();
        if !status.is_success() {
            return (status, bytes.to_vec()).into_response();
        }
        let resp: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return (status, bytes.to_vec()).into_response(),
        };
        return Json(chat_from_responses(&model_bare, &resp)).into_response();
    }
    // Streaming: re-emit Responses SSE events as chat chunks.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::body::Bytes, axum::Error>>(64);
    let mut map = SseChatMap::new(&model_bare);
    let mut byte_stream = upstream.bytes_stream();
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut buf: Vec<u8> = Vec::new();
        let mut closed = false;
        while let Some(chunk) = byte_stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => break,
            };
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end();
                if text.is_empty() || text == ":" {
                    continue;
                }
                let Some(payload) = text.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload == "[DONE]" {
                    closed = true;
                    break;
                }
                let Ok(event) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                for out in map.map_event(&event) {
                    if tx.send(Ok(axum::body::Bytes::from(out))).await.is_err() {
                        return;
                    }
                }
            }
            if closed {
                break;
            }
        }
        let _ = tx
            .send(Ok(axum::body::Bytes::from("data: [DONE]\n\n")))
            .await;
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// Fill `context_length`/`context_window` on each model entry. Currently
/// unused: kept for clients with lenient parsers, since strict OpenAI-schema
/// clients reject unknown fields outright.
#[allow(dead_code)]
pub fn enrich_models_list(
    mut list: Value,
    sizes: &std::collections::HashMap<String, usize>,
) -> Value {
    use vioraharness_core::provider::catalog::match_or_context;
    if let Some(arr) = list.get_mut("data").and_then(Value::as_array_mut) {
        for m in arr.iter_mut() {
            let id = m
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let ctx = match_or_context(&id, sizes).unwrap_or(128_000);
            m["context_length"] = Value::from(ctx);
            m["context_window"] = Value::from(ctx);
        }
    }
    list
}

async fn forward(
    State(state): State<ShimState>,
    OriginalUri(uri): OriginalUri,
    req: Request,
) -> Response {
    let rel = upstream_path(uri.path());
    if rel.is_empty() {
        return Json(serde_json::json!({
            "object": "shim",
            "upstream": UPSTREAM,
            "usage": "point an OpenAI-compatible client at /v1 with any key",
        }))
        .into_response();
    }
    let ua = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-");
    let t0 = std::time::Instant::now();
    tracing::info!("shim {} {} ua={}", req.method(), uri.path(), ua);
    let mut url = format!("{UPSTREAM}/{rel}");
    if let Some(q) = uri.query() {
        url.push('?');
        url.push_str(q);
    }

    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let is_json = parts
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"));
    let chat: Option<Value> = if is_json {
        serde_json::from_slice(&bytes).ok()
    } else {
        None
    };
    // Responses-family models fail on the chat endpoint upstream: translate
    // the request, then shape the answer back into chat format.
    if rel == "chat/completions" {
        if let Some(ref c) = chat {
            if let Some(model) = c.get("model").and_then(Value::as_str) {
                if is_responses_model(&bare_model_id(model)) {
                    let stream = c.get("stream").and_then(Value::as_bool).unwrap_or(false);
                    return forward_translated(&state, &parts, c, model, stream).await;
                }
            }
        }
    }
    let send_body = if is_json {
        rewrite_model(&bytes).unwrap_or_else(|| bytes.to_vec())
    } else {
        bytes.to_vec()
    };

    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::POST);
    let fwd = forward_headers(&parts);

    let upstream = match state
        .client
        .request(method, &url)
        .headers(fwd)
        .body(send_body)
        .send()
        .await
    {
        Ok(r) => {
            tracing::info!(
                "shim {} {} -> {} in {}ms",
                parts.method,
                uri.path(),
                r.status(),
                t0.elapsed().as_millis()
            );
            r
        }
        Err(e) => {
            tracing::warn!("shim upstream failed: {e:#}");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    for (name, value) in upstream.headers() {
        if is_hop_header(name.as_str()) {
            continue;
        }
        if let Ok(v) = HeaderValue::from_bytes(value.as_bytes()) {
            out = out.header(name.as_str(), v);
        }
    }
    out.body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

pub async fn serve_shim(port: u16) -> anyhow::Result<()> {
    let state = ShimState {
        client: reqwest::Client::new(),
    };
    let app = Router::new()
        .route("/", any(forward))
        .route("/*rest", any(forward))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    tracing::info!("gateway shim on http://127.0.0.1:{port}/v1 (loopback only, Ctrl-C to stop)");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_provider_prefixes() {
        assert_eq!(bare_model_id("zen/gpt-5.5"), "gpt-5.5");
        assert_eq!(bare_model_id("opencode/claude-x"), "claude-x");
        assert_eq!(bare_model_id("ZEN/MIMO-V2.5-FREE"), "MIMO-V2.5-FREE");
        assert_eq!(
            bare_model_id("nemotron-3.5-lightning-free"),
            "nemotron-3.5-lightning-free"
        );
    }

    #[test]
    fn drops_plugin_v1_prefix() {
        assert_eq!(upstream_path("/v1/chat/completions"), "chat/completions");
        assert_eq!(upstream_path("/v1/models"), "models");
        assert_eq!(upstream_path("/chat/completions"), "chat/completions");
        assert_eq!(upstream_path("/"), "");
    }

    #[test]
    fn rewrites_model_in_json_body() {
        let out = rewrite_model(br#"{"model":"zen/gpt-5.5","messages":[]}"#).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["model"], "gpt-5.5");
        assert!(v.get("messages").is_some(), "rest untouched");
        assert!(rewrite_model(br#"{"messages":[]}"#).is_none());
        assert!(rewrite_model(b"not json").is_none());
    }

    #[test]
    fn session_id_has_stable_shape() {
        let id = shim_session_id();
        assert!(id.starts_with("vioraharness-shim-"), "{id}");
    }

    #[test]
    fn models_list_gains_context_fields() {
        use std::collections::HashMap;
        let sizes: HashMap<String, usize> = [("meta/muse-spark-1.3-contributor".into(), 1048576)]
            .into_iter()
            .collect();
        let list = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "muse-spark-1.3-contributor-free", "object": "model"},
                {"id": "mystery-model-zzz", "object": "model"},
            ],
        });
        let out = enrich_models_list(list, &sizes);
        assert_eq!(out["data"][0]["context_length"], 1048576);
        assert_eq!(out["data"][0]["context_window"], 1048576);
        assert_eq!(out["data"][0]["id"], "muse-spark-1.3-contributor-free");
        assert_eq!(out["data"][1]["context_length"], 128_000);
    }

    #[test]
    fn detects_responses_family() {
        assert!(is_responses_model("muse-spark-1.3-contributor-free"));
        assert!(is_responses_model("gpt-5.5"));
        assert!(is_responses_model("grok-4"));
        assert!(!is_responses_model("nemotron-3.5-lightning-free"));
        assert!(!is_responses_model("claude-opus-4-5"));
    }

    #[test]
    fn chat_to_responses_body() {
        let chat = serde_json::json!({
            "model": "zen/muse-spark-1.3-contributor-free",
            "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": "hi"},
            ],
            "max_tokens": 50,
            "stream": true,
        });
        let body = responses_body_from_chat(&chat);
        assert_eq!(body["model"], "muse-spark-1.3-contributor-free");
        assert_eq!(body["input"], "system: be brief\nhi");
        assert_eq!(body["max_output_tokens"], 16384);
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn responses_to_chat_completion() {
        let resp = serde_json::json!({
            "id": "resp_1",
            "created_at": 123,
            "model": "muse-spark-1.3-contributor-free",
            "output": [
                {"type": "reasoning", "summary": [{"text": "thinking"}]},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "OK"},
                ]},
            ],
            "usage": {"input_tokens": 5, "output_tokens": 7, "total_tokens": 12},
        });
        let chat = chat_from_responses("muse-spark-1.3-contributor-free", &resp);
        assert_eq!(chat["object"], "chat.completion");
        assert_eq!(chat["choices"][0]["message"]["content"], "OK");
        assert_eq!(
            chat["choices"][0]["message"]["reasoning_content"],
            "thinking"
        );
        assert_eq!(chat["usage"]["total_tokens"], 12);
    }

    #[test]
    fn sse_events_become_chat_chunks() {
        let mut map = SseChatMap::new("muse-spark-1.3-contributor-free");
        let created = serde_json::json!({
            "type": "response.created",
            "response": {"id": "resp_9", "created_at": 42},
        });
        assert!(map.map_event(&created).is_empty());
        let delta = serde_json::json!({"type": "response.output_text.delta", "delta": "OK"});
        let lines = map.map_event(&delta);
        assert_eq!(lines.len(), 2, "role chunk + content chunk");
        assert!(lines[1].contains("\"content\":\"OK\""), "{}", lines[1]);
        let done = serde_json::json!({"type": "response.completed",
            "response": {"incomplete_details": null}});
        let lines = map.map_event(&done);
        assert!(
            lines[0].contains("\"finish_reason\":\"stop\""),
            "{}",
            lines[0]
        );
    }
}
