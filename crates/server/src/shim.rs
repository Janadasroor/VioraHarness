//! Free-tier gateway shim for clients that only accept URL + API key.
//!
//! Forwards OpenAI-compatible requests to the Zen gateway while injecting
//! the session header the free tier requires and stripping provider id
//! prefixes (`zen/`, `opencode/`, …) down to the bare model id.
//! Response bodies (including SSE streams) pass through untouched.

use axum::{
    body::Body,
    extract::{OriginalUri, Request},
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
        .trim_start_matches('/');
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

async fn forward(OriginalUri(uri): OriginalUri, req: Request) -> Response {
    let rel = upstream_path(uri.path());
    if rel.is_empty() {
        return Json(serde_json::json!({
            "object": "shim",
            "upstream": UPSTREAM,
            "usage": "point an OpenAI-compatible client at /v1 with any key",
        }))
        .into_response();
    }
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
    let send_body = if is_json {
        rewrite_model(&bytes).unwrap_or_else(|| bytes.to_vec())
    } else {
        bytes.to_vec()
    };

    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .unwrap_or(reqwest::Method::POST);
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

    let client = reqwest::Client::new();
    let upstream = match client
        .request(method, &url)
        .headers(fwd)
        .body(send_body)
        .send()
        .await
    {
        Ok(r) => r,
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
    let app = Router::new()
        .route("/", any(forward))
        .route("/*rest", any(forward));
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
}
