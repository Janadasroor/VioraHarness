// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::{ChatRequest, Provider, ProviderEvent};
use reqwest::Client;

mod chat;
mod convert;
mod messages;
mod responses;
mod routing;
use routing::{
    endpoint_for_model, opencode_api_key, opencode_base_for_model, strip_model_prefix, EndpointKind,
};
pub use routing::{is_free_model, is_free_model_dynamic, is_opencode_model};

pub struct OpenCodeProvider {
    pub(crate) api_key: String,
    pub(crate) base_url: String,
    pub(crate) client: Client,

    pub(crate) _orig_model: String,
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

    pub(crate) fn effective_key(&self, model: &str) -> String {
        if !self.api_key.is_empty() {
            return self.api_key.clone();
        }

        if is_free_model(model) {
            return "public".to_string();
        }
        String::new()
    }

    pub(crate) fn session_header() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("vioraharness-{:x}", ns & 0xffffff)
    }
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

impl OpenCodeProvider {}

#[cfg(test)]
mod tests {
    use super::convert::*;
    use super::routing::*;
    use super::*;
    use crate::provider::ChatMessage;
    use serde_json::json;

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
            thinking_level: None,
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

    #[tokio::test]
    async fn responses_reasoning_text_delta_reaches_label() {
        // Models/gateways that stream full reasoning text (not summaries)
        // must still feed the thinking block — previously these deltas
        // were dropped, so thinking vanished the moment the turn ended.
        let raw = b"event: response.reasoning_text.delta\ndata: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"Weigh\"}\n\n\
event: response.reasoning_text.delta\ndata: {\"type\":\"response.reasoning_text.delta\",\"delta\":\" options\"}\n\n\
event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\n\
event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Weigh options\"}]},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"done\"}]}]}}\n\n".to_vec();
        for chunk in [1, 7, 64] {
            let evs = replay_sse(raw.clone(), chunk).await;
            let (text, reasoning) = replay_text(&evs);
            assert_eq!(text, "done", "chunk {chunk}: message text once: {text:?}");
            assert_eq!(
                reasoning, "Weigh options",
                "chunk {chunk}: live reasoning deltas, no completed dup: {reasoning:?}"
            );
        }

        // Fallback shape: full reasoning text under content[].
        let raw2 = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"reasoning\",\"content\":[{\"type\":\"reasoning_text\",\"text\":\"Late think\"}]},{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}]}}\n\n".to_vec();
        let evs2 = replay_sse(raw2.clone(), 64).await;
        let (_, reasoning2) = replay_text(&evs2);
        assert_eq!(
            reasoning2, "Late think",
            "content[] fallback: {reasoning2:?}"
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
            thinking_level: None,
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
            thinking_level: None,
        };
        let body = chat_to_messages_body(&req, "claude-test-4-5");
        assert_eq!(body["model"], json!("claude-test-4-5"));
        assert!(body["messages"].is_array());
    }

    #[test]
    fn responses_effort_follows_thinking_level() {
        let mut req = ChatRequest {
            model: "opencode/gpt-test-1".into(),
            messages: vec![ChatMessage::text("user", "hello")],
            tools: None,
            tool_choice: None,
            max_tokens: Some(20),
            temperature: Some(0.7),
            thinking_level: None,
        };
        // Unset dial keeps the provider default.
        assert_eq!(
            chat_to_responses_body(&req, "gpt-test-1")["reasoning"]["effort"],
            json!("medium")
        );
        for (level, want) in [
            ("off", "none"),
            ("minimal", "minimal"),
            ("low", "low"),
            ("high", "high"),
            ("xhigh", "xhigh"),
            ("max", "max"),
        ] {
            req.thinking_level = Some(level.into());
            assert_eq!(
                chat_to_responses_body(&req, "gpt-test-1")["reasoning"]["effort"],
                json!(want),
                "level {level}"
            );
        }
    }

    #[test]
    fn messages_thinking_budget_fits_under_output_limit() {
        let mut req = ChatRequest {
            model: "opencode/claude-test-4-5".into(),
            messages: vec![ChatMessage::text("user", "hello")],
            tools: None,
            tool_choice: None,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            thinking_level: None,
        };
        // Unset dial behaves like medium.
        let body = chat_to_messages_body(&req, "claude-test-4-5");
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap();
        let max_out = body["max_tokens"].as_u64().unwrap();
        assert!(budget >= 1024 && budget < max_out);
        // `off` restores the exact old shape: no knob, tiny ceiling.
        req.thinking_level = Some("off".into());
        let off = chat_to_messages_body(&req, "claude-test-4-5");
        assert!(off.get("thinking").is_none());
        assert_eq!(off["max_tokens"], json!(1024));
    }
}
