// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const MAX_SSE_BUF: usize = 32 << 20;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Humanize a provider error body for chat + desktop notifications.
/// Upstream bodies are often raw JSON (`{"error":{"message":...}}`) —
/// showing that verbatim in a notification reads as "error as json".
/// Extracts the human message from common shapes, else first line,
/// truncated to `max` chars.
pub(crate) fn humanize_error_body(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        // {"error": {"message": "..."}} (OpenRouter, gateway, Gemini)
        if let Some(err) = v.get("error") {
            if let Some(msg) = err.get("message").and_then(|m| m.as_str()) {
                return first_line_capped(msg, max);
            }
            if let Some(s) = err.as_str() {
                return first_line_capped(s, max);
            }
        }
        for key in ["message", "detail", "msg"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
                return first_line_capped(s, max);
            }
        }
    }
    first_line_capped(trimmed, max)
}

fn first_line_capped(s: &str, max: usize) -> String {
    s.lines()
        .next()
        .unwrap_or(s)
        .trim()
        .chars()
        .take(max)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProviderEvent {
    TextDelta(String),
    ToolCallDelta {
        id: String,
        name: String,
        args: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    ToolResultDelta {
        id: String,
        content: String,
        ok: bool,
    },
    ReasoningDelta(String),

    Notice(String),
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,

    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    pub fn text(role: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: Value::String(text.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn with_image(role: impl Into<String>, text: impl Into<String>, base64_png: &str) -> Self {
        Self::with_image_mime(role, text, "image/png", base64_png)
    }
    pub fn with_image_mime(
        role: impl Into<String>,
        text: impl Into<String>,
        mime: &str,
        b64: &str,
    ) -> Self {
        let url = format!("data:{mime};base64,{b64}");
        Self {
            role: role.into(),
            content: serde_json::json!([
                {"type": "text", "text": text.into()},
                {"type": "image_url", "image_url": {"url": url}}
            ]),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
    pub fn content_len(&self) -> usize {
        let mut n = match &self.content {
            Value::String(s) => s.len(),
            other => other.to_string().len(),
        };
        if let Some(tcs) = &self.tool_calls {
            for tc in tcs {
                n += tc.function.name.len() + tc.function.arguments.len() + tc.id.len();
                if let Some(sig) = &tc.thought_signature {
                    n += sig.len();
                }
            }
        }
        if let Some(id) = &self.tool_call_id {
            n += id.len();
        }
        if let Some(name) = &self.name {
            n += name.len();
        }
        n
    }
    pub fn content_as_str(&self) -> String {
        match &self.content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefForProvider {
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefForProvider>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Reasoning-depth dial (`off|minimal|low|medium|high|xhigh|max`).
    /// Never serialized directly — each body builder maps it onto the
    /// vendor's own knob. `None` means the provider default.
    #[serde(default, skip_serializing)]
    pub thinking_level: Option<String>,
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    async fn stream(
        &self,
        req: ChatRequest,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>>;
    async fn complete(&self, req: ChatRequest) -> anyhow::Result<String>;
}

pub mod catalog;
pub mod gemini;
pub mod opencode;
pub mod openrouter;

pub fn provider_for_model(model: &str) -> Box<dyn Provider> {
    if opencode::is_opencode_model(model) {
        return Box::new(opencode::OpenCodeProvider::from_env_for_model(model));
    }
    let is_gemini =
        model.starts_with("google/") || model.starts_with("gemini") || model.contains("gemini-");
    if is_gemini {
        Box::new(gemini::GeminiProvider::from_env())
    } else {
        Box::new(openrouter::OpenRouterProvider::from_env())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_is_strict_no_fallback() {
        for m in [
            "opencode/gpt-x",
            "zen/claude-y",
            "go/kimi-z",
            "anything-free",
        ] {
            let p = provider_for_model(m);
            assert!(
                p.name() == "gateway" || p.name() == "gateway-go",
                "{m} -> {}",
                p.name()
            );
        }
        for m in [
            "google/gemini-2.0-flash",
            "gemini-2.0-flash",
            "gemini-flash-lite",
            "x-gemini-y",
        ] {
            assert_eq!(provider_for_model(m).name(), "gemini", "{m}");
        }
        for m in [
            "openai/gpt-4o",
            "anthropic/claude-sonnet",
            "mystery/model-1",
            "",
        ] {
            assert_eq!(provider_for_model(m).name(), "openrouter", "{m}");
        }
    }

    #[test]
    fn chat_message_image_shape() {
        let m = ChatMessage::with_image_mime("user", "see this", "image/png", "QUJD");
        let arr = m.content.as_array().expect("multipart");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], serde_json::json!("text"));
        let url = arr[1]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(url.ends_with("QUJD"));
        let m2 = ChatMessage::with_image("user", "t", "QUJD");
        assert!(m2.content.to_string().contains("image/png"));
    }

    #[test]
    fn content_len_and_str() {
        let m = ChatMessage::text("user", "hello");
        assert_eq!(m.content_as_str(), "hello");
        assert_eq!(m.content_len(), 5);
        let mut m2 = ChatMessage::text("assistant", "");
        m2.tool_calls = Some(vec![ToolCall {
            id: "c1".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read".into(),
                arguments: "{}".into(),
            },
            thought_signature: Some("sig".into()),
        }]);
        m2.tool_call_id = Some("c1".into());
        m2.name = Some("read".into());
        assert!(m2.content_len() > "read".len());
        let img = ChatMessage::with_image("user", "t", "QUJD");
        assert!(img.content_as_str().contains("image_url"));
    }

    #[test]
    fn humanize_extracts_message_from_json() {
        let json = r#"{"error":{"message":"No endpoints found for free-tier","code":429}}"#;
        assert_eq!(
            humanize_error_body(json, 300),
            "No endpoints found for free-tier"
        );
        let json2 = r#"{"error": "boom"}"#;
        assert_eq!(humanize_error_body(json2, 300), "boom");
        let json3 = r#"{"message": "quota exceeded"}"#;
        assert_eq!(humanize_error_body(json3, 300), "quota exceeded");
        let plain = "plain failure\nsecond line";
        assert_eq!(humanize_error_body(plain, 300), "plain failure");
        // Never leaks braces for the notification path.
        assert!(!humanize_error_body(json, 300).contains('{'));
    }
}
