// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use serde_json::{json, Value};

pub async fn ask_question(id: &str, args: Value) -> Value {
    let items = match crate::permissions::validate_questions(&args) {
        Ok(items) => items,
        Err(e) => return json!({"ok": false, "error": format!("invalid question args: {e}")}),
    };
    let in_tui = std::env::var("VIORAHARNESS_TUI").is_ok();
    let sender = crate::permissions::get_question_sender();
    if !(in_tui && sender.is_some()) {
        let headers: Vec<String> = items
            .iter()
            .map(|q| {
                if q.header.is_empty() {
                    q.question.clone()
                } else {
                    format!("{}: {}", q.header, q.question)
                }
            })
            .collect();
        return json!({
            "ok": false,
            "interactive": false,
            "error": "question needs an interactive TUI (no user to ask in this headless run). Proceed with your best judgment and state your assumptions.",
            "unanswered": headers,
        });
    }
    let sender = sender.unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let req = crate::permissions::PendingQuestion {
        id: id.to_string(),
        questions: items,
        tx,
    };
    if sender.send(req).await.is_err() {
        return json!({"ok": false, "error": "question channel closed; proceed with best judgment"});
    }

    match rx.await {
        Ok(crate::permissions::QuestionResult::Answered(answers)) => {
            json!({"ok": true, "answers": answers}
            )
        }
        Ok(crate::permissions::QuestionResult::Cancelled) => {
            json!({"ok": false, "cancelled": true, "error": "user cancelled the question; proceed with best judgment"})
        }
        Err(_) => {
            json!({"ok": false, "error": "question channel dropped; proceed with best judgment"})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn headless_fails_closed_with_guidance() {
        let _g = lock_tests();

        assert!(std::env::var("VIORAHARNESS_TUI").is_err());
        assert!(crate::permissions::get_question_sender().is_none());
        let r = ask_question(
            "call_1",
            json!({"questions": [{"question": "A or B?", "options": [{"label": "A"}, {"label": "B"}]}]}),
        )
        .await;
        assert_eq!(r["ok"], false);
        assert_eq!(r["interactive"], false);
        assert!(r["error"].as_str().unwrap().contains("best judgment"));
    }

    #[tokio::test]
    async fn invalid_args_rejected_before_channel() {
        let r = ask_question("call_1", json!({"questions": []})).await;
        assert_eq!(r["ok"], false);
        assert!(r["error"].as_str().unwrap().contains("must not be empty"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn answered_roundtrip_via_channel() {
        let _g = lock_tests();

        std::env::set_var("VIORAHARNESS_TUI", "1");
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        crate::permissions::set_question_sender(tx);
        let handle = tokio::spawn(async move {
            ask_question(
                "call_9",
                json!({"questions": [{"question": "Q?", "options": [{"label": "Yes"}, {"label": "No"}]}]}),
            )
            .await
        });

        let req = rx.recv().await.expect("request sent");
        assert_eq!(req.id, "call_9");
        assert_eq!(req.questions.len(), 1);
        let _ = req
            .tx
            .send(crate::permissions::QuestionResult::Answered(vec![
                crate::permissions::QuestionAnswer {
                    question: "Q?".into(),
                    selected: vec!["Yes".into()],
                    custom: None,
                },
            ]));
        let r = handle.await.unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["answers"][0]["selected"][0], "Yes");

        std::env::remove_var("VIORAHARNESS_TUI");
        crate::permissions::clear_question_sender();
    }
}
