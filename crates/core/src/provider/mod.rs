use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const MAX_SSE_BUF: usize = 32 << 20;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
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
