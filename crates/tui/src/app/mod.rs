use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use std::collections::HashSet;
use std::time::Duration;
mod background;
mod clipboard;
mod commands;
mod format;
mod image;
mod input;
mod keys;
mod latex;
mod layout;
mod markdown;
mod msg;
mod popups;
mod render;
pub(crate) mod settings;
mod skills;
#[cfg(test)]
mod testkit;
mod turn;
pub(crate) use clipboard::*;
pub(crate) use format::*;
pub(crate) use image::*;
pub(crate) use input::*;
pub(crate) use layout::*;
pub(crate) use markdown::*;
pub(crate) use msg::*;
pub(crate) use skills::*;

type ModelFetchRx =
    tokio::sync::mpsc::Receiver<(Vec<String>, std::collections::HashMap<String, usize>)>;

pub struct App {
    pub model: String,
    pub session_id: String,
    pub messages: Vec<Msg>,
    pub input: InputState,
    pub scroll: usize,
    pub status: String,
    pub should_quit: bool,
    pub busy: bool,
    pub(crate) pending: Option<tokio::task::JoinHandle<anyhow::Result<String>>>,
    pub(crate) stream_rx:
        Option<tokio::sync::mpsc::Receiver<vioraharness_core::provider::ProviderEvent>>,
    pub(crate) popup: Popup,
    pub(crate) model_cursor: usize,
    pub(crate) available_models: Vec<String>,
    pub(crate) model_filter: String,
    pub(crate) tick: usize,
    pub(crate) streaming_buf: String,
    pub(crate) thinking_buf: String,
    pub(crate) model_fetch_rx: Option<ModelFetchRx>,

    pub(crate) thinking_expanded: bool,
    pub(crate) thinking_title: String,
    pub(crate) thinking_label: String,
    pub(crate) show_thinking: bool,
    /// Reasoning-depth dial (`off|minimal|low|medium|high|xhigh|max`).
    /// Shown next to the model name; `Ctrl+T` cycles it. Flows into
    /// every provider request via `resolve_thinking_level`.
    pub(crate) thinking_level: String,

    pub(crate) tool_display_default: String,
    pub(crate) tool_display: std::collections::HashMap<String, String>,

    pub(crate) expanded_reasoning: HashSet<usize>,
    pub(crate) chat_search: Option<String>,

    pub(crate) theme_cursor: usize,
    pub(crate) available_themes: Vec<String>,

    pub(crate) settings_cursor: usize,

    pub(crate) session_cursor: usize,
    pub(crate) task_cursor: usize,
    pub(crate) error_cursor: usize,
    pub(crate) rewind_cursor: usize,
    pub(crate) rewind_armed: Option<i64>,
    pub(crate) rewind_armed_note: Option<String>,
    pub(crate) session_filter: String,
    pub(crate) show_all_sessions: bool,

    pub(crate) provider_cursor: usize,
    pub(crate) provider_key_input: String,
    pub(crate) provider_input_active: bool,
    pub(crate) provider_selected: Option<String>,
    pub(crate) provider_validating: bool,
    pub(crate) provider_msg: Option<(String, bool)>,

    pub(crate) pending_image: Option<PendingImage>,

    pub(crate) pending_texts: Vec<(usize, String)>,
    pub(crate) paste_seq: usize,

    pub(crate) expanded_messages: HashSet<(usize, Option<usize>)>,

    pub(crate) model_context: std::collections::HashMap<String, usize>,

    pub(crate) mode: String,

    /// Agent working mode (`eda` = full tools, `web` = web-dev subset).
    /// Distinct from `mode` (plan/build input toggle). Persisted as
    /// `last_mode` in tui_state.json; per-session value wins on /resume.
    pub(crate) agent_mode: String,
    pub(crate) mode_cursor: usize,

    pub(crate) last_diff: Option<String>,

    pub(crate) pending_perm: Option<vioraharness_core::permissions::InteractiveAsk>,
    pub(crate) perm_cursor: usize,
    pub(crate) perm_rx:
        Option<tokio::sync::mpsc::Receiver<vioraharness_core::permissions::InteractiveAsk>>,

    pub(crate) pending_q: Option<vioraharness_core::permissions::PendingQuestion>,
    pub(crate) q_cursor: usize,
    pub(crate) q_answered: Vec<vioraharness_core::permissions::QuestionAnswer>,
    pub(crate) q_toggled: Vec<usize>,
    pub(crate) q_custom: String,
    pub(crate) q_custom_active: bool,
    pub(crate) q_rx:
        Option<tokio::sync::mpsc::Receiver<vioraharness_core::permissions::PendingQuestion>>,

    pub(crate) running_tool: Option<(String, String, String, std::time::Instant)>,

    pub(crate) compact_rx: Option<(
        String,
        tokio::sync::oneshot::Receiver<
            Result<vioraharness_core::context::compaction::CompactReport, String>,
        >,
    )>,

    pub(crate) ctx_freed_tokens: usize,

    pub(crate) wake_on_tasks: bool,

    pub(crate) queued_prompts: Vec<QueuedPrompt>,

    /// `$` instant-prompt handle for the live turn (cloned out of its
    /// `AgentLoop` in `start_turn`). `None` when no turn is running.
    pub(crate) instant_injector: Option<vioraharness_core::loop_mod::Injector>,
    /// Session the live turn belongs to. `$` falls back to the normal queue
    /// when this differs from the current session.
    pub(crate) turn_session: Option<String>,

    pub(crate) last_tool_output: Option<(String, String, String, bool)>,
    pub(crate) tool_output_scroll: usize,

    pub(crate) chat_total_lines: usize,

    pub(crate) selection: Option<Selection>,
    pub(crate) dragging: bool,
    pub(crate) input_drag: bool,
    pub(crate) copy_pending: bool,
    /// Finished async clipboard copy: status line to show. Polled with
    /// try_recv — the helper thread owns any X11/subprocess stall.
    pub(crate) copy_rx: Option<std::sync::mpsc::Receiver<String>>,
    /// Finished async clipboard paste probe. Same non-blocking contract.
    pub(crate) paste_rx: Option<std::sync::mpsc::Receiver<PasteDone>>,

    pub(crate) chat_area: Rect,
    pub(crate) input_area: Rect,
    pub(crate) view_start: usize,
    pub(crate) view_total: usize,

    pub(crate) vis_rows: Vec<(usize, usize)>,
}

impl App {
    fn fetch_models_from_openrouter() -> Vec<String> {
        let has_openrouter = has_key("OPENROUTER_API_KEY");
        let has_gemini = has_key("GEMINI_API_KEY");

        let is_gateway_model = |m: &str| {
            m.starts_with("opencode/")
                || m.starts_with("opencode-go/")
                || m.starts_with("zen/")
                || m.starts_with("go/")
                || vioraharness_core::provider::opencode::is_free_model(m)
        };

        let filter_by_keys = |list: Vec<String>| -> Vec<String> {
            list.into_iter()
                .filter(|m| {
                    if is_gateway_model(m) {
                        true
                    } else {
                        let is_gemini = m.contains("gemini")
                            || m.starts_with("google/")
                            || m.starts_with("gemini-");
                        if is_gemini {
                            has_gemini
                        } else {
                            has_openrouter
                        }
                    }
                })
                .collect()
        };
        let normalize_gemini = |m: String| -> String {
            if m.starts_with("google/") || m.contains('/') {
                m
            } else if m.starts_with("gemini-") {
                format!("google/{m}")
            } else {
                m
            }
        };

        let mut json_models: Vec<String> = Vec::new();
        for path in vioraharness_core::loop_mod::config_candidates() {
            if let Ok(s) = std::fs::read_to_string(path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("openrouter"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(normalize_gemini(x.to_string()));
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("gemini"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(normalize_gemini(x.to_string()));
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("opencode"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(x.to_string());
                        }
                    }

                    if let Some(arr) = v
                        .get("provider")
                        .and_then(|p| p.get("opencode-go"))
                        .and_then(|o| o.get("models"))
                        .and_then(|m| m.as_array())
                    {
                        for x in arr.iter().filter_map(|x| x.as_str()) {
                            json_models.push(x.to_string());
                        }
                    }
                }
            }
        }
        json_models.sort();
        json_models.dedup();
        let filtered = filter_by_keys(json_models);
        if !filtered.is_empty() {
            tracing::info!(
                "fetch_models_from_openrouter: {} from vioraharness.json after key filter",
                filtered.len()
            );
            return filtered;
        }

        filtered
    }

    async fn fetch_models_live() -> (Vec<String>, std::collections::HashMap<String, usize>) {
        let fallback = Self::fetch_models_from_openrouter();
        let has_openrouter = has_key("OPENROUTER_API_KEY");
        let has_gemini = has_key("GEMINI_API_KEY");
        let has_gateway = has_key("OPENCODE_API_KEY") || has_key("ZEN_API_KEY");
        tracing::info!(
            "fetch_models_live: has_openrouter={} has_gemini={} has_gateway={} fallback={} ",
            has_openrouter,
            has_gemini,
            has_gateway,
            fallback.len()
        );

        let mut merged: Vec<String> = fallback.clone();
        let mut context_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for m in &fallback {
            context_map.entry(m.clone()).or_insert(128_000);
        }

        // Seed live context sizes from the disk cache first: if the live
        // catalog fetch flakes (it did — 200 OK with a truncated body),
        // the picker still shows real sizes instead of 128k fallbacks.
        // Live values overwrite these below.
        for (id, size) in vioraharness_core::provider::catalog::openrouter_context_cached().await {
            context_map.insert(id, size);
        }

        if has_openrouter {
            match std::env::var("OPENROUTER_API_KEY") {
                Ok(key) => {
                    match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(15))
                        .build()
                    {
                        Ok(client) => {
                            tracing::info!("fetch_models_live: curl OpenRouter GET https://openrouter.ai/api/v1/models Bearer {}...{}", &key[..4.min(key.len())], &key[key.len().saturating_sub(4)..]);
                            // The catalog body is megabytes of JSON; a
                            // truncated stream decodes as 200 OK + parse
                            // failure (seen live). Retry transport/decode,
                            // not HTTP error statuses.
                            let mut fetched: Option<serde_json::Value> = None;
                            for attempt in 1..=3 {
                                match client
                                    .get("https://openrouter.ai/api/v1/models")
                                    .header("Authorization", format!("Bearer {key}"))
                                    .send()
                                    .await
                                {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        tracing::info!(
                                            "fetch_models_live: OpenRouter status {} (attempt {attempt})",
                                            status
                                        );
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!(
                                                "fetch_models_live: OpenRouter error {} — {}",
                                                status,
                                                txt.chars().take(300).collect::<String>()
                                            );
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                fetched = Some(json);
                                                break;
                                            }
                                            Err(e) => tracing::warn!(
                                                "fetch_models_live: OpenRouter json parse failed (attempt {attempt}): {e}"
                                            ),
                                        }
                                    }
                                    Err(e) => tracing::warn!(
                                        "fetch_models_live: OpenRouter request failed (attempt {attempt}): {e}"
                                    ),
                                }
                                if attempt < 3 {
                                    tokio::time::sleep(std::time::Duration::from_secs(
                                        attempt as u64,
                                    ))
                                    .await;
                                }
                            }
                            if let Some(json) = fetched {
                                if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                    let mut out: Vec<String> = Vec::new();
                                    for v in data.iter() {
                                        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                            let id_s = id.to_string();
                                            out.push(id_s.clone());
                                            if let Some(cl) =
                                                v.get("context_length").and_then(|x| x.as_u64())
                                            {
                                                context_map.insert(id_s.clone(), cl as usize);
                                            } else if let Some(cl2) = v
                                                .get("context_length")
                                                .and_then(|x| x.as_str())
                                                .and_then(|s| s.parse::<u64>().ok())
                                            {
                                                context_map.insert(id_s.clone(), cl2 as usize);
                                            }

                                            if let Some(tp) = v
                                                .get("top_provider")
                                                .and_then(|x| x.get("context_length"))
                                                .and_then(|x| x.as_u64())
                                            {
                                                context_map.entry(id_s).or_insert(tp as usize);
                                            }
                                        }
                                    }
                                    let before = out.len();
                                    out.retain(|m| {
                                        !m.contains("embedding") && !m.contains("moderation")
                                    });
                                    tracing::info!("fetch_models_live: OpenRouter {} models ({} after filter embedding/moderation)", before, out.len());
                                    merged.extend(out);
                                } else {
                                    tracing::warn!("fetch_models_live: OpenRouter no data array");
                                }
                            } else {
                                tracing::warn!("fetch_models_live: OpenRouter catalog unavailable after 3 attempts");
                            }
                        }
                        Err(e) => tracing::warn!(
                            "fetch_models_live: OpenRouter client build failed: {}",
                            e
                        ),
                    }
                }
                Err(_) => tracing::warn!("fetch_models_live: OPENROUTER_API_KEY not set"),
            }
        }

        {
            let known: Vec<(String, usize)> = fallback
                .iter()
                .filter_map(|m| {
                    vioraharness_core::provider::catalog::match_or_context(m, &context_map)
                        .map(|s| (m.clone(), s))
                })
                .collect();
            for (m, s) in known {
                context_map.insert(m, s);
            }
        }

        if has_gemini {
            match std::env::var("GEMINI_API_KEY") {
                Ok(key) => {
                    let prefix = &key[..4.min(key.len())];
                    let suffix = &key[key.len().saturating_sub(4)..];
                    let mut gemini_fetched: Vec<String> = Vec::new();
                    let mut gemini_ok = false;

                    match reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .build()
                    {
                        Ok(client) => {
                            let url =
                                "https://generativelanguage.googleapis.com/v1beta/openai/models";
                            tracing::info!("fetch_models_live: curl Gemini openai compat GET {} Authorization: Bearer {}...{}", url, prefix, suffix);
                            match client
                                .get(url)
                                .header("Authorization", format!("Bearer {key}"))
                                .send()
                                .await
                            {
                                Ok(resp) => {
                                    let status = resp.status();
                                    tracing::info!(
                                        "fetch_models_live: Gemini openai compat status {}",
                                        status
                                    );
                                    if status.is_success() {
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
                                                    for v in data {
                                                        if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                            let short = id.trim_start_matches("models/").to_string();
                                                            let norm = if short.starts_with("gemini-") || short.starts_with("gemma-") || short.starts_with("veo-") || short.starts_with("nano-") || short.starts_with("lyria-") { format!("google/{short}") } else { short };

                                                            gemini_fetched.push(norm);
                                                        }
                                                    }
                                                    tracing::info!("fetch_models_live: Gemini openai compat fetched {} models", gemini_fetched.len());
                                                    gemini_ok = !gemini_fetched.is_empty();
                                                }
                                            }
                                            Err(e) => tracing::warn!("fetch_models_live: Gemini openai compat json parse {}", e),
                                        }
                                    } else {
                                        let txt = resp.text().await.unwrap_or_default();
                                        tracing::warn!(
                                            "fetch_models_live: Gemini openai compat error {} — {}",
                                            status,
                                            txt.chars().take(300).collect::<String>()
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!(
                                    "fetch_models_live: Gemini openai compat request failed: {}",
                                    e
                                ),
                            }
                        }
                        Err(e) => {
                            tracing::warn!("fetch_models_live: Gemini client build failed: {}", e)
                        }
                    }

                    if !gemini_ok {
                        gemini_fetched.clear();
                        if let Ok(client) = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                        {
                            let mut page_token: Option<String> = None;
                            let mut pages = 0;
                            loop {
                                pages += 1;
                                let mut url = "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000".to_string();
                                if let Some(tok) = &page_token {
                                    url = format!("{url}&pageToken={tok}");
                                }
                                tracing::info!("fetch_models_live: curl Gemini native GET {} x-goog-api-key: {}...{} (page {})", url, prefix, suffix, pages);
                                match client.get(&url).header("x-goog-api-key", &key).send().await {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!("fetch_models_live: Gemini native header error {} — {}", status, txt.chars().take(300).collect::<String>());
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(models) =
                                                    json.get("models").and_then(|m| m.as_array())
                                                {
                                                    for v in models {
                                                        if let Some(name) =
                                                            v.get("name").and_then(|n| n.as_str())
                                                        {
                                                            let short = name
                                                                .trim_start_matches("models/")
                                                                .to_string();
                                                            let id = if short.starts_with("gemini-")
                                                                || short.starts_with("gemma-")
                                                            {
                                                                format!("google/{short}")
                                                            } else {
                                                                short
                                                            };
                                                            gemini_fetched.push(id.clone());
                                                            if let Some(limit) = v
                                                                .get("inputTokenLimit")
                                                                .and_then(|x| x.as_u64())
                                                            {
                                                                context_map.insert(
                                                                    id.clone(),
                                                                    limit as usize,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Some(next) = json
                                                    .get("nextPageToken")
                                                    .and_then(|t| t.as_str())
                                                {
                                                    if !next.is_empty() && pages < 5 {
                                                        page_token = Some(next.to_string());
                                                        continue;
                                                    }
                                                }
                                                break;
                                            }
                                            Err(e) => {
                                                tracing::warn!("fetch_models_live: Gemini native json parse {}", e);
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "fetch_models_live: Gemini native request failed: {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                            if !gemini_fetched.is_empty() {
                                tracing::info!("fetch_models_live: Gemini native header fetched {} models in {} pages", gemini_fetched.len(), pages);
                                gemini_ok = true;
                            }
                        }
                    }

                    if !gemini_ok {
                        gemini_fetched.clear();
                        if let Ok(client) = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                        {
                            let mut page_token: Option<String> = None;
                            let mut pages = 0;
                            loop {
                                pages += 1;
                                let mut url = format!("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000&key={key}");
                                if let Some(tok) = &page_token {
                                    url = format!("{url}&pageToken={tok}");
                                }
                                tracing::info!("fetch_models_live: curl Gemini native GET {} (query key, page {})", url.chars().take(90).collect::<String>(), pages);
                                match client.get(&url).send().await {
                                    Ok(resp) => {
                                        let status = resp.status();
                                        if !status.is_success() {
                                            let txt = resp.text().await.unwrap_or_default();
                                            tracing::warn!("fetch_models_live: Gemini native query error {} — {}", status, txt.chars().take(300).collect::<String>());
                                            break;
                                        }
                                        match resp.json::<serde_json::Value>().await {
                                            Ok(json) => {
                                                if let Some(models) =
                                                    json.get("models").and_then(|m| m.as_array())
                                                {
                                                    for v in models {
                                                        if let Some(name) =
                                                            v.get("name").and_then(|n| n.as_str())
                                                        {
                                                            let short = name
                                                                .trim_start_matches("models/")
                                                                .to_string();
                                                            let id = if short.starts_with("gemini-")
                                                                || short.starts_with("gemma-")
                                                            {
                                                                format!("google/{short}")
                                                            } else {
                                                                short
                                                            };
                                                            gemini_fetched.push(id.clone());
                                                            if let Some(limit) = v
                                                                .get("inputTokenLimit")
                                                                .and_then(|x| x.as_u64())
                                                            {
                                                                context_map.insert(
                                                                    id.clone(),
                                                                    limit as usize,
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Some(next) = json
                                                    .get("nextPageToken")
                                                    .and_then(|t| t.as_str())
                                                {
                                                    if !next.is_empty() && pages < 5 {
                                                        page_token = Some(next.to_string());
                                                        continue;
                                                    }
                                                }
                                                break;
                                            }
                                            Err(e) => {
                                                tracing::warn!("fetch_models_live: Gemini native query json {}", e);
                                                break;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "fetch_models_live: Gemini native query request {}",
                                            e
                                        );
                                        break;
                                    }
                                }
                            }
                            if !gemini_fetched.is_empty() {
                                tracing::info!(
                                    "fetch_models_live: Gemini native query fetched {} models",
                                    gemini_fetched.len()
                                );
                                gemini_ok = true;
                            }
                        }
                    }
                    if gemini_ok {
                        tracing::info!(
                            "fetch_models_live: Gemini final fetched {} (sample: {:?})",
                            gemini_fetched.len(),
                            gemini_fetched.iter().take(5).cloned().collect::<Vec<_>>()
                        );
                        merged.extend(gemini_fetched);
                    } else {
                        tracing::warn!("fetch_models_live: Gemini all endpoints failed — check GEMINI_API_KEY and run curl isolation");
                    }
                }
                Err(_) => tracing::warn!("fetch_models_live: GEMINI_API_KEY not set"),
            }
        }

        let or_sizes: std::collections::HashMap<String, usize> = context_map.clone();

        {
            let key_opt = std::env::var("OPENCODE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ZEN_API_KEY").ok());
            let has_key = key_opt
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);

            let zen_urls = ["https://opencode.ai/zen/v1/models"];
            for url in zen_urls {
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(4))
                    .build()
                {
                    tracing::info!(
                        "fetch_models_live: curl Zen GET {} {}",
                        url,
                        if has_key { "with key" } else { "public" }
                    );
                    let mut reqb = client.get(url);
                    if has_key {
                        reqb = reqb.header(
                            "Authorization",
                            format!("Bearer {}", key_opt.as_ref().unwrap()),
                        );
                    }
                    match reqb.send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<serde_json::Value>().await {
                                Ok(json) => {
                                    if let Some(data) = json.get("data").and_then(|d| d.as_array())
                                    {
                                        let mut out: Vec<String> = Vec::new();
                                        for v in data {
                                            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                let full = format!("opencode/{}", id);
                                                out.push(full.clone());

                                                let size = vioraharness_core::provider::catalog::match_or_context(
                                                    &full, &or_sizes,
                                                )
                                                .unwrap_or(128_000);
                                                context_map.entry(full.clone()).or_insert(size);

                                                context_map.entry(id.to_string()).or_insert(size);
                                            }
                                        }
                                        tracing::info!("fetch_models_live: Zen fetched {} models (sample {:?})", out.len(), out.iter().take(4).cloned().collect::<Vec<_>>());
                                        merged.extend(out);
                                    }
                                }
                                Err(e) => tracing::warn!("fetch_models_live: Zen json parse {}", e),
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            tracing::warn!(
                                "fetch_models_live: Zen error {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => tracing::warn!("fetch_models_live: Zen request failed {}", e),
                    }
                }
            }
        }

        {
            let key_opt = std::env::var("OPENCODE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ZEN_API_KEY").ok());
            let has_key = key_opt
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);

            let go_urls = ["https://opencode.ai/zen/go/v1/models"];
            for url in go_urls {
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(4))
                    .build()
                {
                    tracing::info!(
                        "fetch_models_live: curl Go GET {} {}",
                        url,
                        if has_key { "with key" } else { "public" }
                    );
                    let mut reqb = client.get(url);
                    if has_key {
                        reqb = reqb.header(
                            "Authorization",
                            format!("Bearer {}", key_opt.as_ref().unwrap()),
                        );
                    }
                    match reqb.send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<serde_json::Value>().await {
                                Ok(json) => {
                                    if let Some(data) = json.get("data").and_then(|d| d.as_array())
                                    {
                                        let mut out: Vec<String> = Vec::new();
                                        for v in data {
                                            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                                                let full = format!("opencode-go/{}", id);
                                                out.push(full.clone());
                                                let size = vioraharness_core::provider::catalog::match_or_context(
                                                    &full, &or_sizes,
                                                )
                                                .unwrap_or(128_000);
                                                context_map.entry(full.clone()).or_insert(size);
                                            }
                                        }
                                        tracing::info!(
                                            "fetch_models_live: Go fetched {} models (sample {:?})",
                                            out.len(),
                                            out.iter().take(4).cloned().collect::<Vec<_>>()
                                        );
                                        merged.extend(out);
                                    }
                                }
                                Err(e) => tracing::warn!("fetch_models_live: Go json parse {}", e),
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();

                            tracing::info!(
                                "fetch_models_live: Go status {} — {}",
                                status,
                                txt.chars().take(150).collect::<String>()
                            );
                        }
                        Err(e) => tracing::warn!("fetch_models_live: Go request failed {}", e),
                    }
                }
            }
        }
        if merged.is_empty() {
            tracing::warn!(
                "fetch_models_live: merged empty, returning fallback {} ",
                fallback.len()
            );
            return (fallback, context_map);
        }
        merged.retain(|m| {
            !m.to_lowercase().contains("embedding") && !m.to_lowercase().contains("moderation")
                || m.contains("gemini-embedding")
        });
        merged.sort();
        merged.dedup();
        tracing::info!(
            "fetch_models_live: merged total {} (after dedup, sample {:?})",
            merged.len(),
            merged.iter().take(8).cloned().collect::<Vec<_>>()
        );

        let prioritized: Vec<String> = {
            let gemini: Vec<String> = merged
                .iter()
                .filter(|m| m.contains("gemini") || m.contains("gemma"))
                .cloned()
                .collect();
            let gateway_models: Vec<String> = merged
                .iter()
                .filter(|m| m.starts_with("opencode/") || m.starts_with("opencode-go/"))
                .cloned()
                .collect();
            let others: Vec<String> = merged
                .iter()
                .filter(|m| {
                    !(m.contains("gemini")
                        || m.contains("gemma")
                        || m.starts_with("opencode/")
                        || m.starts_with("opencode-go/"))
                })
                .cloned()
                .collect();
            let mut out = gemini;
            out.extend(gateway_models);
            out.extend(others);
            out
        };

        let mut out = prioritized;
        out.truncate(200);
        tracing::info!(
            "fetch_models_live: after family-first truncate {} (gemini={}, gateway={})",
            out.len(),
            out.iter().any(|m| m.contains("gemini")),
            out.iter().any(|m| m.starts_with("opencode")),
        );
        if out.is_empty() {
            (fallback, context_map)
        } else {
            (out, context_map)
        }
    }
    fn tui_state_path() -> std::path::PathBuf {
        let base = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            });
        base.join("vioraharness/tui_state.json")
    }

    fn load_tui_state() -> serde_json::Value {
        let p = Self::tui_state_path();
        std::fs::read_to_string(p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}))
    }

    fn save_tui_state(patch: serde_json::Value) {
        let p = Self::tui_state_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut cur = Self::load_tui_state();
        if let (Some(cur_obj), Some(patch_obj)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in patch_obj {
                cur_obj.insert(k.clone(), v.clone());
            }
        }
        let _ = std::fs::write(p, serde_json::to_string_pretty(&cur).unwrap_or_default());
    }

    /// Mode stored on the last-opened chat (`last_session`, written on TUI
    /// exit in main.rs). A restart lands back in that chat's own mode
    /// instead of the global default. Best-effort: None on any failure.
    fn last_opened_chat_mode() -> Option<String> {
        let last_path = std::env::var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".local/share")
            })
            .join("vioraharness/last_session");
        let sid = std::fs::read_to_string(&last_path).ok()?;
        let sid = sid.trim();
        if sid.is_empty() {
            return None;
        }
        let db = std::env::var("VIORAHARNESS_DB")
            .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
        // Read-only lookup: never migrate/create here. App::new runs for
        // every test_app(), and a migrating open would contend with the
        // test that owns the temp DB file.
        vioraharness_core::session::read_session_mode(&db, sid)
    }

    fn tool_verbosity(&self, name: &str) -> ToolVerbosity {
        Self::verbosity_for(&self.tool_display, &self.tool_display_default, name)
    }

    /// Project hash of the process cwd — matches what `create_session`
    /// stores, so pickers can scope to this folder.
    pub(crate) fn project_filter_for_cwd() -> Option<String> {
        std::env::current_dir()
            .ok()
            .map(|p| vioraharness_core::session::project_hash_for_cwd(&p.to_string_lossy()))
    }

    /// Session-picker scope: current project by default, everything when
    /// the `p` toggle in `/sessions` is on.
    pub(crate) fn session_scope_filter(&self) -> Option<String> {
        if self.show_all_sessions {
            None
        } else {
            Self::project_filter_for_cwd()
        }
    }

    /// Single-line-cap preview for a tool result, or None when hidden.
    /// Shared by render and the Ctrl+X toggle so both agree on collapse.
    pub(crate) fn tool_result_preview(
        &self,
        items: &[Content],
        id: &str,
        content: &str,
        ok: bool,
    ) -> Option<String> {
        let tname = items
            .iter()
            .filter_map(|it| match it {
                Content::ToolCall { id: cid, name, .. } if cid == id => Some(name.as_str()),
                _ => None,
            })
            .next()
            .unwrap_or("");
        let mut verbosity = self.tool_verbosity(tname);
        if !ok && verbosity != ToolVerbosity::Full {
            verbosity = ToolVerbosity::Compact;
        }
        if verbosity == ToolVerbosity::Hidden {
            return None;
        }
        if verbosity == ToolVerbosity::Quiet {
            return Some(String::new());
        }
        // Read results never show content — `path (N lines)` like every
        // other agent. Full text stays in the result JSON for the model
        // and the /output viewer.
        if tname == "read" {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                // Surface real failures (e.g. `Is a directory`) instead of
                // the generic hidden-lines placeholder.
                if !ok {
                    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
                        if !e.is_empty() {
                            return Some(truncate_chars(e, 200));
                        }
                    }
                }
                // Vision reads carry dims, not line counts — reuse the
                // shared image summary (`path · WxH → vision`).
                if v.get("base64").is_some() || v.get("base64_len").is_some() {
                    let wide = verbosity == ToolVerbosity::Full;
                    return Some(pretty_tool_result_wide(content, wide));
                }
                let path = v
                    .get("path")
                    .and_then(|x| x.as_str())
                    .map(rel_path)
                    .unwrap_or_else(|| "file".to_string());
                let total = v.get("total_lines").and_then(|x| x.as_u64());
                let off = v.get("offset").and_then(|x| x.as_u64()).unwrap_or(0);
                let lim = v.get("limit").and_then(|x| x.as_u64()).unwrap_or(0);
                if let Some(t) = total {
                    if off != 0 || lim != 0 {
                        let shown = v
                            .get("content")
                            .and_then(|x| x.as_str())
                            .map(|c| c.lines().count() as u64)
                            .unwrap_or(0);
                        let end = (off + shown).min(t.max(1)).max(1);
                        return Some(format!("{path} (lines {}–{end} of {t})", off + 1));
                    }
                    return Some(format!("{path} ({t} lines)"));
                }
                // No line counts (bare read): show the path only,
                // never leak content onto the card.
                if ok {
                    return Some(path);
                }
            }
            return Some(if ok {
                "file read".to_string()
            } else {
                truncate_chars(content, 200)
            });
        }
        let wide = verbosity == ToolVerbosity::Full;
        let pretty = pretty_tool_result_wide(content, wide);
        let cap = if wide { 900 } else { 240 };
        Some(if pretty.chars().count() > cap {
            format!("{}…", truncate_chars(&pretty, cap))
        } else {
            pretty
        })
    }

    fn verbosity_for(
        map: &std::collections::HashMap<String, String>,
        default: &str,
        name: &str,
    ) -> ToolVerbosity {
        if let Some(l) = map.get(name) {
            return parse_tool_verbosity(l);
        }
        parse_tool_verbosity(default)
    }

    fn save_tool_display(&self) {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "default".into(),
            serde_json::Value::String(self.tool_display_default.clone()),
        );
        for (k, v) in &self.tool_display {
            obj.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        Self::save_tui_state(serde_json::json!({"tool_display": obj}));
    }

    /// Accumulate one reasoning delta for the running turn. Always
    /// stored — display is gated at render — so the finished assistant
    /// message keeps its thinking even when the dial was off mid-turn
    /// (matches the loop's unconditional DB persist). Field-disjoint
    /// on purpose: the stream poll holds `stream_rx` while calling it.
    pub(crate) fn accumulate_reasoning_delta(
        thinking_buf: &mut String,
        status: &mut String,
        show_thinking: bool,
        r: &str,
    ) {
        thinking_buf.push_str(r);
        if show_thinking {
            *status = format!("thinking… {} chars", thinking_buf.len());
        }
    }

    /// Ctrl+O: expand the latest reasoning block. Falls back to the
    /// global live-thinking toggle when no message carries reasoning
    /// yet, so the key always does something visible instead of
    /// silently no-op'ing.
    pub(crate) fn ctrl_o_expand(&mut self) {
        if self.toggle_recent_reasoning() {
            self.status = "reasoning block flipped — Ctrl+O again to flip back".into();
        } else {
            self.thinking_expanded = !self.thinking_expanded;
            self.status = if self.thinking_expanded {
                "no saved reasoning yet — live thinking expanded".into()
            } else {
                "thinking collapsed".into()
            };
        }
    }

    fn toggle_recent_reasoning(&mut self) -> bool {
        if let Some(idx) = self.messages.iter().rposition(|m| {
            m.role == "assistant" && m.reasoning.as_ref().is_some_and(|r| !r.trim().is_empty())
        }) {
            if self.expanded_reasoning.contains(&idx) {
                self.expanded_reasoning.remove(&idx);
            } else {
                self.expanded_reasoning.insert(idx);
            }
            true
        } else {
            false
        }
    }

    fn toggle_recent_long_message(&mut self) {
        // Most recent collapsible wins: a tool result inside a later message
        // beats an earlier long user message.
        let mut best: Option<(usize, Option<usize>)> = None;
        if let Some(idx) = self
            .messages
            .iter()
            .rposition(|m| m.role == "user" && m.content.lines().count() > CHAT_COLLAPSE_LINES)
        {
            best = Some((idx, None));
        }
        for (mi, m) in self.messages.iter().enumerate().rev() {
            for (ii, it) in m.items.iter().enumerate().rev() {
                if let Content::ToolResult { id, content, ok } = it {
                    let long = self
                        .tool_result_preview(&m.items, id, content, *ok)
                        .map(|p| p.lines().count() > CHAT_COLLAPSE_LINES)
                        .unwrap_or(false);
                    if long {
                        let key = (mi, Some(ii));
                        if best.map(|b| key > b).unwrap_or(true) {
                            best = Some(key);
                        }
                        break;
                    }
                }
            }
            if best.map(|(bi, _)| bi == mi).unwrap_or(false) {
                break;
            }
        }
        if let Some(key) = best {
            if !self.expanded_messages.remove(&key) {
                self.expanded_messages.insert(key);
            }
        }
    }

    fn insert_with_space(&mut self, s: &str) {
        let glued = self.input.cursor > 0
            && self.input.cursor <= self.input.text.len()
            && self.input.text[..self.input.cursor]
                .chars()
                .next_back()
                .map(|c| !c.is_whitespace())
                .unwrap_or(false);
        if glued {
            self.input.insert_str(" ");
        }
        self.input.insert_str(s);
    }

    fn insert_pasted_text(&mut self, txt: &str) {
        if is_long_paste(txt) {
            self.paste_seq += 1;
            let id = self.paste_seq;
            let lines = txt.lines().count();
            self.pending_texts.push((id, txt.to_string()));
            self.insert_with_space(&paste_chip(id, lines));
            self.status = format!("pasted {lines} lines → chip (expands on send)");
        } else {
            self.input.insert_str(txt);
        }
    }

    fn attach_pasted_image(&mut self, b64: String, mime: &'static str, label: String) {
        let chip = image_chip(&label);
        let kb = b64.len() / 1024;
        self.pending_image = Some(PendingImage { b64, mime, label });
        self.insert_with_space(&chip);
        self.messages.push(Msg::new(
            "system",
            format!("Image attached ({chip}, ~{kb}KB) — sent with next prompt as vision"),
        ));
    }

    fn cmd_verbosity(&mut self, args: &[&str]) {
        if args.is_empty() {
            let mut lines = vec![format!(
                "tool cards: default = {}",
                tool_verbosity_name(self.tool_verbosity(""))
            )];
            let mut tools: Vec<&String> = self.tool_display.keys().collect();
            tools.sort();
            for t in tools {
                lines.push(format!(
                    "  {t} = {}",
                    tool_verbosity_name(self.tool_verbosity(t))
                ));
            }
            lines.push("usage: /verbosity [tool] <hidden|quiet|compact|full>".into());
            self.messages.push(Msg::new("system", lines.join("\n")));
            return;
        }
        let (tool, level) = if args.len() == 1 {
            let a = args[0].to_lowercase();
            if matches!(
                a.as_str(),
                "hidden"
                    | "off"
                    | "none"
                    | "quiet"
                    | "name"
                    | "minimal"
                    | "compact"
                    | "full"
                    | "verbose"
                    | "detail"
            ) {
                (None, a)
            } else {
                self.messages.push(Msg::new(
                    "system",
                    format!(
                        "{} = {} (default = {})\nusage: /verbosity [tool] <hidden|quiet|compact|full>",
                        args[0],
                        tool_verbosity_name(self.tool_verbosity(args[0])),
                        tool_verbosity_name(self.tool_verbosity("")),
                    ),
                ));
                return;
            }
        } else {
            (Some(args[0].to_string()), args[1].to_lowercase())
        };
        if !matches!(
            level.as_str(),
            "hidden"
                | "off"
                | "none"
                | "quiet"
                | "name"
                | "minimal"
                | "compact"
                | "full"
                | "verbose"
                | "detail"
        ) {
            self.messages.push(Msg::new(
                "system",
                "usage: /verbosity [tool] <hidden|quiet|compact|full>".to_string(),
            ));
            return;
        }
        let canonical = tool_verbosity_name(parse_tool_verbosity(&level)).to_string();
        match tool {
            None => {
                self.tool_display_default = canonical.clone();
                self.save_tool_display();
                self.messages.push(Msg::new(
                    "system",
                    format!("tool cards default → {canonical} — saved"),
                ));
            }
            Some(t) => {
                self.tool_display.insert(t.clone(), canonical.clone());
                self.save_tool_display();
                self.messages.push(Msg::new(
                    "system",
                    format!("tool cards: {t} → {canonical} — saved"),
                ));
            }
        }
    }

    fn save_screenshot(&mut self, path: &str) -> anyhow::Result<()> {
        let backend = TestBackend::new(120, 40);
        let mut term = Terminal::new(backend)?;
        term.draw(|f| self.draw(f))?;
        let buffer = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    pub fn new(model: String) -> Self {
        let sid = new_session_id();
        let tcfg = crate::theme::ThemeConfig::load();
        let state = Self::load_tui_state();

        let cli_default = String::new();
        let model = if model == cli_default {
            if let Some(saved) = state.get("last_model").and_then(|v| v.as_str()) {
                if !saved.is_empty() && saved != cli_default {
                    saved.to_string()
                } else {
                    model
                }
            } else {
                model
            }
        } else {
            model
        };

        let show_thinking = state
            .get("show_thinking")
            .and_then(|v| v.as_bool())
            .unwrap_or(tcfg.show_thinking);
        let thinking_title = state
            .get("thinking_title")
            .and_then(|v| v.as_str())
            .unwrap_or(&tcfg.thinking_title)
            .to_string();
        let thinking_label = state
            .get("thinking_label")
            .and_then(|v| v.as_str())
            .unwrap_or(&tcfg.thinking_label)
            .to_string();

        let (mut tool_display_default, mut tool_display) = load_tool_display_config();
        if let Some(td) = state.get("tool_display") {
            if let Some(d) = td.get("default").and_then(|v| v.as_str()) {
                tool_display_default = d.to_string();
            }
            if let Some(obj) = td.as_object() {
                for (k, vv) in obj {
                    if k == "default" {
                        continue;
                    }
                    if let Some(s) = vv.as_str() {
                        tool_display.insert(k.clone(), s.to_string());
                    }
                }
            }
        }

        let wake_on_tasks: bool = 'cfg: {
            for cand in vioraharness_core::loop_mod::config_candidates() {
                if let Ok(s) = std::fs::read_to_string(&cand) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        if let Some(tui) = v.get("tui") {
                            break 'cfg tui
                                .get("wake_on_task_done")
                                .and_then(|x| x.as_bool())
                                .unwrap_or(true);
                        }
                    }
                }
            }
            true
        };

        let initial_models = Self::fetch_models_from_openrouter();

        // Agent mode: explicit --mode/env wins, then the last-opened
        // chat's own stored mode (per-chat memory), then the saved TUI
        // default, then the standard chain (project config > eda).
        let agent_mode = std::env::var(vioraharness_core::mode::MODE_ENV_VAR)
            .ok()
            .filter(|m| vioraharness_core::mode::is_known_mode(m))
            .or_else(Self::last_opened_chat_mode)
            .or_else(|| {
                state
                    .get("last_mode")
                    .and_then(|v| v.as_str())
                    .map(|m| m.to_string())
                    .filter(|m| vioraharness_core::mode::is_known_mode(m))
            })
            .unwrap_or_else(|| vioraharness_core::mode::resolve_mode(None));

        let (perm_tx, perm_rx) = tokio::sync::mpsc::channel(8);
        vioraharness_core::permissions::set_interactive_sender(perm_tx);

        let (q_tx, q_rx) = tokio::sync::mpsc::channel(4);
        vioraharness_core::permissions::set_question_sender(q_tx);

        vioraharness_core::tools::bash::prune_old_logs();

        Self {
            model: model.clone(),
            session_id: sid,
            messages: Vec::new(),
            input: InputState::with_disk_history(),
            scroll: 0,
            status: "ready".into(),
            should_quit: false,
            busy: false,
            pending: None,
            popup: Popup::None,
            model_cursor: 0,
            available_models: initial_models,
            model_filter: String::new(),
            tick: 0,
            streaming_buf: String::new(),
            thinking_buf: String::new(),
            stream_rx: None,
            model_fetch_rx: None,
            thinking_expanded: false,
            thinking_title,
            thinking_label,
            show_thinking,
            thinking_level: vioraharness_core::thinking::resolve_thinking_level(),
            tool_display_default,
            tool_display,
            expanded_reasoning: HashSet::new(),
            chat_search: None,
            theme_cursor: 0,
            available_themes: vec![
                "tokyonight".into(),
                "tokyonight-soft".into(),
                "eye-comfort".into(),
                "warm-dark".into(),
                "catppuccin".into(),
                "dracula".into(),
                "gruvbox".into(),
                "nord".into(),
                "system".into(),
            ],
            settings_cursor: 0,
            session_cursor: 0,
            task_cursor: 0,
            error_cursor: 0,
            rewind_cursor: 0,
            rewind_armed: None,
            rewind_armed_note: None,
            session_filter: String::new(),
            show_all_sessions: false,
            provider_cursor: 0,
            provider_key_input: String::new(),
            provider_input_active: false,
            provider_selected: None,
            provider_validating: false,
            provider_msg: None,
            pending_image: None,
            pending_texts: Vec::new(),
            paste_seq: 0,
            expanded_messages: HashSet::new(),
            model_context: std::collections::HashMap::new(),
            mode: "build".into(),
            agent_mode,
            mode_cursor: 0,
            last_diff: None,
            pending_perm: None,
            perm_cursor: 0,
            perm_rx: Some(perm_rx),
            pending_q: None,
            q_cursor: 0,
            q_answered: Vec::new(),
            q_toggled: Vec::new(),
            q_custom: String::new(),
            q_custom_active: false,
            q_rx: Some(q_rx),
            running_tool: None,
            compact_rx: None,
            ctx_freed_tokens: 0,
            wake_on_tasks,
            queued_prompts: Vec::new(),
            instant_injector: None,
            turn_session: None,
            last_tool_output: None,
            tool_output_scroll: 0,
            chat_total_lines: 0,
            selection: None,
            dragging: false,
            input_drag: false,
            copy_pending: false,
            copy_rx: None,
            paste_rx: None,
            chat_area: Rect::default(),
            input_area: Rect::default(),
            view_start: 0,
            view_total: 0,
            vis_rows: Vec::new(),
        }
    }

    pub async fn run(mut self) -> anyhow::Result<String> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        self.model_fetch_rx = Some(rx);
        tokio::spawn(async move {
            let live = Self::fetch_models_live().await;
            let _ = tx.send(live).await;
        });
        self.status = "fetching models…".into();
        // Blocking `event::read()` lives on its own thread: a pathological
        // escape sequence can park the reader, but rendering + provider/task
        // updates keep flowing on the heartbeat below — the screen never
        // needs a keypress to catch up. The thread dies with the process
        // on TUI exit (no join: it may sit parked in read).
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Event>();
        std::thread::Builder::new()
            .name("viora-input".into())
            .spawn(move || {
                while let Ok(ev) = event::read() {
                    if input_tx.send(ev).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| anyhow::anyhow!("spawn input reader: {e}"))?;
        let mut terminal = ratatui::init();

        if let Err(e) = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
        {
            tracing::warn!("mouse capture unavailable: {e}");
        }
        let sid = self.session_id.clone();
        let res = self.event_loop(&mut terminal, &input_rx).await;
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        ratatui::restore();
        match res {
            Ok(()) => Ok(self.session_id),
            Err(e) => {
                tracing::warn!("tui event_loop error for {}: {e}", sid);
                Err(e)
            }
        }
    }

    async fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        input_rx: &std::sync::mpsc::Receiver<Event>,
    ) -> anyhow::Result<()> {
        let mut last_draw = std::time::Instant::now();
        let mut dirty = true;
        loop {
            if self.should_quit {
                break;
            }

            if let Some(rx) = &mut self.model_fetch_rx {
                match rx.try_recv() {
                    Ok((models, ctx_map)) => {
                        dirty = true;
                        let n = models.len();
                        let has_gem = models.iter().any(|m| m.contains("gemini"));
                        let sample: Vec<String> = models
                            .iter()
                            .filter(|m| m.contains("gemini"))
                            .take(3)
                            .cloned()
                            .collect();
                        tracing::info!(
                            "model_fetch_rx: got {} models (has_gemini={}, sample {:?})",
                            n,
                            has_gem,
                            sample
                        );
                        if n > self.available_models.len()
                            || (n > 0 && n != self.available_models.len())
                        {
                            self.available_models = models;
                            for (k, v) in ctx_map {
                                self.model_context.insert(k, v);
                            }
                            self.status = format!("ready — {n} models");

                            tracing::info!("fetched {n} models");
                        } else if n == 0 {
                            self.status = "ready".into();

                            if self.messages.len() < 2 {
                                self.messages.push(Msg::new("system", "⚠ Fetched 0 models — check keys: /providers or see tui.log — doctor: vioraharness doctor (shell)"));
                            }
                        } else {
                            self.status = "ready".into();
                            tracing::info!(
                                "model_fetch_rx: no growth {} vs {}",
                                n,
                                self.available_models.len()
                            );
                        }
                        self.model_fetch_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.model_fetch_rx = None;
                        self.status = "ready".into();
                        dirty = true;
                        tracing::warn!("model_fetch_rx disconnected");
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            // Fingerprint: task/compact/queue polling mutates messages and
            // status through several paths — any change redraws this frame.
            let fp_before = (self.messages.len(), self.status.clone());
            self.poll_compact();

            self.poll_task_completions();

            if let Some(rx) = &mut self.perm_rx {
                match rx.try_recv() {
                    Ok(ask) => {
                        dirty = true;
                        self.pending_perm = Some(ask);
                        self.popup = Popup::PermissionAsk;
                        self.perm_cursor = 0;
                        self.status = "permission ask — choose".into();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.perm_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            if let Some(rx) = &mut self.q_rx {
                match rx.try_recv() {
                    Ok(q) => {
                        dirty = true;
                        if let Some(old) = self.pending_q.take() {
                            let _ = old
                                .tx
                                .send(vioraharness_core::permissions::QuestionResult::Cancelled);
                        }
                        self.pending_q = Some(q);
                        self.popup = Popup::Question;
                        self.q_cursor = 0;
                        self.q_answered.clear();
                        self.q_toggled.clear();
                        self.q_custom.clear();
                        self.q_custom_active = false;
                        self.status = "question — answer to continue".into();
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.q_rx = None;
                    }
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                }
            }

            if let Some(rx) = &mut self.stream_rx {
                let td_map = self.tool_display.clone();
                let td_default = self.tool_display_default.clone();
                let mut stream_dirty = false;
                while let Ok(ev) = rx.try_recv() {
                    stream_dirty = true;
                    match ev {
                        vioraharness_core::provider::ProviderEvent::TextDelta(t) => {
                            self.streaming_buf.push_str(&t);
                        }
                        vioraharness_core::provider::ProviderEvent::ReasoningDelta(r) => {
                            Self::accumulate_reasoning_delta(
                                &mut self.thinking_buf,
                                &mut self.status,
                                self.show_thinking,
                                &r,
                            );
                        }
                        vioraharness_core::provider::ProviderEvent::ToolCallDelta {
                            id,
                            name,
                            args,
                            thought_signature: _,
                        } => {
                            let verbosity = Self::verbosity_for(&td_map, &td_default, &name);
                            let pretty = pretty_tool_args_wide(
                                &name,
                                &args,
                                verbosity == ToolVerbosity::Full,
                            );

                            let mut coalesced = false;
                            if let Some(last) = self.messages.last_mut() {
                                if last.role == "system" && last.items.len() == 1 {
                                    if let Some(Content::ToolCall {
                                        name: ln,
                                        status,
                                        args: la,
                                        id: lid,
                                    }) = last.items.last_mut()
                                    {
                                        if *ln == name && *status == ToolStatus::Running {
                                            *la = args.clone();
                                            *lid = id.clone();
                                            last.content = String::new();
                                            coalesced = true;
                                        }
                                    }
                                }
                            }
                            if !coalesced && verbosity != ToolVerbosity::Hidden {
                                self.messages.push(Msg {
                                    role: "system".into(),
                                    content: String::new(),
                                    items: vec![Content::ToolCall {
                                        id: id.clone(),
                                        name: name.clone(),
                                        args: args.clone(),
                                        status: ToolStatus::Running,
                                    }],
                                    timestamp: chrono_like_now(),
                                    reasoning: None,
                                });
                            }

                            self.running_tool = Some((
                                id.clone(),
                                name.clone(),
                                pretty.clone(),
                                std::time::Instant::now(),
                            ));
                            if name == "bash" {
                                self.status =
                                    format!("running: bash {}…", truncate_chars(&pretty, 40));
                            } else {
                                self.status = format!("running: {name}…");
                            }
                        }
                        vioraharness_core::provider::ProviderEvent::ToolResultDelta {
                            id,
                            content,
                            ok,
                        } => {
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                                if let Some(d) = v.get("diff").and_then(|x| x.as_str()) {
                                    self.last_diff = Some(d.to_string());
                                } else if let Some(dp) =
                                    v.get("diff_preview").and_then(|x| x.as_str())
                                {
                                    self.last_diff = Some(dp.to_string());
                                }
                            }

                            {
                                let mut tool_name = String::new();
                                for msg in self.messages.iter().rev() {
                                    for it in &msg.items {
                                        if let Content::ToolCall { id: cid, name, .. } = it {
                                            if cid == &id {
                                                tool_name = name.clone();
                                                break;
                                            }
                                        }
                                    }
                                    if !tool_name.is_empty() {
                                        break;
                                    }
                                }
                                let full_out = if let Ok(v) =
                                    serde_json::from_str::<serde_json::Value>(&content)
                                {
                                    if let Some(log_path) = v.get("log").and_then(|x| x.as_str()) {
                                        if let Ok(full) = std::fs::read_to_string(log_path) {
                                            full
                                        } else if let Some(stdout) =
                                            v.get("stdout").and_then(|x| x.as_str())
                                        {
                                            let stderr = v
                                                .get("stderr")
                                                .and_then(|x| x.as_str())
                                                .unwrap_or("");
                                            if !stderr.is_empty() {
                                                format!("{stdout}\n--- stderr ---\n{stderr}")
                                            } else {
                                                stdout.to_string()
                                            }
                                        } else {
                                            content.clone()
                                        }
                                    } else if let Some(stdout) =
                                        v.get("stdout").and_then(|x| x.as_str())
                                    {
                                        let stderr =
                                            v.get("stderr").and_then(|x| x.as_str()).unwrap_or("");
                                        if !stderr.is_empty() {
                                            format!("{stdout}\n--- stderr ---\n{stderr}")
                                        } else {
                                            stdout.to_string()
                                        }
                                    } else if let Some(d) = v
                                        .get("diff_preview")
                                        .or_else(|| v.get("diff"))
                                        .and_then(|x| x.as_str())
                                    {
                                        d.to_string()
                                    } else if let Some(c) =
                                        v.get("content").and_then(|x| x.as_str())
                                    {
                                        c.to_string()
                                    } else if let Some(e) = v.get("error").and_then(|x| x.as_str())
                                    {
                                        e.to_string()
                                    } else {
                                        content.clone()
                                    }
                                } else {
                                    content.clone()
                                };
                                let display_name = if tool_name.is_empty() {
                                    "tool".to_string()
                                } else {
                                    tool_name
                                };
                                self.last_tool_output =
                                    Some((id.clone(), display_name, full_out, ok));
                                self.tool_output_scroll = 0;
                            }

                            for msg in self.messages.iter_mut().rev() {
                                if msg.role != "system" {
                                    continue;
                                }
                                let mut found = false;
                                for item in &mut msg.items {
                                    if let Content::ToolCall {
                                        id: cid, status, ..
                                    } = item
                                    {
                                        if cid == &id {
                                            *status = if ok {
                                                ToolStatus::Done
                                            } else {
                                                ToolStatus::Error
                                            };
                                            found = true;
                                            break;
                                        }
                                    }
                                }
                                if found {
                                    let already = msg.items.iter().any(|it| matches!(it, Content::ToolResult { id: rid, .. } if rid == &id));
                                    if !already {
                                        msg.items.push(Content::ToolResult {
                                            id: id.clone(),
                                            content: content.clone(),
                                            ok,
                                        });
                                    }

                                    if let Some((rid, _, _, _)) = &self.running_tool {
                                        if rid == &id {
                                            self.running_tool = None;

                                            if self.status.starts_with("running:") {
                                                self.status = if ok {
                                                    "ready".into()
                                                } else {
                                                    "error".into()
                                                };
                                            }
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        vioraharness_core::provider::ProviderEvent::Notice(msg) => {
                            self.ctx_freed_tokens += Self::parse_compact_freed(&msg);
                            self.status = msg;
                        }
                        _ => {}
                    }
                }
                if stream_dirty {
                    dirty = true;
                }
            }

            if self.busy {
                if let Some(handle) = &mut self.pending {
                    if handle.is_finished() {
                        let handle = self.pending.take().unwrap();
                        self.busy = false;
                        self.streaming_buf.clear();
                        match handle.await {
                            Ok(Ok(text)) => {
                                let mut msg = Msg::new("assistant", text);

                                if !self.thinking_buf.trim().is_empty() {
                                    msg.reasoning = Some(self.thinking_buf.clone());
                                }
                                self.thinking_buf.clear();
                                self.messages.push(msg);
                                self.status = "ready".into();
                            }
                            Ok(Err(e)) => {
                                self.report_error("turn", format!("error: {e:#}"));
                                self.status = "error".into();
                            }
                            Err(e) => {
                                self.report_error("turn", format!("join error: {e}"));
                                self.status = "error".into();
                            }
                        }
                    }
                }

                if !self.busy {
                    // A finished turn may leave `$` prompts the loop never
                    // picked up (typed during the final stream): head them
                    // onto the queue before the drain.
                    self.promote_instant_leftovers();
                }
                self.drain_queue();
            }
            if (self.messages.len(), self.status.clone()) != fp_before {
                dirty = true;
            }

            // Input arrives via the reader thread — drain everything pending
            // so bursts (pastes, mouse drags) collapse into one redraw
            // instead of one expensive frame per event.
            loop {
                match input_rx.try_recv() {
                    Ok(Event::Key(k)) => {
                        if k.kind != KeyEventKind::Press {
                            continue;
                        }
                        dirty = true;
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && k.modifiers.contains(KeyModifiers::ALT)
                        {
                            if self.input.selected_range().is_some() {
                                self.copy_input_selection();
                            } else if self.selection.is_some() {
                                self.copy_pending = true;
                            } else {
                                self.status = "nothing selected — drag to select first".into();
                            }
                            continue;
                        }
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT)
                        {
                            if self.input.selected_range().is_some() {
                                self.copy_input_selection();
                            } else if self.selection.is_some() {
                                self.copy_pending = true;
                            } else {
                                self.selection = None;
                                self.should_quit = true;
                                break;
                            }
                            continue;
                        }
                        if k.code == KeyCode::Char('g')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            if !self.show_thinking {
                                self.show_thinking = true;
                                self.thinking_expanded = true;
                                Self::save_tui_state(serde_json::json!({"show_thinking": true}));
                                self.messages.push(Msg::new(
                                    "system",
                                    format!(
                                        "Thinking ON ({} / {}) — Ctrl+G to collapse",
                                        self.thinking_title, self.thinking_label
                                    ),
                                ));
                            } else {
                                if !self.toggle_recent_reasoning() {
                                    self.thinking_expanded = !self.thinking_expanded;
                                }
                            }
                            continue;
                        }

                        if k.code == KeyCode::Char('o')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && self.popup == Popup::None
                        {
                            self.ctrl_o_expand();
                            continue;
                        }

                        if (k.code == KeyCode::Char('x') || k.code == KeyCode::Char('X'))
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT)
                            && self.popup == Popup::None
                        {
                            self.toggle_recent_long_message();
                            continue;
                        }

                        if self.input.text.is_empty() && self.popup == Popup::None {
                            match k.code {
                                KeyCode::Char('j')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    if self.scroll > 0 {
                                        self.scroll -= 1;
                                    }
                                    continue;
                                }
                                KeyCode::Char('k')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    self.scroll = self.scroll.saturating_add(1);
                                    continue;
                                }
                                KeyCode::Char('G') if k.modifiers.contains(KeyModifiers::SHIFT) => {
                                    self.scroll = 0;
                                    continue;
                                }
                                KeyCode::Char('g')
                                    if k.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    self.scroll = 10000;
                                    continue;
                                }
                                _ => {}
                            }
                        }

                        if k.code == KeyCode::Char('f')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && self.popup == Popup::None
                        {
                            self.chat_search = Some(String::new());
                            self.messages
                                .push(Msg::new("system", "Search: type then Enter (Esc to clear)"));
                            continue;
                        }

                        if self.chat_search.is_some() {
                            match k.code {
                                KeyCode::Char(c)
                                    if !k.modifiers.contains(KeyModifiers::CONTROL)
                                        && !k.modifiers.contains(KeyModifiers::ALT) =>
                                {
                                    if let Some(q) = &mut self.chat_search {
                                        q.push(c);
                                    }
                                    continue;
                                }
                                KeyCode::Backspace => {
                                    if let Some(q) = &mut self.chat_search {
                                        q.pop();
                                    }
                                    continue;
                                }
                                KeyCode::Esc => {
                                    self.chat_search = None;
                                    self.messages.push(Msg::new("system", "Search cleared"));
                                    continue;
                                }
                                KeyCode::Enter => {
                                    let q = self.chat_search.clone().unwrap_or_default();
                                    let cnt = self
                                        .messages
                                        .iter()
                                        .filter(|m| m.content.contains(q.as_str()))
                                        .count();
                                    self.messages.push(Msg::new(
                                        "system",
                                        format!("Search: '{}' — {} matches (Esc to clear)", q, cnt),
                                    ));
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        if k.code == KeyCode::Char('t')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                            && (self.popup == Popup::None || self.popup == Popup::Settings)
                        {
                            self.cycle_thinking_level(1);
                            continue;
                        }
                        if k.code == KeyCode::F(12) {
                            let ts = chrono_like_now().replace(':', "-");
                            let path = format!("/tmp/vioraharness_screenshot_{}.txt", ts);
                            match self.save_screenshot(&path) {
                            Ok(_) => self.messages.push(Msg::new("system", format!("Screenshot saved → {} (120x40 TestBackend) — attach for vision debug", path))),
                            Err(e) => self.messages.push(Msg::new("system", format!("Screenshot failed: {e}"))),
                        }
                            continue;
                        }
                        if k.code == KeyCode::F(2) {
                            self.provider_cursor = 0;
                            self.provider_key_input.clear();
                            self.provider_input_active = false;
                            self.provider_selected = None;
                            self.provider_msg = None;
                            self.popup = if self.popup == Popup::Providers {
                                Popup::None
                            } else {
                                Popup::Providers
                            };
                            continue;
                        }

                        if self.popup == Popup::None && self.last_tool_output.is_some() {
                            let is_f9 = k.code == KeyCode::F(9);
                            let is_shift_v = k.code == KeyCode::Char('V')
                                && !k.modifiers.contains(KeyModifiers::CONTROL)
                                && !k.modifiers.contains(KeyModifiers::ALT);
                            let is_v_empty = k.code == KeyCode::Char('v')
                                && self.input.text.is_empty()
                                && !k.modifiers.contains(KeyModifiers::CONTROL)
                                && !k.modifiers.contains(KeyModifiers::ALT)
                                && self.popup == Popup::None;

                            if is_f9 || (is_shift_v && self.input.text.is_empty()) {
                                self.popup = Popup::ToolOutput;
                                self.tool_output_scroll = 0;
                                continue;
                            }

                            let _ = is_v_empty;
                        }
                        if k.code == KeyCode::Enter && k.modifiers.contains(KeyModifiers::ALT) {
                            self.input.insert('\n');
                            continue;
                        }
                        if k.code == KeyCode::Char('l')
                            && k.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            self.input.text.clear();
                            self.input.cursor = 0;
                            continue;
                        }

                        if self.popup == Popup::None {
                            if k.modifiers.contains(KeyModifiers::CONTROL)
                                && k.modifiers.contains(KeyModifiers::ALT)
                                && matches!(k.code, KeyCode::Char('v') | KeyCode::Char('V'))
                            {
                                // Probed off-thread (arboard/X11 can stall);
                                // completion inserts via poll_clipboard_results.
                                self.begin_clipboard_paste(true);
                                continue;
                            }
                            if k.modifiers.contains(KeyModifiers::CONTROL) {
                                match k.code {
                                    KeyCode::Char('u') | KeyCode::Char('U') => {
                                        self.input.delete_to_start();
                                        continue;
                                    }
                                    KeyCode::Char('k') | KeyCode::Char('K') => {
                                        self.input.delete_to_end();
                                        continue;
                                    }
                                    KeyCode::Char('a') | KeyCode::Char('A') => {
                                        self.input.move_to_start(false);
                                        continue;
                                    }
                                    KeyCode::Char('e') | KeyCode::Char('E') => {
                                        self.input.move_to_end(false);
                                        continue;
                                    }
                                    KeyCode::Char('w') | KeyCode::Char('W') => {
                                        self.input.delete_word_before();
                                        continue;
                                    }
                                    KeyCode::Char('h') | KeyCode::Char('H') => {
                                        self.input.backspace();
                                        continue;
                                    }
                                    KeyCode::Char('d') | KeyCode::Char('D') => {
                                        if !self.input.text.is_empty() {
                                            self.input.delete();
                                        }
                                        continue;
                                    }
                                    KeyCode::Char('v') | KeyCode::Char('V') => {
                                        // Probed off-thread (arboard/X11 can
                                        // stall); completion inserts via
                                        // poll_clipboard_results.
                                        self.begin_clipboard_paste(false);
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                            if k.modifiers.contains(KeyModifiers::ALT) {
                                match k.code {
                                    KeyCode::Char('d') | KeyCode::Char('D') => {
                                        self.input.delete_word_after();
                                        continue;
                                    }
                                    KeyCode::Backspace => {
                                        self.input.delete_word_before();
                                        continue;
                                    }
                                    KeyCode::Char('b') | KeyCode::Char('B') => {
                                        self.input.move_left(false);
                                        continue;
                                    }
                                    KeyCode::Char('f') | KeyCode::Char('F') => {
                                        self.input.move_right(false);
                                        continue;
                                    }
                                    _ => {}
                                }
                            }
                        }

                        if self.popup != Popup::None {
                            self.handle_popup_key(k);
                            continue;
                        }
                        self.handle_key_event(k).await?;
                    }
                    Ok(Event::Resize(_, _)) => {
                        dirty = true;
                    }
                    Ok(Event::Mouse(m)) => {
                        self.handle_mouse(m);
                        dirty = true;
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // Reader gone (terminal lost): nothing left to drive us.
                        self.should_quit = true;
                        break;
                    }
                }
            }
            if self.poll_clipboard_results() {
                dirty = true;
            }
            // Heartbeat: spinner + tool timers animate while busy; popups
            // and running-task footers stay live; the idle backstop bounds
            // any missed-dirty staleness without needing a keypress.
            let active = self.busy || self.popup != Popup::None;
            let interval = if active {
                Duration::from_millis(100)
            } else if has_running_tasks() {
                Duration::from_millis(500)
            } else {
                Duration::from_millis(2000)
            };
            if dirty || last_draw.elapsed() >= interval {
                if self.busy {
                    self.tick = self.tick.wrapping_add(1);
                }
                terminal.draw(|f| self.draw(f))?;
                last_draw = std::time::Instant::now();
                dirty = false;
            } else {
                // Fully idle: sip CPU instead of spinning try_recv.
                // Worst-case input/stream latency +10ms.
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        Ok(())
    }
}

/// Any live background task? Drives the idle heartbeat so /tasks footers
/// stay fresh while work runs, without redrawing a static screen hot.
fn has_running_tasks() -> bool {
    vioraharness_core::tools::tasks::list_tasks()
        .iter()
        .any(|t| t.status == vioraharness_core::tools::tasks::BgStatus::Running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;

    #[test]
    fn tool_verbosity_levels_parse() {
        assert_eq!(parse_tool_verbosity("hidden"), ToolVerbosity::Hidden);
        assert_eq!(parse_tool_verbosity("off"), ToolVerbosity::Hidden);
        assert_eq!(parse_tool_verbosity("quiet"), ToolVerbosity::Quiet);
        assert_eq!(parse_tool_verbosity("name"), ToolVerbosity::Quiet);
        assert_eq!(parse_tool_verbosity("compact"), ToolVerbosity::Compact);
        assert_eq!(parse_tool_verbosity("full"), ToolVerbosity::Full);
        assert_eq!(parse_tool_verbosity("verbose"), ToolVerbosity::Full);
        assert_eq!(parse_tool_verbosity("bogus"), ToolVerbosity::Compact);
        assert_eq!(parse_tool_verbosity(""), ToolVerbosity::Compact);

        let mut app = test_app();
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Compact);
        app.tool_display_default = "quiet".into();
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Quiet);
        app.tool_display.insert("bash".into(), "hidden".into());
        assert_eq!(app.tool_verbosity("bash"), ToolVerbosity::Hidden);
        assert_eq!(app.tool_verbosity("write"), ToolVerbosity::Quiet);
    }
}
