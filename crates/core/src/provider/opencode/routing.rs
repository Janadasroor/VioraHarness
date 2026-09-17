// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub(crate) const ZEN_BASE: &str = "https://opencode.ai/zen/v1";
pub(crate) const GO_BASE: &str = "https://opencode.ai/zen/go/v1";

pub(crate) fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
) -> Option<std::time::Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;

    if let Ok(s) = v.trim().parse::<u64>() {
        return Some(std::time::Duration::from_secs(s.min(300)));
    }
    None
}

pub(crate) fn is_free_quota_body(body: &str) -> bool {
    let b = body.to_lowercase();
    b.contains("freeusage")
        || b.contains("free usage")
        || b.contains("free tier")
        || b.contains("quota")
        || b.contains("exhausted")
}

pub(crate) fn is_auth_body(body: &str) -> bool {
    body.contains("Invalid API key")
        || body.contains("AuthError")
        || body.contains("Invalid")
        || body.contains("Auth")
}

pub(crate) fn http_retry_delays(
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

pub(crate) fn transport_retry_delays() -> Vec<std::time::Duration> {
    vec![1, 2, 4, 8]
        .into_iter()
        .map(std::time::Duration::from_secs)
        .collect()
}

pub(crate) fn jitter_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % 1000)
        .unwrap_or(0)
}

pub(crate) async fn send_with_retry(
    build: impl Fn() -> reqwest::RequestBuilder,
    kind: &str,
    notices: Option<&tokio::sync::mpsc::Sender<super::super::ProviderEvent>>,
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
                    let _ = tx.send(super::super::ProviderEvent::Notice(msg)).await;
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
                    let _ = tx.send(super::super::ProviderEvent::Notice(msg)).await;
                }
                tokio::time::sleep(d).await;
                waited += d.as_secs();
                attempt += 1;
            }
        }
    }
}

pub(crate) fn coalesce_pending(
    mut entries: Vec<(String, String, String)>,
) -> Vec<(String, String, String)> {
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

pub(crate) fn opencode_api_key() -> String {
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

pub(crate) fn opencode_base_for_model(model: &str) -> String {
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

pub(crate) fn strip_model_prefix(model: &str) -> String {
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
pub(crate) enum EndpointKind {
    Chat,
    Responses,
    Messages,
    Google,
}

pub(crate) fn endpoint_for_model(bare: &str, base_is_go: bool) -> EndpointKind {
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
