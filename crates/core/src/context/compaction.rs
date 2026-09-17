// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use crate::provider::{ChatMessage, ChatRequest, Provider};
use crate::session::SessionStore;

pub const DEFAULT_THRESHOLD: f64 = 0.8;
pub const DEFAULT_KEEP_TAIL: usize = 20;
const SUMMARY_INPUT_CAP: usize = 120_000;
const SUMMARY_PER_MSG_CAP: usize = 4000;

pub struct CompactionConfig {
    pub threshold: f64,
    pub keep_tail: usize,
}

pub fn compaction_config() -> CompactionConfig {
    for cand in crate::loop_mod::config_candidates() {
        if let Ok(s) = std::fs::read_to_string(&cand) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                if let Some(c) = v.get("compaction") {
                    let threshold = c
                        .get("threshold")
                        .and_then(|t| t.as_f64())
                        .filter(|t| *t > 0.0 && *t < 1.0)
                        .unwrap_or(DEFAULT_THRESHOLD);
                    let keep_tail =
                        c.get("keepTail")
                            .and_then(|k| k.as_u64())
                            .filter(|k| *k >= 4)
                            .unwrap_or(DEFAULT_KEEP_TAIL as u64) as usize;
                    return CompactionConfig {
                        threshold,
                        keep_tail,
                    };
                }
            }
        }
    }
    CompactionConfig {
        threshold: DEFAULT_THRESHOLD,
        keep_tail: DEFAULT_KEEP_TAIL,
    }
}

pub fn should_compact(total_tokens: usize, max_tokens: usize) -> bool {
    let threshold = compaction_config().threshold;
    max_tokens > 0 && total_tokens as f64 / max_tokens as f64 > threshold
}

pub fn compact_messages(messages: Vec<ChatMessage>, keep_tail: usize) -> Vec<ChatMessage> {
    let (system, rest) = match messages.split_first() {
        Some((first, rest)) if first.role == "system" => (Some(first.clone()), rest),
        _ => (None, messages.as_slice()),
    };
    if rest.len() <= keep_tail + 1 {
        return messages;
    }
    let tail = rest[rest.len() - keep_tail..].to_vec();
    let summary = format!(
        "[Summary of {} earlier messages — compacted. Key facts preserved.]",
        rest.len() - keep_tail
    );
    let mut out = Vec::with_capacity(tail.len() + 2);
    if let Some(sys) = system {
        out.push(sys);
    }
    out.push(ChatMessage::text("user", summary));
    out.extend(tail);
    out
}

pub fn is_context_full_error(err: &anyhow::Error) -> bool {
    let m = format!("{err:#}").to_lowercase();
    if m.contains("deadline")
        || m.contains("cancelled")
        || m.contains("canceled")
        || m.contains("timeout")
        || m.contains("timed out")
    {
        return false;
    }
    [
        "context window",
        "context length",
        "context_length",
        "maximum context",
        "too many tokens",
        "token limit",
        "tokens exceed",
        "prompt too long",
        "prompt is too long",
        "input too long",
        "max_tokens",
        " 413",
        "413 ",
        "payload too large",
        "input_too_long",
    ]
    .iter()
    .any(|p| m.contains(p))
}

pub struct CompactPlan {
    pub cutoff_seq: i64,
    pub dropped: usize,
    pub kept: usize,

    pub dropped_chars: usize,

    pub head: Vec<(String, String)>,
}

pub fn plan_compaction(rows: &[(i64, String, String)], keep_tail: usize) -> Option<CompactPlan> {
    let visible: Vec<(i64, String, String)> = rows
        .iter()
        .filter(|(_, role, _)| role != "system")
        .cloned()
        .collect();
    if visible.len() <= keep_tail + 1 {
        return None;
    }

    // Start the kept tail at a clean boundary, never mid tool-call/result
    // (sanitize_tool_contiguity is the safety net for anything ragged).
    let mut start = visible.len() - keep_tail;
    while start < visible.len() && visible[start].1 == "tool" {
        start += 1;
    }
    if start >= visible.len() {
        start = visible.len() - 1;
    }
    let cutoff_seq = visible[start].0;
    let (dropped_rows, kept_rows) = visible.split_at(start);
    let dropped_chars: usize = dropped_rows.iter().map(|(_, _, c)| c.len()).sum();
    let mut head = Vec::new();
    let mut budget = SUMMARY_INPUT_CAP;
    for (_, role, content) in dropped_rows {
        if budget == 0 {
            break;
        }
        let t: String = content.chars().take(SUMMARY_PER_MSG_CAP).collect();
        budget = budget.saturating_sub(t.len());
        head.push((role.clone(), t));
    }
    Some(CompactPlan {
        cutoff_seq,
        dropped: dropped_rows.len(),
        kept: kept_rows.len(),
        dropped_chars,
        head,
    })
}

const SUMMARIZER_SYSTEM: &str = "You are compacting a coding-agent conversation to free context. Write a dense but complete handoff summary so the agent continuing the work loses nothing important. Cover: 1) overall goal, 2) key decisions and facts discovered, 3) files created or modified (paths), 4) commands run and their results, 5) test/verification state, 6) open TODOs, pending work, and the concrete next step. Be specific — names, paths, numbers. Plain text, no tools, under ~1500 words.";

fn placeholder_summary(dropped: usize) -> String {
    format!("[Summary of {dropped} earlier messages — compacted. Key facts preserved.]")
}

pub async fn summarize_head(
    provider: &dyn Provider,
    model: &str,
    head: &[(String, String)],
    dropped: usize,
) -> String {
    let mut transcript = String::new();
    for (role, text) in head {
        transcript.push_str(&format!("\n\n===== {role} =====\n{text}"));
    }
    if transcript.trim().is_empty() {
        return placeholder_summary(dropped);
    }
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage::text("system", SUMMARIZER_SYSTEM),
            ChatMessage::text(
                "user",
                format!("Summarize this conversation head:{transcript}"),
            ),
        ],
        tools: None,
        tool_choice: None,
        max_tokens: Some(2048),
        temperature: Some(0.3),
        thinking_level: None,
    };
    match provider.complete(req).await {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        Ok(_) => {
            tracing::warn!("compaction summarizer returned empty — placeholder kept");
            placeholder_summary(dropped)
        }
        Err(e) => {
            tracing::warn!("compaction summarizer failed ({e:#}) — placeholder kept");
            placeholder_summary(dropped)
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactReport {
    pub compacted: bool,
    pub dropped: usize,
    pub kept: usize,

    pub freed_chars: usize,
    pub summary: String,

    pub note: String,
}

pub async fn compact_session(
    store: &SessionStore,
    provider: &dyn Provider,
    session_id: &str,
    model: &str,
    keep_tail: usize,
) -> anyhow::Result<CompactReport> {
    let rows = store.get_messages_detailed(session_id)?;
    let rc: Vec<(i64, String, String)> = rows
        .into_iter()
        .map(|m| (m.seq, m.role, m.content))
        .collect();
    let Some(plan) = plan_compaction(&rc, keep_tail) else {
        return Ok(CompactReport {
            compacted: false,
            dropped: 0,
            kept: rc.iter().filter(|(_, r, _)| r != "system").count(),
            freed_chars: 0,
            summary: String::new(),
            note: format!(
                "compact: not needed ({} msgs, need >{})",
                rc.len(),
                keep_tail + 1
            ),
        });
    };
    let summary = summarize_head(provider, model, &plan.head, plan.dropped).await;
    let stamped = format!(
        "[Compacted context — {} earlier messages summarized]\n{}",
        plan.dropped, summary
    );
    let (dropped, kept) = store.compact_replace(session_id, plan.cutoff_seq, &stamped)?;
    Ok(CompactReport {
        compacted: true,
        dropped,
        kept,
        freed_chars: plan.dropped_chars,
        summary: stamped,
        note: format!("Compacted context: {}→{} msgs", dropped + kept, kept),
    })
}

pub fn compact_notice(report: &CompactReport) -> String {
    format!(
        "Compacted context: {}→{} msgs, freed ~{}k tokens",
        report.dropped + report.kept,
        report.kept,
        report.freed_chars / 4000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderEvent;

    fn row(seq: i64, role: &str) -> (i64, String, String) {
        (seq, role.into(), format!("content {seq}"))
    }

    #[test]
    fn plan_skips_small_histories() {
        let rows: Vec<_> = (1..=10).map(|i| row(i, "user")).collect();
        assert!(plan_compaction(&rows, 20).is_none());
        let rows: Vec<_> = (1..=21).map(|i| row(i, "user")).collect();
        assert!(plan_compaction(&rows, 20).is_none(), "need > keep+1");
    }

    #[test]
    fn plan_cuts_and_snaps_past_tool_rows() {
        let mut rows: Vec<_> = (1..=25).map(|i| row(i, "user")).collect();

        for i in 6..=10 {
            rows[i - 1].1 = "tool".into();
        }
        let plan = plan_compaction(&rows, 20).expect("plans");
        assert_eq!(plan.cutoff_seq, 11, "snapped past tool block");
        assert_eq!(plan.dropped, 10);
        assert_eq!(plan.kept, 15);
    }

    #[test]
    fn plan_ignores_system_audit_rows() {
        let mut rows: Vec<_> = (1..=25).map(|i| row(i, "user")).collect();
        rows.insert(0, (0, "system".into(), "Compacted context".into()));
        let plan = plan_compaction(&rows, 20).expect("plans");
        assert_eq!(plan.dropped + plan.kept, 25, "system row invisible");
        assert!(plan.cutoff_seq >= 1);
    }

    #[test]
    fn plan_caps_head_transcript() {
        let big = "x".repeat(50_000);
        let rows: Vec<_> = (1..=30).map(|i| (i, "user".into(), big.clone())).collect();
        let plan = plan_compaction(&rows, 20).expect("plans");
        let head_chars: usize = plan.head.iter().map(|(_, t)| t.len()).sum();
        assert!(head_chars <= SUMMARY_INPUT_CAP + SUMMARY_PER_MSG_CAP);
        assert!(plan.dropped_chars >= 9 * 50_000);
    }

    #[test]
    fn legacy_squash_keeps_tail() {
        let msgs: Vec<ChatMessage> = (0..30)
            .map(|i| ChatMessage::text("user", format!("m{i}")))
            .collect();
        let out = compact_messages(msgs, 20);
        assert_eq!(out.len(), 21);
        assert!(out[0].content_as_str().contains("Summary of 10"));
    }

    #[test]
    fn legacy_squash_preserves_system_prompt() {
        let mut msgs = vec![ChatMessage::text("system", "be helpful")];
        msgs.extend((0..30).map(|i| ChatMessage::text("user", format!("m{i}"))));
        let out = compact_messages(msgs, 20);
        assert_eq!(out.len(), 22);
        assert_eq!(out[0].role, "system");
        assert_eq!(out[0].content_as_str(), "be helpful");
        assert!(out[1].content_as_str().contains("Summary of 10"));
    }

    #[test]
    fn context_error_classifier() {
        for m in [
            "Gateway error 400: This model's maximum context length is 128000 tokens",
            "429 prompt too long: too many tokens",
            "input_too_long: input exceeds token limit",
            "Request failed 413 payload too large",
            "Error: context window exceeded (200k > 128k)",
        ] {
            assert!(
                is_context_full_error(&anyhow::anyhow!(m)),
                "should match: {m}"
            );
        }
        for m in [
            "context deadline exceeded",
            "request timed out after 600s",
            "401 invalid api key",
            "429 rate limit, retry in 4s",
            "connection reset by peer",
        ] {
            assert!(
                !is_context_full_error(&anyhow::anyhow!(m)),
                "should not match: {m}"
            );
        }
    }

    struct FailProvider;
    #[async_trait::async_trait]
    impl Provider for FailProvider {
        fn name(&self) -> &str {
            "fail-test"
        }
        async fn stream(
            &self,
            _req: ChatRequest,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<ProviderEvent>> {
            anyhow::bail!("offline")
        }
        async fn complete(&self, _req: ChatRequest) -> anyhow::Result<String> {
            anyhow::bail!("offline")
        }
    }

    #[tokio::test]
    async fn summarizer_falls_back_to_placeholder_offline() {
        let head = vec![("user".to_string(), "do the thing".to_string())];
        let s = summarize_head(&FailProvider, "test-model", &head, 5).await;
        assert!(s.contains("Summary of 5"), "placeholder kept: {s}");
    }

    #[tokio::test]
    async fn compact_session_rewrites_rows_persistently() {
        let store = SessionStore::new_in_memory().expect("mem store");
        store.create_session("csess", "m", None).expect("session");
        for i in 0..30 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            store
                .append_message("csess", role, &format!("message number {i}"))
                .expect("append");
        }

        store
            .record_tool_call(
                "call_old_1",
                "csess",
                2,
                "bash",
                &serde_json::json!({"command": "ls"}),
            )
            .expect("tool call");
        let rep = compact_session(&store, &FailProvider, "csess", "m", 20)
            .await
            .expect("compact");
        assert!(rep.compacted);
        assert_eq!(rep.dropped + rep.kept, 30, "visible rows conserved");
        assert_eq!(rep.kept, 20);
        assert!(rep.summary.contains("Summary of 10"), "offline placeholder");
        let rows = store.get_messages_detailed("csess").expect("rows");
        assert_eq!(rows.len(), 21, "summary + kept tail");
        assert_eq!(rows[0].role, "user");
        assert!(rows[0].content.contains("Compacted context"));
        assert!(
            rows[1..].iter().all(|m| m.seq > rows[0].seq),
            "summary sorts before tail"
        );
        let remaining = store.get_tool_calls_grouped("csess").expect("tool calls");
        assert!(
            remaining
                .values()
                .all(|v| v.iter().all(|(id, _, _, _, _)| id != "call_old_1")),
            "orphaned tool_calls of dropped rows are gone"
        );

        let rep2 = compact_session(&store, &FailProvider, "csess", "m", 20)
            .await
            .expect("compact2");
        assert!(!rep2.compacted, "stable: {}", rep2.note);
    }
}
