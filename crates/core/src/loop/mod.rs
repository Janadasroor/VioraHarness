use crate::context::{assemble_context, compaction};
use crate::permissions::{decide, rules_from_json, Decision, Rule};
use crate::provider::{ChatMessage, ChatRequest, ProviderEvent, ToolDefForProvider};
use crate::tools::{self, ToolRegistry};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct AgentLoop {
    pub registry: ToolRegistry,
    pub rules: Vec<Rule>,
    pub max_tokens: usize,
}

impl Default for AgentLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentLoop {
    pub fn new() -> Self {
        let rules = load_rules_from_config();
        Self {
            registry: ToolRegistry::new(),
            rules,
            max_tokens: 128_000,
        }
    }

    pub fn with_registry(registry: ToolRegistry) -> Self {
        Self {
            registry,
            rules: load_rules_from_config(),
            max_tokens: 128_000,
        }
    }

    #[async_recursion::async_recursion]
    pub async fn run(
        &self,
        prompt: &str,
        model: &str,
        session_id: Option<String>,
    ) -> anyhow::Result<String> {
        self.run_inner(prompt, model, session_id, 0, None, None)
            .await
    }

    #[async_recursion::async_recursion]
    pub async fn run_streaming(
        &self,
        prompt: &str,
        model: &str,
        session_id: Option<String>,
        stream_tx: tokio::sync::mpsc::Sender<crate::provider::ProviderEvent>,
    ) -> anyhow::Result<String> {
        self.run_inner(prompt, model, session_id, 0, Some(stream_tx), None)
            .await
    }

    #[async_recursion::async_recursion]
    pub async fn run_streaming_with_image(
        &self,
        prompt: &str,
        model: &str,
        session_id: Option<String>,
        stream_tx: tokio::sync::mpsc::Sender<crate::provider::ProviderEvent>,
        image: Option<(String, String)>,
    ) -> anyhow::Result<String> {
        self.run_inner(prompt, model, session_id, 0, Some(stream_tx), image)
            .await
    }

    async fn run_inner(
        &self,
        prompt: &str,
        model: &str,
        session_id: Option<String>,
        depth: usize,
        stream_tx: Option<tokio::sync::mpsc::Sender<crate::provider::ProviderEvent>>,
        image: Option<(String, String)>,
    ) -> anyhow::Result<String> {
        if depth > 3 {
            anyhow::bail!("max subagent recursion depth 3 exceeded");
        }
        let provider = crate::provider::provider_for_model(model);

        let session_id = session_id.unwrap_or_else(simple_id);
        let store = try_create_store(&session_id, model);

        let mut history: Vec<ChatMessage> = Vec::new();
        let mut is_new_session = true;
        if let Some(ref s) = store {
            match s.get_session(&session_id) {
                Ok(Some(_sess)) => {
                    is_new_session = false;

                    history = load_history_from_store(s, &session_id);
                    let _ = s.touch_session(&session_id, Some(model));
                }
                Ok(None) => {
                    let _ = s.create_session(
                        &session_id,
                        model,
                        Some(prompt.chars().take(80).collect::<String>().as_str()),
                    );
                }
                Err(_) => {
                    let _ = s.create_session(
                        &session_id,
                        model,
                        Some(prompt.chars().take(80).collect::<String>().as_str()),
                    );
                }
            }
        }

        let ctx = assemble_context(prompt);
        let mut messages: Vec<ChatMessage> = vec![ChatMessage::text("system", ctx.system)];

        messages.extend(history);

        let user_msg = if let Some((mime, b64)) = image.clone() {
            ChatMessage::with_image_mime(
                "user",
                format!(
                    "{}\n\nUser request: {} [image attached]",
                    ctx.user_extra, prompt
                ),
                &mime,
                &b64,
            )
        } else {
            ChatMessage::text(
                "user",
                format!("{}\n\nUser request: {}", ctx.user_extra, prompt),
            )
        };
        messages.push(user_msg.clone());
        // Safety net: history must keep tool contiguity (every assistant

        sanitize_tool_contiguity(&mut messages);

        if let Some(ref s) = store {
            if is_new_session {
                let sid_clone = session_id.clone();
                let prompt_clone = prompt.to_string();
                let model_clone = model.to_string();
                let fallback_title = prompt.chars().take(50).collect::<String>();
                tokio::spawn(async move {
                    let title_model = std::env::var("VIORAHARNESS_TITLE_MODEL")
                        .ok()
                        .filter(|m| !m.trim().is_empty())
                        .unwrap_or(model_clone);
                    let provider = crate::provider::provider_for_model(&title_model);
                    let req = crate::provider::ChatRequest {
                        model: title_model.clone(),
                        messages: vec![
                            crate::provider::ChatMessage::text("system", "You are a title generator. Produce a concise 4-6 word title for the conversation, no quotes, no punctuation beyond spaces. Tools denied."),
                            crate::provider::ChatMessage::text("user", format!("Title for: {}", prompt_clone)),
                        ],
                        tools: None,
                        tool_choice: None,
                        max_tokens: Some(20),
                        temperature: Some(0.5),
                    };
                    let title = match provider.stream(req).await {
                        Ok(mut rx) => {
                            let mut out = String::new();
                            while let Some(ev) = rx.recv().await {
                                match ev {
                                    crate::provider::ProviderEvent::TextDelta(t) => {
                                        out.push_str(&t)
                                    }
                                    crate::provider::ProviderEvent::Done => break,
                                    _ => {}
                                }
                            }
                            let t = out.trim().trim_matches('"').to_string();
                            if t.is_empty() || t.len() < 3 {
                                fallback_title
                            } else {
                                t.chars().take(80).collect::<String>()
                            }
                        }
                        Err(_) => fallback_title,
                    };
                    let db = std::env::var("VIORAHARNESS_DB")
                        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                    if let Ok(store) = crate::session::SessionStore::new(&db) {
                        let _ = store.rename_session(&sid_clone, &title);
                    }
                });
            }
            if is_new_session {
                let _ = s.append_message(&session_id, "user", prompt);
            } else {
                let _ = s.append_message_full(
                    &session_id,
                    "user",
                    prompt,
                    None,
                    None,
                    Some(model),
                    None,
                );
            }

            if let Ok(Some(sess)) = s.get_session(&session_id) {
                if sess.title.is_none()
                    || sess.title.as_deref().map(|t| t.len() < 5).unwrap_or(false)
                {
                    let _ =
                        s.rename_session(&session_id, &prompt.chars().take(80).collect::<String>());
                }
            }
        }

        let provider_tools: Vec<ToolDefForProvider> = self.registry.to_provider_tools();
        #[allow(unused_assignments)]
        let mut final_text: String = String::new();

        let _run_approval = auto_allow_on().then(ApprovalGuard::hold);
        let mut turn = 0;

        let mut retried_context_compact = false;

        let mut call_history: Vec<(String, String)> = Vec::new();
        let mut unproductive_streak: usize = 0;
        let mut empty_nudges: u32 = 0;

        loop {
            turn += 1;
            if turn > 20 {
                messages.push(ChatMessage::text(
                    "user",
                    "Turn budget (20 tool turns) exhausted. Summarize your findings so far for the user now — be concrete, cite what you actually found. Do not call any tools.",
                ));
                let summary_req = ChatRequest {
                    model: model.to_string(),
                    messages: messages.clone(),
                    tools: None,
                    tool_choice: None,
                    max_tokens: Some(1024),
                    temperature: Some(0.7),
                };
                let mut summary = String::new();
                match provider.stream(summary_req).await {
                    Ok(mut rx) => {
                        while let Some(ev) = rx.recv().await {
                            if let Some(tx) = &stream_tx {
                                let _ = tx.send(ev.clone()).await;
                            }
                            if let ProviderEvent::TextDelta(t) = ev {
                                summary.push_str(&t);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("budget summary call failed: {e:#}");
                    }
                }
                if summary.trim().is_empty() {
                    anyhow::bail!("max turns 20 exceeded — possible tool loop");
                }
                if let Some(ref s) = store {
                    let _ = s.append_message_full(
                        &session_id,
                        "assistant",
                        &summary,
                        None,
                        None,
                        Some(model),
                        None,
                    );
                }
                return Ok(summary);
            }

            let total_chars: usize = messages.iter().map(|m| m.content_len()).sum();
            let est_tokens = total_chars / 4;

            let context_limit =
                crate::provider::catalog::context_limit_for_model(model, self.max_tokens).await;
            if compaction::should_compact(est_tokens, context_limit) {
                if let Some(tx) = &stream_tx {
                    let _ = tx
                        .send(crate::provider::ProviderEvent::Notice(format!(
                            "Compacting context… (est {est_tokens} tokens / limit {context_limit})"
                        )))
                        .await;
                }
                let cmp_keep = compaction::compaction_config().keep_tail;
                match store.as_ref() {
                    Some(s) => {
                        match compaction::compact_session(
                            s,
                            provider.as_ref(),
                            &session_id,
                            model,
                            cmp_keep,
                        )
                        .await
                        {
                            Ok(rep) if rep.compacted => {
                                let notice = compaction::compact_notice(&rep);
                                tracing::warn!("auto-compact: {notice}");
                                splice_compacted_history(&mut messages, s, &session_id);

                                let est2: usize =
                                    messages.iter().map(|m| m.content_len()).sum::<usize>() / 4;
                                if est2 > context_limit && messages.len() > 8 {
                                    tracing::warn!(
                                        "still over limit after compact ({est2} > {context_limit}) — emergency squash"
                                    );
                                    messages = compaction::compact_messages(messages, 5);
                                    sanitize_tool_contiguity(&mut messages);
                                    if let Some(tx) = &stream_tx {
                                        let _ = tx
                                            .send(crate::provider::ProviderEvent::Notice(
                                                "Context still full after compact — emergency squash to last 5 (session rows kept)".to_string(),
                                            ))
                                            .await;
                                    }
                                }
                                if let Some(tx) = &stream_tx {
                                    let _ = tx
                                        .send(crate::provider::ProviderEvent::Notice(notice))
                                        .await;
                                }
                            }
                            Ok(rep) => tracing::info!("auto-compact skipped: {}", rep.note),
                            Err(e) => {
                                tracing::warn!("auto-compact failed ({e:#}) — in-memory squash");
                                messages = compaction::compact_messages(messages, cmp_keep);
                            }
                        }
                    }
                    None => {
                        messages = compaction::compact_messages(messages, cmp_keep);
                    }
                }
            }

            let req = ChatRequest {
                model: model.to_string(),
                messages: messages.clone(),
                tools: if provider_tools.is_empty() {
                    None
                } else {
                    Some(provider_tools.clone())
                },
                tool_choice: Some("auto".into()),
                max_tokens: Some(1024),
                temperature: Some(0.7),
            };

            tracing::info!(
                "AgentLoop turn {turn} model={model} msg_len={} depth={depth}",
                messages.len()
            );

            let mut rx = match provider.stream(req).await {
                Ok(rx) => rx,
                Err(e) if !retried_context_compact && compaction::is_context_full_error(&e) => {
                    retried_context_compact = true;
                    tracing::warn!(
                        "provider reports full context ({e:#}) — forced compact + retry"
                    );
                    if let Some(tx) = &stream_tx {
                        let _ = tx
                            .send(crate::provider::ProviderEvent::Notice(
                                "Provider reports full context — emergency compact + one retry…"
                                    .to_string(),
                            ))
                            .await;
                    }
                    let cmp_keep = compaction::compaction_config().keep_tail;
                    let mut freed = false;
                    if let Some(s) = store.as_ref() {
                        match compaction::compact_session(
                            s,
                            provider.as_ref(),
                            &session_id,
                            model,
                            cmp_keep,
                        )
                        .await
                        {
                            Ok(rep) if rep.compacted => {
                                freed = true;
                                splice_compacted_history(&mut messages, s, &session_id);
                                if let Some(tx) = &stream_tx {
                                    let _ = tx
                                        .send(crate::provider::ProviderEvent::Notice(
                                            compaction::compact_notice(&rep),
                                        ))
                                        .await;
                                }
                            }
                            Ok(rep) => tracing::info!("forced compact skipped: {}", rep.note),
                            Err(ce) => tracing::warn!("forced compact failed: {ce:#}"),
                        }
                    }
                    if !freed {
                        return Err(e);
                    }
                    let req2 = ChatRequest {
                        model: model.to_string(),
                        messages: messages.clone(),
                        tools: if provider_tools.is_empty() {
                            None
                        } else {
                            Some(provider_tools.clone())
                        },
                        tool_choice: Some("auto".into()),
                        max_tokens: Some(1024),
                        temperature: Some(0.7),
                    };
                    provider.stream(req2).await?
                }
                Err(e) => return Err(e),
            };
            let mut assistant_text = String::new();
            let mut reasoning_buf = String::new();
            let mut tool_calls: Vec<(String, String, String, Option<String>)> = Vec::new();

            while let Some(ev) = rx.recv().await {
                if let Some(tx) = &stream_tx {
                    let _ = tx.send(ev.clone()).await;
                }
                match ev {
                    ProviderEvent::TextDelta(t) => {
                        if stream_tx.is_none() && std::env::var("VIORAHARNESS_TUI").is_err() {
                            print!("{t}");
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        }
                        assistant_text.push_str(&t);
                    }
                    ProviderEvent::ReasoningDelta(r) => {
                        reasoning_buf.push_str(&r);
                        tracing::debug!("reasoning: {r}");
                    }
                    ProviderEvent::ToolCallDelta {
                        id,
                        name,
                        args,
                        thought_signature,
                    } => {
                        tracing::info!(
                            "tool call {name} id={id} args={args} sig={:?}",
                            thought_signature.as_deref().map(|s| &s[..8.min(s.len())])
                        );
                        tool_calls.push((id, name, args, thought_signature));
                    }
                    ProviderEvent::ToolResultDelta { .. } => {}
                    ProviderEvent::Notice(msg) => {
                        tracing::warn!("{msg}");
                    }
                    ProviderEvent::Done => break,
                }
            }
            if stream_tx.is_none() && std::env::var("VIORAHARNESS_TUI").is_err() {
                println!();
            }

            for (_id, _name, args, _sig) in tool_calls.iter_mut() {
                *args = normalize_tool_args(args);
            }
            final_text = assistant_text.clone();
            let reasoning_opt = if reasoning_buf.trim().is_empty() {
                None
            } else {
                Some(reasoning_buf.as_str())
            };

            if tool_calls.is_empty() {
                if let Some(ref s) = store {
                    let _ = s.append_message_full(
                        &session_id,
                        "assistant",
                        &assistant_text,
                        None,
                        reasoning_opt,
                        Some(model),
                        None,
                    );
                }

                if assistant_text.trim().is_empty() && empty_nudges < 2 {
                    empty_nudges += 1;
                    tracing::warn!("empty turn — nudge {empty_nudges}/2");
                    let nudge = ChatMessage::text(
                        "user",
                        "Your last response was empty (no text, no tool calls) — \
                         likely a generation glitch. If the task needs no tools, \
                         answer in text; otherwise make the tool call(s) now.",
                    );
                    messages.push(nudge);
                    if let Some(ref s) = store {
                        let _ = s.append_message(
                            &session_id,
                            "user",
                            "[harness nudge after empty turn — not the user]",
                        );
                    }
                    continue;
                }
                break;
            }

            let tool_calls_for_history: Vec<crate::provider::ToolCall> = tool_calls
                .iter()
                .map(|(id, name, args, sig)| crate::provider::ToolCall {
                    id: id.clone(),
                    call_type: "function".into(),
                    function: crate::provider::FunctionCall {
                        name: name.clone(),
                        arguments: args.clone(),
                    },
                    thought_signature: sig.clone(),
                })
                .collect();

            let mut assistant_msg = ChatMessage::text("assistant", assistant_text.clone());
            assistant_msg.tool_calls = Some(tool_calls_for_history);
            messages.push(assistant_msg);
            if let Some(ref s) = store {
                let seq = s
                    .append_message_full(
                        &session_id,
                        "assistant",
                        &assistant_text,
                        None,
                        reasoning_opt,
                        Some(model),
                        None,
                    )
                    .unwrap_or(0);
                for (id, name, args, sig) in &tool_calls {
                    let args_val: Value = serde_json::from_str(args).unwrap_or(json!({}));
                    if let Err(e) = s.record_tool_call_with_sig(
                        id,
                        &session_id,
                        seq,
                        name,
                        &args_val,
                        sig.as_deref(),
                    ) {
                        tracing::error!(
                            "record_tool_call_with_sig failed for {}: {e} sig={:?}",
                            name,
                            sig.as_deref().map(|v| &v[..20.min(v.len())])
                        );
                    }
                }
            }

            let mut pending_images: Vec<(String, String)> = Vec::new();
            for (id, name, args_str, _sig) in tool_calls {
                let mut args_val: Value = serde_json::from_str(&args_str).unwrap_or(json!({}));

                if matches!(
                    name.as_str(),
                    "todowrite" | "edit" | "apply_patch" | "question" | "task"
                ) {
                    if let Some(obj) = args_val.as_object_mut() {
                        obj.entry("session_id")
                            .or_insert(Value::String(session_id.clone()));
                        obj.entry("tool_call_id")
                            .or_insert(Value::String(id.clone()));
                        if name == "task" {
                            obj.entry("model")
                                .or_insert(Value::String(model.to_string()));
                        }
                    }
                }

                let mut decision = decide(&self.rules, &name, &args_str);

                if name == "bash" && decision == Decision::Allow && is_dangerous_bash(&args_str) {
                    tracing::warn!("dangerous bash downgraded to ask: {args_str}");
                    decision = Decision::Ask;
                }
                let result = match decision {
                    Decision::Deny => {
                        json!({"ok": false, "error": format!("FILE NOT CREATED: tool {name} denied by policy (pattern matched deny). args={args_str}")})
                    }
                    Decision::Ask => {
                        let is_write_allowed_path = if name == "write" || name == "edit" {
                            if let Some(p) = args_val.get("path").and_then(|v| v.as_str()) {
                                let resolved = crate::tools::viora::resolve_path(p);
                                crate::tools::viora::is_within_root(&resolved)
                            } else {
                                false
                            }
                        } else if name == "apply_patch" {
                            args_val
                                .get("patch")
                                .and_then(|v| v.as_str())
                                .and_then(|text| crate::tools::patch::parse_patch(text).ok())
                                .map(|ops| {
                                    let paths = crate::tools::patch::touched_paths(&ops);
                                    !paths.is_empty()
                                        && paths.iter().all(|p| {
                                            let rp = crate::tools::viora::resolve_path(p);
                                            crate::tools::viora::is_within_root(&rp)
                                                || p.starts_with("/tmp/")
                                        })
                                })
                                .unwrap_or(false)
                        } else {
                            false
                        };
                        let is_tmp_write = name == "write" && args_str.contains("/tmp/");

                        let auto_allow = auto_allow_on();
                        if auto_allow {
                            tracing::warn!(
                                "auto-allow: executing {name} (VIORAHARNESS_AUTO_ALLOW)"
                            );
                        }
                        if is_read_only_tool(&name)
                            || (name == "bash" && is_safe_bash(&args_str))
                            || is_tmp_write
                            || is_write_allowed_path
                            || auto_allow
                        {
                            if name == "task" && depth >= 2 {
                                json!({"ok": false, "error": "task recursion depth exceeded (max 2)"})
                            } else {
                                if name == "write" {
                                    if let Some(p) = args_val.get("path").and_then(|v| v.as_str()) {
                                        let snap = crate::session::snapshot::UndoStack::new();
                                        let seq = messages.len() as i64;
                                        snap.push(&session_id, seq, p).await;
                                    }
                                }

                                if auto_allow {
                                    execute_approved(&name, args_val).await
                                } else {
                                    tools::execute_tool(&name, args_val).await
                                }
                            }
                        } else {
                            let interactive_result: Option<Value> = if std::env::var(
                                "VIORAHARNESS_TUI",
                            )
                            .is_ok()
                            {
                                if let Some(sender) = crate::permissions::get_interactive_sender() {
                                    let (tx, rx) = tokio::sync::oneshot::channel();
                                    let ask = crate::permissions::InteractiveAsk {
                                        id: id.clone(),
                                        tool: name.clone(),
                                        args: args_str.clone(),
                                        tx,
                                    };

                                    if let Some(s_tx) = &stream_tx {
                                        let _ = s_tx
                                            .send(crate::provider::ProviderEvent::ToolCallDelta {
                                                id: id.clone(),
                                                name: format!("{} (permission ask)", name),
                                                args: args_str.clone(),
                                                thought_signature: None,
                                            })
                                            .await;
                                    }
                                    if sender.send(ask).await.is_ok() {
                                        match rx.await {
                                            Ok(crate::permissions::InteractiveDecision::AllowOnce) => {
                                                if name == "write" {
                                                    if let Some(p) = args_val.get("path").and_then(|v| v.as_str()) {
                                                        let snap = crate::session::snapshot::UndoStack::new();
                                                        let seq = messages.len() as i64;
                                                        snap.push(&session_id, seq, p).await;
                                                    }
                                                }
                                                Some(execute_approved(&name, args_val.clone()).await)
                                            }
                                            Ok(crate::permissions::InteractiveDecision::AllowAlways) => {

                                                persist_allow_always(&name, &args_str);


                                                if name == "write" {
                                                    if let Some(p) = args_val.get("path").and_then(|v| v.as_str()) {
                                                        let snap = crate::session::snapshot::UndoStack::new();
                                                        let seq = messages.len() as i64;
                                                        snap.push(&session_id, seq, p).await;
                                                    }
                                                }
                                                Some(execute_approved(&name, args_val.clone()).await)
                                            }
                                            Ok(crate::permissions::InteractiveDecision::Deny) => {
                                                Some(json!({"ok": false, "error": format!("tool {name} denied by user (permission ask). args={args_str}")}))
                                            }
                                            Err(_) => None,
                                        }
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(v) = interactive_result {
                                v
                            } else {
                                json!({"ok": false, "error": format!("FILE NOT CREATED: tool {name} blocked by permissions (ask). In TUI use the Allow once / Allow always dialog, headless rerun with -y, or add to vioraharness.json permissions allow. For ls prefer glob; for file content prefer read. args={args_str}")})
                            }
                        }
                    }
                    Decision::Allow => {
                        if name == "task" && depth >= 2 {
                            json!({"ok": false, "error": "task recursion depth exceeded (max 2)"})
                        } else {
                            if name == "write" {
                                if let Some(p) = args_val.get("path").and_then(|v| v.as_str()) {
                                    let snap = crate::session::snapshot::UndoStack::new();

                                    let seq = messages.len() as i64;
                                    snap.push(&session_id, seq, p).await;
                                }
                            }
                            tools::execute_tool(&name, args_val).await
                        }
                    }
                };

                let has_image = result.get("base64").is_some();
                let mut result_for_llm = result.clone();

                if let Some(obj) = result_for_llm.as_object_mut() {
                    if obj.contains_key("diff_preview") {
                        obj.remove("diff");
                    }
                }
                if has_image {
                    if let Some(obj) = result_for_llm.as_object_mut() {
                        obj.remove("base64");

                        if let Some(b64) = result.get("base64").and_then(|v| v.as_str()) {
                            obj.insert("base64_len".into(), serde_json::json!(b64.len()));
                            obj.insert(
                                "base64_preview".into(),
                                serde_json::json!(format!(
                                    "{}... ({} chars total)",
                                    &b64[..100.min(b64.len())],
                                    b64.len()
                                )),
                            );
                        }
                    }
                }
                let mut result_str =
                    serde_json::to_string(&result_for_llm).unwrap_or_else(|_| "{}".into());
                if result_str.len() > 8000 {
                    let tmp_path = format!(
                        "/tmp/vioraharness_tool_{}_{}.json",
                        name,
                        &id[..8.min(id.len())]
                    );
                    let _ = std::fs::write(&tmp_path, &result_str);
                    result_str = crate::tools::viora::truncate_to_bytes(&result_str, 8000);
                    result_str.push_str(&format!(
                        "...(truncated, full {} chars saved to {} — press V in TUI / cat {} or /output)",
                        result_for_llm.to_string().len(),
                        tmp_path,
                        tmp_path
                    ));
                }

                let repeats = call_history
                    .iter()
                    .filter(|(n, a)| n == &name && a == &args_str)
                    .count()
                    + 1;
                call_history.push((name.clone(), args_str.clone()));
                if is_unproductive_call(&name, &result) {
                    unproductive_streak += 1;
                } else {
                    unproductive_streak = 0;
                }
                if let Some(note) = loop_guard_notes(&name, repeats, unproductive_streak) {
                    tracing::warn!(
                        "loop guard tripped on {name}: repeats={repeats} streak={unproductive_streak}"
                    );
                    result_str.push_str(&format!("\n{note}"));
                }

                tracing::info!(
                    "tool {name} result len={} ok={}",
                    result_str.len(),
                    result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false)
                );

                if let Some(tx) = &stream_tx {
                    let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let _ = tx
                        .send(crate::provider::ProviderEvent::ToolResultDelta {
                            id: id.clone(),
                            content: result_str.clone(),
                            ok,
                        })
                        .await;
                }

                let mut tool_msg = ChatMessage::text("tool", result_str.clone());
                tool_msg.tool_call_id = Some(id.clone());
                tool_msg.name = Some(name.clone());
                messages.push(tool_msg);
                if let Some(b64) = result.get("base64").and_then(|v| v.as_str()) {
                    if b64.len() < 800_000 {
                        pending_images.push((name.clone(), b64.to_string()));
                    } else {
                        tracing::warn!(
                            "skipping vision for {name}: base64 too large {} chars",
                            b64.len()
                        );
                    }
                }
                if let Some(ref s) = store {
                    let v: Value =
                        serde_json::from_str(&result_str).unwrap_or(json!({"raw": result_str}));
                    let _ = s.settle_tool_call(&id, &v);

                    let _ = s.append_message_full(
                        &session_id,
                        "tool",
                        &result_str,
                        None,
                        None,
                        None,
                        Some(&id),
                    );
                }
            }

            if !pending_images.is_empty() {
                let text = format!(
                    "Vision feedback for {} image(s): base64 attached below",
                    pending_images.len()
                );
                let mut parts = vec![serde_json::json!({"type": "text", "text": text})];
                for (tool_name, b64) in pending_images {
                    let url = format!("data:image/png;base64,{b64}");
                    parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": url}}));
                    tracing::info!(
                        "injected vision image for tool {tool_name} len {}",
                        b64.len()
                    );
                }
                messages.push(ChatMessage {
                    role: "user".into(),
                    content: Value::Array(parts),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                });
            }
        }

        Ok(final_text)
    }

    pub async fn ask(&self, prompt: &str, model: &str) -> anyhow::Result<String> {
        self.run(prompt, model, None).await
    }
}

pub fn config_candidates() -> Vec<String> {
    let mut cands = vec![
        "vioraharness.json".to_string(),
        ".vioraharness/vioraharness.json".to_string(),
    ];
    if let Ok(cwd) = std::env::current_dir() {
        cands.push(cwd.join("vioraharness.json").to_string_lossy().to_string());

        let mut cur = cwd.clone();
        loop {
            let cand = cur.join("vioraharness.json");
            let s = cand.to_string_lossy().to_string();
            if !cands.contains(&s) {
                cands.push(s);
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
            if cur.to_string_lossy().len() < 2 {
                break;
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        cands.push(format!("{home}/.config/vioraharness/vioraharness.json"));
    }
    if let Ok(p) = std::env::var("VIORAHARNESS_CONFIG") {
        if !p.trim().is_empty() {
            cands.insert(0, p);
        }
    }
    cands
}

fn load_rules_from_config() -> Vec<Rule> {
    for cand in config_candidates() {
        if let Ok(content) = std::fs::read_to_string(cand) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                if let Some(perms) = val.get("permissions") {
                    return rules_from_json(perms);
                }
            }
        }
    }

    vec![
        Rule::new(Decision::Allow, "read"),
        Rule::new(Decision::Allow, "glob"),
        Rule::new(Decision::Allow, "grep"),
        Rule::new(Decision::Allow, "schematic_query"),
        Rule::new(Decision::Allow, "symbol_search"),
        Rule::new(Decision::Allow, "footprint_list"),
        Rule::new(Decision::Allow, "netlist_validate"),
        Rule::new(Decision::Allow, "schematic_render"),
        Rule::new(Decision::Allow, "pcb_render"),
        Rule::new(Decision::Allow, "task"),
        Rule::new(Decision::Allow, "skill"),
        Rule::new(Decision::Ask, "write"),
        Rule::new(Decision::Allow, "bash"),
        Rule::new(Decision::Deny, "bash rm -rf*"),
        Rule::new(Decision::Deny, "bash sudo:*"),
    ]
}

fn is_unproductive_call(name: &str, result: &Value) -> bool {
    if result.get("ok").and_then(|v| v.as_bool()) == Some(false) {
        return true;
    }
    match name {
        "bash" => result.get("bytes").and_then(|v| v.as_u64()) == Some(0),
        "grep" => result
            .get("hits")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false),
        "glob" => result
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(false),
        _ => false,
    }
}

fn sanitize_tool_contiguity(messages: &mut Vec<ChatMessage>) {
    use std::collections::HashSet;
    let mut declared: HashSet<String> = HashSet::new();
    for m in messages.iter() {
        if m.role == "assistant" {
            if let Some(tcs) = &m.tool_calls {
                for t in tcs {
                    declared.insert(t.id.clone());
                }
            }
        }
    }
    let mut answered: HashSet<String> = HashSet::new();
    for m in messages.iter() {
        if m.role == "tool" {
            if let Some(id) = &m.tool_call_id {
                if declared.contains(id) {
                    answered.insert(id.clone());
                }
            }
        }
    }
    messages.retain(|m| {
        if m.role != "tool" {
            return true;
        }
        match &m.tool_call_id {
            Some(id) => declared.contains(id),
            None => false,
        }
    });
    for m in messages.iter_mut() {
        if m.role == "assistant" {
            if let Some(tcs) = m.tool_calls.as_mut() {
                tcs.retain(|t| answered.contains(&t.id));
                if tcs.is_empty() {
                    m.tool_calls = None;
                }
            }
        }
    }
}

struct ApprovalGuard {
    prev: Option<String>,
}

impl ApprovalGuard {
    fn hold() -> Self {
        let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
        std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
        Self { prev }
    }
}

impl Drop for ApprovalGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
            None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
        }
    }
}

fn auto_allow_on() -> bool {
    std::env::var("VIORAHARNESS_AUTO_ALLOW")
        .map(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

async fn execute_approved(name: &str, args: Value) -> Value {
    let prev = std::env::var("VIORAHARNESS_APPROVED_CALL").ok();
    std::env::set_var("VIORAHARNESS_APPROVED_CALL", "1");
    let out = tools::execute_tool(name, args).await;
    match prev {
        Some(v) => std::env::set_var("VIORAHARNESS_APPROVED_CALL", v),
        None => std::env::remove_var("VIORAHARNESS_APPROVED_CALL"),
    }
    out
}

fn normalize_tool_args(args: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(args).is_ok() {
        args.to_string()
    } else {
        tracing::warn!("normalizing invalid tool-call args to {{}}: {args}");
        "{}".into()
    }
}

fn loop_guard_notes(name: &str, repeats: usize, unproductive_streak: usize) -> Option<String> {
    if repeats >= 3 {
        return Some(format!(
            "[harness: you already ran this exact `{name}` call {repeats} times — do NOT run it again; answer from what you have or try a different approach]"
        ));
    }
    if unproductive_streak >= 4 {
        return Some(format!(
            "[harness: {unproductive_streak} consecutive unproductive tool calls (empty results / errors) — stop circling; answer the user with what you already found]"
        ));
    }
    None
}

fn is_read_only_tool(name: &str) -> bool {
    matches!(
        name,
        "read"
            | "glob"
            | "grep"
            | "schematic_query"
            | "pcb_query"
            | "symbol_search"
            | "footprint_list"
            | "netlist_validate"
            | "netlist_run"
            | "netlist_to_schematic"
            | "erc"
            | "pcb_validate"
            | "raw_export"
            | "viora"
            | "task"
            | "skill"
            | "question"
            | "todowrite"
            | "webfetch"
            | "websearch"
            | "schematic_render"
            | "pcb_render"
    )
}

fn bash_command(args_str: &str) -> String {
    if args_str.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(args_str) {
            if let Some(c) = v.get("command").and_then(|c| c.as_str()) {
                return c.to_string();
            }
        }
    }
    args_str.to_string()
}

fn redirect_writes_external(seg: &str) -> bool {
    let b = seg.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'>' {
            i += 1;
            continue;
        }

        if i + 1 < b.len() && b[i + 1] == b'&' {
            i += 2;
            continue;
        }

        let mut j = i + 1;
        if j < b.len() && b[j] == b'>' {
            j += 1;
        }
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        let mut k = j;
        while k < b.len() && !b" \t;&|<>()".contains(&b[k]) {
            k += 1;
        }
        let mut target = seg[j..k].trim().to_string();

        target = target.trim_start_matches('&').to_string();
        for q in ['\'', '"'] {
            if target.len() >= 2 && target.starts_with(q) && target.ends_with(q) {
                target = target[1..target.len() - 1].to_string();
            }
        }
        i = k.max(i + 1);
        if target.is_empty() {
            continue;
        }
        if matches!(
            target.as_str(),
            "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
        ) {
            continue;
        }
        let resolved = crate::tools::viora::resolve_path(&target);
        if !crate::tools::viora::is_within_root(&resolved) {
            return true;
        }
    }
    false
}

/// (every `2>&1` command would look compound and fail safety checks).
pub(crate) fn split_shell_segments(cmd: &str) -> Vec<&str> {
    let b = cmd.as_bytes();
    let mut segs: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < b.len() {
        let c = b[i];

        if c == b'\\' && i + 1 < b.len() {
            i += 2;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            i += 1;
            continue;
        }
        match c {
            b';' => {
                segs.push(&cmd[start..i]);
                i += 1;
                start = i;
            }
            b'|' => {
                segs.push(&cmd[start..i]);
                i += 1;
                if i < b.len() && b[i] == b'|' {
                    i += 1;
                }
                start = i;
            }
            b'&' => {
                if i + 1 < b.len() && b[i + 1] == b'&' {
                    segs.push(&cmd[start..i]);
                    i += 2;
                    start = i;
                } else {
                    let prev_gt = i > 0 && b[i - 1] == b'>';
                    let next_gt = i + 1 < b.len() && b[i + 1] == b'>';
                    if prev_gt || next_gt {
                        i += 1;
                    } else {
                        segs.push(&cmd[start..i]);
                        i += 1;
                        start = i;
                    }
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    segs.push(&cmd[start..]);
    segs.into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_dangerous_bash(args_str: &str) -> bool {
    let raw = bash_command(args_str);
    let lower = raw.trim().to_lowercase();
    if lower.contains(":(){") || lower.contains(": (){") {
        return true;
    }

    let mut cmd = lower.as_str();
    for wrapper in ["sh -c ", "bash -c "] {
        if let Some(rest) = cmd.strip_prefix(wrapper) {
            let rest = rest.trim();
            cmd = rest
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .unwrap_or(rest);
            break;
        }
    }
    for seg in split_shell_segments(cmd) {
        if segment_is_dangerous(seg) {
            return true;
        }
    }
    false
}

const DANGEROUS_CMDS: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "wipe", "srm", "dd", "mkfs", "wipefs", "mkswap", "fdisk",
    "parted", "shutdown", "reboot", "halt", "poweroff",
];

fn interpreter_danger(seg: &str) -> bool {
    const INTERP: &[&str] = &[
        "python", "python2", "python3", "node", "nodejs", "perl", "ruby", "php", "lua", "R",
        "Rscript", "deno", "bun",
    ];
    const PATTERNS: &[&str] = &[
        "os.remove",
        "os.unlink",
        "os.removedirs",
        "os.rmdir",
        "shutil.rmtree",
        "unlink",
        "rmtree",
        ".unlink(",
        "fs.rm",
        "fs.rmdir",
        "rimraf",
        "child_process",
        "file.delete",
        "fileutils",
        "shell_exec",
        "passthru",
        "os.system",
        "os.popen",
        "os.spawn",
        "subprocess.",
        "exec(",
        "system(",
        "popen(",
        "spawn(",
    ];
    let seg = strip_wrappers(seg.trim_start_matches('!').trim());
    let first = seg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches("./");
    let base = first.rsplit('/').next().unwrap_or(first);
    let is_interp =
        INTERP.contains(&base) || base.starts_with("python") || base.starts_with("node");
    if !is_interp {
        return false;
    }
    PATTERNS.iter().any(|p| seg.contains(p))
}

fn segment_is_dangerous(seg: &str) -> bool {
    let seg = strip_wrappers(seg.trim_start_matches('!').trim());

    let first = seg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches("./");
    let base = first.rsplit('/').next().unwrap_or(first);

    if base == "exec" {
        let raw_first = seg.split_whitespace().next().unwrap_or("");
        let rest = seg[raw_first.len()..].trim();
        if !rest.is_empty() && !rest.starts_with('>') {
            return segment_is_dangerous(rest);
        }
        return false;
    }

    if DANGEROUS_CMDS.contains(&base) || base.starts_with("mkfs.") || base.starts_with("mkfs-") {
        return true;
    }

    if (base == "systemctl" || base == "service")
        && ["poweroff", "reboot", "halt"]
            .iter()
            .any(|w| seg.contains(w))
    {
        return true;
    }

    let toks: Vec<&str> = seg.split_whitespace().collect();
    if base == "find"
        && toks
            .iter()
            .any(|t| matches!(*t, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        return true;
    }
    if base == "xargs"
        && toks[1..].iter().any(|t| {
            let b = t.trim_start_matches("./").rsplit('/').next().unwrap_or(t);
            DANGEROUS_CMDS.contains(&b)
        })
    {
        return true;
    }

    if interpreter_danger(seg) {
        return true;
    }
    false
}

pub(crate) fn strip_wrappers(mut seg: &str) -> &str {
    const WRAPPERS: &[&str] = &[
        "sudo", "nohup", "timeout", "nice", "ionice", "stdbuf", "setsid", "env", "command",
    ];
    fn is_assign(tok: &str) -> bool {
        tok.contains('=')
            && !tok.starts_with('=')
            && tok
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && tok
                .chars()
                .take_while(|c| *c != '=')
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    fn is_flag_or_num(tok: &str) -> bool {
        if tok.len() > 1 && tok.starts_with('-') {
            return true;
        }
        let num = match tok.strip_suffix(['s', 'm', 'h', 'd']) {
            Some(n) if !n.is_empty() => n,
            _ => tok,
        };
        !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.')
    }
    fn strip_leading_assigns(mut seg: &str) -> &str {
        loop {
            let tok_end = seg.find([' ', '\t']).unwrap_or(seg.len());
            if !is_assign(&seg[..tok_end]) {
                break;
            }
            seg = seg[tok_end..].trim();
        }
        seg
    }

    seg = strip_leading_assigns(seg);
    loop {
        let mut progress = false;
        for w in WRAPPERS {
            if let Some(rest) = seg
                .strip_prefix(w)
                .filter(|r| r.starts_with(' ') || r.starts_with('\t'))
            {
                seg = rest.trim();
                progress = true;
                break;
            }
        }

        if progress {
            loop {
                let tok_end = seg.find([' ', '\t']).unwrap_or(seg.len());
                let tok = &seg[..tok_end];
                if tok == "--" {
                    seg = seg[tok_end..].trim();
                    break;
                }
                if is_assign(tok) || is_flag_or_num(tok) {
                    seg = seg[tok_end..].trim();
                } else {
                    break;
                }
            }
        }
        if !progress {
            break;
        }
    }
    seg
}

fn is_safe_bash(args_str: &str) -> bool {
    let cmd = if args_str.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(args_str) {
            v.get("command")
                .and_then(|c| c.as_str())
                .unwrap_or(args_str)
                .to_string()
        } else {
            args_str.to_string()
        }
    } else {
        args_str.to_string()
    };
    let lower = cmd.trim().to_lowercase();

    let segments: Vec<&str> = split_shell_segments(&lower);

    let safe_prefixes = &[
        "ls",
        "pwd",
        "cat ",
        "echo ",
        "head ",
        "tail ",
        "wc ",
        "find ",
        "tree",
        "grep ",
        "grep -",
        "awk ",
        "sed ",
        "sort ",
        "uniq ",
        "cut ",
        "tr ",
        "seq ",
        "seq",
        "git status",
        "git log",
        "git branch",
        "git diff",
        "git show",
        "git add",
        "git commit",
        "git clone",
        "git fetch",
        "git pull",
        "git push",
        "git stash",
        "date",
        "uname",
        "env ",
        "printenv",
        "which ",
        "whereis ",
        "command ",
        "type ",
        "file ",
        "stat ",
        "du ",
        "df ",
        "free ",
        "uptime ",
        "uptime",
        "whoami",
        "id ",
        "id",
        "hostname",
        "diff ",
        "cmp ",
        "cd ",
        "cd",
        "mkdir ",
        "touch ",
        "cp ",
        "mv ",
        "chmod ",
        "ln ",
        "tar ",
        "zip ",
        "unzip ",
        "xdg-open",
        "open ",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "firefox",
        "brave",
        "sensible-browser",
        "npm run",
        "npm ",
        "npx ",
        "node ",
        "cargo run",
        "cargo check",
        "cargo test",
        "python ",
        "python3 ",
        "pytest ",
        "pytest",
        "pip ",
        "pip3 ",
        "perl ",
        "ruby ",
        "php ",
        "lua ",
        "go ",
        "make ",
        "ls ",
    ];

    let seg_is_safe = |seg: &str| {
        if redirect_writes_external(seg) {
            return false;
        }

        if interpreter_danger(seg) {
            return false;
        }
        let seg = strip_wrappers(seg);

        if seg == "find" || seg.starts_with("find ") {
            let bad = seg.split_whitespace().any(|t| {
                matches!(
                    t,
                    "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir" | "-fls" | "-fprint"
                )
            });
            if bad {
                return false;
            }
        }

        if seg == "xargs" || seg.starts_with("xargs ") {
            let mut toks = seg.split_whitespace().skip(1).peekable();
            while let Some(t) = toks.peek() {
                if t.starts_with('-') {
                    let flag = toks.next().unwrap();
                    if matches!(flag, "-I" | "--replace" | "-a" | "--arg-file") {
                        toks.next();
                    }
                } else {
                    break;
                }
            }
            let rest: Vec<&str> = toks.collect();
            if rest.is_empty() {
                return true;
            }
            let raw_first = rest[0];
            let base = raw_first
                .trim_start_matches("./")
                .rsplit('/')
                .next()
                .unwrap_or(raw_first);
            let rebuilt = format!("{base}{}", &rest.join(" ")[raw_first.len()..]);
            return safe_prefixes.iter().any(|p| rebuilt.starts_with(p));
        }
        if safe_prefixes.iter().any(|p| seg.starts_with(p)) {
            return true;
        }
        let (first, rest) = match seg.find(' ') {
            Some(i) => (&seg[..i], &seg[i..]),
            None => (seg, ""),
        };
        if first.contains('/') {
            let base = first.rsplit('/').next().unwrap_or(first);
            let rebuilt = format!("{base}{rest}");
            return safe_prefixes.iter().any(|p| rebuilt.starts_with(p));
        }
        false
    };

    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|s| seg_is_safe(s))
}

fn persist_allow_always(tool: &str, args_str: &str) {
    let pattern = if tool == "bash" {
        let cmd = if args_str.trim_start().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(args_str) {
                v.get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        } else {
            args_str.to_string()
        };

        let first = cmd
            .trim()
            .split([' ', '|', '&', ';'])
            .next()
            .unwrap_or(tool)
            .trim()
            .trim_matches(['"', '\'', '`']);
        let base = first.split('/').next_back().unwrap_or(first);
        if base.is_empty() {
            "bash".to_string()
        } else {
            format!("bash {base}*")
        }
    } else if tool == "write" {
        "write".to_string()
    } else {
        tool.to_string()
    };
    for cand in config_candidates() {
        if let Ok(content) = std::fs::read_to_string(&cand) {
            if let Ok(mut val) = serde_json::from_str::<Value>(&content) {
                if let Some(obj) = val.get_mut("permissions").and_then(|p| p.as_object_mut()) {
                    obj.insert(pattern.clone(), Value::String("allow".into()));
                    if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                        let _ = std::fs::write(&cand, pretty);
                        tracing::info!("persist_allow_always: {} -> allow in {}", pattern, cand);
                    }
                    break;
                }
            }
        }
    }
}

fn simple_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:010x}", nanos & 0xffffffffff)
}

fn load_history_from_store(s: &crate::session::SessionStore, session_id: &str) -> Vec<ChatMessage> {
    let mut history = Vec::new();
    if let Ok(stored) = s.get_messages_detailed(session_id) {
        let tool_map = s.get_tool_calls_grouped(session_id).unwrap_or_default();

        let mut legacy_assign: std::collections::HashMap<i64, usize> =
            std::collections::HashMap::new();
        for m in stored {
            let cm = match m.role.as_str() {
                "user" => ChatMessage::text("user", m.content.clone()),
                "assistant" => {
                    let mut c = ChatMessage::text("assistant", m.content.clone());
                    if let Some(cj) = &m.content_json {
                        if let Ok(v) = serde_json::from_str::<Value>(cj) {
                            if v.is_array() {
                                c.content = v;
                            }
                        }
                    }

                    if let Some(calls) = tool_map.get(&m.seq) {
                        let tcs: Vec<crate::provider::ToolCall> = calls
                            .iter()
                            .map(|(id, name, args, sig, _res)| crate::provider::ToolCall {
                                id: id.clone(),
                                call_type: "function".into(),
                                function: crate::provider::FunctionCall {
                                    name: name.clone(),

                                    arguments: normalize_tool_args(args),
                                },
                                thought_signature: sig.clone(),
                            })
                            .collect();
                        if !tcs.is_empty() {
                            c.tool_calls = Some(tcs);
                        }
                    }
                    c
                }
                "tool" => {
                    let mut c = ChatMessage::text("tool", m.content.clone());
                    let mut tid = m.tool_call_id.clone();
                    if tid.is_none() {
                        let owner = tool_map
                            .iter()
                            .filter(|(seq, calls)| **seq < m.seq && !calls.is_empty())
                            .max_by_key(|(seq, _)| **seq)
                            .map(|(seq, _)| *seq);
                        if let Some(oseq) = owner {
                            let calls = &tool_map[&oseq];
                            let k = legacy_assign.get(&oseq).copied().unwrap_or(0);
                            if let Some((id, _, _, _, _)) = calls.get(k.min(calls.len() - 1)) {
                                tid = Some(id.clone());
                                tracing::info!(
                                    "recovered legacy tool_call_id for tool msg seq={}",
                                    m.seq
                                );
                            }
                            legacy_assign.insert(oseq, k + 1);
                        }
                    }
                    c.tool_call_id = tid;
                    c
                }
                _ => ChatMessage::text(&m.role, m.content.clone()),
            };
            history.push(cm);
        }
    }
    history
}

fn splice_compacted_history(
    messages: &mut Vec<ChatMessage>,
    s: &crate::session::SessionStore,
    session_id: &str,
) {
    let system_msg = messages.first().cloned();
    let fresh = load_history_from_store(s, session_id);
    *messages = Vec::with_capacity(fresh.len() + 1);
    if let Some(sys) = system_msg {
        messages.push(sys);
    }
    messages.extend(fresh);
    sanitize_tool_contiguity(messages);
}

fn try_create_store(session_id: &str, _model: &str) -> Option<Arc<crate::session::SessionStore>> {
    let db_path = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    match crate::session::SessionStore::new(&db_path) {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            tracing::warn!(
                "session store init failed ({e}) — continuing without persistence for {session_id}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproductive_call_shapes() {
        assert!(is_unproductive_call(
            "bash",
            &json!({"ok": true, "bytes": 0, "code": 0})
        ));
        assert!(!is_unproductive_call(
            "bash",
            &json!({"ok": true, "bytes": 41})
        ));
        assert!(is_unproductive_call(
            "grep",
            &json!({"ok": true, "hits": []})
        ));
        assert!(!is_unproductive_call(
            "grep",
            &json!({"ok": true, "hits": [{"path": "a"}]})
        ));
        assert!(is_unproductive_call(
            "glob",
            &json!({"ok": true, "files": []})
        ));
        assert!(is_unproductive_call(
            "webfetch",
            &json!({"ok": false, "error": "HTTP 404"})
        ));
        assert!(!is_unproductive_call(
            "websearch",
            &json!({"ok": true, "count": 10})
        ));

        assert!(!is_unproductive_call("read", &json!({"ok": true})));
    }

    fn assistant_with_calls(id: &str) -> ChatMessage {
        let mut m = ChatMessage::text("assistant", "");
        m.tool_calls = Some(vec![crate::provider::ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: crate::provider::FunctionCall {
                name: "websearch".into(),
                arguments: "{}".into(),
            },
            thought_signature: None,
        }]);
        m
    }

    fn tool_msg(id: Option<&str>) -> ChatMessage {
        let mut m = ChatMessage::text("tool", "{}");
        m.tool_call_id = id.map(|s| s.into());
        m
    }

    #[test]
    fn sanitize_keeps_valid_history() {
        let mut msgs = vec![
            ChatMessage::text("user", "hi"),
            assistant_with_calls("call-1"),
            tool_msg(Some("call-1")),
        ];
        sanitize_tool_contiguity(&mut msgs);
        assert_eq!(msgs.len(), 3);
        assert!(msgs[1].tool_calls.is_some());
    }

    #[test]
    fn sanitize_drops_orphans() {
        let mut msgs = vec![
            ChatMessage::text("user", "hi"),
            assistant_with_calls("call-9"),
            tool_msg(Some("call-other")),
            tool_msg(None),
        ];
        sanitize_tool_contiguity(&mut msgs);
        assert_eq!(msgs.len(), 2, "orphan tool msgs dropped");
        assert!(
            msgs[1].tool_calls.is_none(),
            "unanswered calls stripped from assistant msg"
        );
    }

    fn bash_args(cmd: &str) -> String {
        serde_json::json!({"command": cmd}).to_string()
    }

    #[test]
    fn shell_segments_respect_redirects() {
        assert_eq!(
            split_shell_segments("python3 -m pytest test_app.py -v 2>&1"),
            vec!["python3 -m pytest test_app.py -v 2>&1"]
        );
        assert_eq!(
            split_shell_segments("cmd &> /tmp/out"),
            vec!["cmd &> /tmp/out"]
        );
        assert_eq!(split_shell_segments("ls && rm foo"), vec!["ls", "rm foo"]);
        assert_eq!(
            split_shell_segments("a || b; c | d & e"),
            vec!["a", "b", "c", "d", "e"]
        );
        assert_eq!(split_shell_segments("trail &"), vec!["trail"]);

        assert!(is_dangerous_bash(&bash_args("rm x 2>/dev/null")));
    }

    #[test]
    fn redirect_write_policy() {
        assert!(redirect_writes_external("echo hi > /home/jnd/x.txt"));
        assert!(redirect_writes_external("echo hi >> /etc/motd"));
        assert!(redirect_writes_external("make 2> /home/jnd/err.log"));
        assert!(redirect_writes_external("cmd &> /home/jnd/out"));

        for seg in [
            "python3 -m pytest -v 2>&1",
            "ls /x 2>/dev/null",
            "echo hi > /tmp/x",
            "echo hi",
            "cat f | grep x",
        ] {
            assert!(!redirect_writes_external(seg), "not a write: {seg}");
        }

        assert!(!is_safe_bash(&bash_args("echo hi > /home/jnd/x.txt")));
        assert!(is_safe_bash(&bash_args("echo hi > /tmp/x")));
    }

    #[test]
    fn safe_bash_covers_session_commands() {
        for cmd in [
            "python3 -m pytest /tmp/vh_cli_test/test_app.py -v 2>&1",
            "which python3",
            "which python",
            "cd /tmp/vh_cli_test && /usr/bin/python3 -m pytest test_app.py -v 2>&1",
            "ls /usr/bin/python* 2>/dev/null",
            "mkdir -p sub && touch sub/f",
            "pytest -q",
        ] {
            assert!(is_safe_bash(&bash_args(cmd)), "safe: {cmd}");
        }
    }

    #[test]
    fn dangerous_bash_detection() {
        for cmd in [
            "rm -rf /",
            "rm foo",
            "sudo rm -rf /tmp/x",
            "/bin/rm x",
            "rmdir d",
            "shred f",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sda1",
            "wipefs -a /dev/sdb",
            "mkswap /dev/sda2",
            "fdisk /dev/sda",
            "parted /dev/sda rm 1",
            "shutdown now",
            "reboot",
            "halt",
            "poweroff",
            "systemctl poweroff",
            "systemctl reboot",
            "service foo reboot",
            "sh -c 'rm -rf /tmp/x'",
            "ls && rm foo",
            "echo hi; dd if=x of=y",
            "X=1;halt",
            ":(){ :|:& };:",
        ] {
            assert!(is_dangerous_bash(&bash_args(cmd)), "danger: {cmd}");
            assert!(
                is_dangerous_bash(cmd),
                "danger plain form (non-JSON args): {cmd}"
            );
        }
    }

    #[test]
    fn dangerous_bash_smuggled_deletion() {
        for cmd in [
            "unlink /tmp/f",
            "/usr/bin/unlink f",
            "wipe f",
            "srm -v f",
            "find . -delete",
            "find /tmp -name '*.log' -exec rm {} \\;",
            "find . -execdir rm {} \\;",
            "echo hi | xargs rm",
            "nohup rm -f f",
            "timeout 5 rm -rf /tmp/x",
            "FOO=1 rm f",
            "nice -n 5 rm f",
            "sudo unlink f",
            "timeout -- rm f",
        ] {
            assert!(is_dangerous_bash(&bash_args(cmd)), "danger: {cmd}");
        }

        for cmd in [
            "find . -name x",
            "echo hi | xargs ls",
            "nohup ls",
            "timeout 5 pytest -q",
            "FOO=1 pytest -q",
            "env FOO=1 pytest -q",
            "nice -5 pytest -q",
        ] {
            assert!(!is_dangerous_bash(&bash_args(cmd)), "safe: {cmd}");
            assert!(is_safe_bash(&bash_args(cmd)), "safe runs: {cmd}");
        }
    }

    #[test]
    fn safe_gate_blocks_smuggled_execution() {
        for cmd in [
            "find . -delete",
            "find . -exec rm {} \\;",
            "echo hi | xargs rm",
            "xargs -0 shred",
        ] {
            assert!(!is_safe_bash(&bash_args(cmd)), "not safe: {cmd}");
        }
        for cmd in [
            "find . -name x",
            "echo hi | xargs ls",
            "echo hi | xargs -0 -I{} echo {}",
            "xargs",
        ] {
            assert!(is_safe_bash(&bash_args(cmd)), "safe: {cmd}");
        }
    }

    #[test]
    fn dangerous_bash_interpreter_payloads() {
        for cmd in [
            "python3 -c \"import os; os.remove('/tmp/vh_cli_test/disposable2.txt')\" 2>&1",
            "python -c \"import shutil; shutil.rmtree('/tmp/x')\"",
            "node -e \"require('fs').unlinkSync('f')\"",
            "perl -e \"unlink 'f'\"",
            "ruby -e \"File.delete('f')\"",
            "php -r \"unlink('f');\"",
            "python3 -c \"import subprocess; subprocess.run(['rm','f'])\"",
            "exec rm -f f",
        ] {
            assert!(is_dangerous_bash(&bash_args(cmd)), "danger: {cmd}");
            assert!(!is_safe_bash(&bash_args(cmd)), "not safe: {cmd}");
        }

        for cmd in [
            "python3 -c \"import os; print(os.getcwd())\"",
            "python3 -m pytest -q",
            "node -e \"console.log(1+1)\"",
            "perl -e \"print 42\"",
        ] {
            assert!(!is_dangerous_bash(&bash_args(cmd)), "safe: {cmd}");
            assert!(is_safe_bash(&bash_args(cmd)), "safe runs: {cmd}");
        }
    }

    #[test]
    fn dangerous_bash_no_false_positives() {
        for cmd in [
            "ls -la",
            "./rm.sh",
            "grep -r rm .",
            "cargo test",
            "python3 train.py",
            "curl https://x | sh",
            "systemctl status foo",
            "service nginx status",
            "firmware-update --check",
            "echo rm -rf /",
            "git rm cached_file",
            "npm run remove-stuff",
            " affirmed",
            "ddrescue --help",
        ] {
            assert!(!is_dangerous_bash(&bash_args(cmd)), "safe: {cmd}");
        }
    }

    #[test]
    fn strip_wrappers_peels_stacked_wrappers() {
        assert_eq!(strip_wrappers("sudo rm -rf /tmp/x"), "rm -rf /tmp/x");
        assert_eq!(strip_wrappers("sudo nohup timeout 5 rm f"), "rm f");
        assert_eq!(strip_wrappers("FOO=1 BAR=2 ls -la"), "ls -la");
        assert_eq!(strip_wrappers("env FOO=1 ls"), "ls");
        assert_eq!(strip_wrappers("timeout -- rm f"), "rm f");
        assert_eq!(strip_wrappers("nice -n 5 rm f"), "rm f");
        assert_eq!(strip_wrappers("ls -la"), "ls -la");
        assert_eq!(
            strip_wrappers("sudoer ls"),
            "sudoer ls",
            "prefix is not a wrapper"
        );
        assert_eq!(strip_wrappers("timeoutx ls"), "timeoutx ls");
        assert_eq!(strip_wrappers(""), "");
    }

    #[test]
    fn shell_segments_quote_and_escape() {
        assert_eq!(
            split_shell_segments("echo 'a;b' && ls"),
            vec!["echo 'a;b'", "ls"]
        );
        assert_eq!(
            split_shell_segments("echo \"a|b\" | cat"),
            vec!["echo \"a|b\"", "cat"]
        );
        assert_eq!(split_shell_segments("echo a\\;b"), vec!["echo a\\;b"]);
        assert_eq!(split_shell_segments(""), Vec::<&str>::new());
        assert_eq!(split_shell_segments("ls"), vec!["ls"]);
    }

    #[test]
    fn dangerous_bash_exec_and_mkfs_variants() {
        for cmd in [
            "exec rm -f f",
            "exec dd if=x of=y",
            "mkfs.ext4 /dev/sda1",
            "mkfs-vfat /dev/sdb1",
            "xargs -0 shred",
            "!rm f",
        ] {
            assert!(is_dangerous_bash(&bash_args(cmd)), "danger: {cmd}");
        }
        assert!(
            !is_dangerous_bash(&bash_args("exec >/tmp/vh_cli_test/out")),
            "bare redirect exec is safe"
        );
    }

    #[test]
    fn tool_args_normalization() {
        assert_eq!(normalize_tool_args("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(normalize_tool_args("{}"), "{}");

        assert_eq!(normalize_tool_args(",\"path\":\"x\"}"), "{}");
        assert_eq!(normalize_tool_args(""), "{}");
        assert_eq!(normalize_tool_args("not json at all"), "{}");
    }

    #[test]
    fn auto_allow_flag_parsing() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for v in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            std::env::set_var("VIORAHARNESS_AUTO_ALLOW", v);
            assert!(auto_allow_on(), "{v}");
        }
        for v in ["0", "false", "no", "", "maybe"] {
            std::env::set_var("VIORAHARNESS_AUTO_ALLOW", v);
            assert!(!auto_allow_on(), "{v}");
        }
        std::env::remove_var("VIORAHARNESS_AUTO_ALLOW");
        assert!(!auto_allow_on());
    }

    #[test]
    fn approval_guard_holds_and_restores() {
        use crate::tools::viora::approved_call;
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("VIORAHARNESS_APPROVED_CALL");
        assert!(!approved_call());
        {
            let _g = ApprovalGuard::hold();
            assert!(approved_call());
        }
        assert!(!approved_call(), "restored after drop");

        std::env::set_var("VIORAHARNESS_APPROVED_CALL", "yes");
        {
            let _g = ApprovalGuard::hold();
            assert!(approved_call());
        }
        assert_eq!(
            std::env::var("VIORAHARNESS_APPROVED_CALL").as_deref(),
            Ok("yes")
        );
        std::env::remove_var("VIORAHARNESS_APPROVED_CALL");
    }

    #[test]
    fn guard_notes_thresholds() {
        assert!(loop_guard_notes("bash", 1, 0).is_none());
        assert!(loop_guard_notes("bash", 2, 3).is_none());
        let n = loop_guard_notes("bash", 3, 0).expect("repeat trips");
        assert!(n.contains("bash") && n.contains("3 times"), "{n}");

        let n = loop_guard_notes("bash", 3, 5).expect("repeat wins");
        assert!(n.contains("times"), "{n}");
        let n = loop_guard_notes("grep", 1, 4).expect("streak trips");
        assert!(n.contains("4 consecutive"), "{n}");
    }
}
