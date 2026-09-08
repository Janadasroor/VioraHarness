use super::*;

pub(crate) fn load_history_from_store(
    s: &crate::session::SessionStore,
    session_id: &str,
) -> Vec<ChatMessage> {
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

pub(crate) fn splice_compacted_history(
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

pub(crate) fn try_create_store(
    session_id: &str,
    _model: &str,
) -> Option<Arc<crate::session::SessionStore>> {
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
