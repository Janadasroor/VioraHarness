// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub mod engine;
#[cfg(test)]
pub use engine::clear_question_sender;
pub use engine::{
    decide, get_interactive_sender, get_question_sender, rules_from_json, set_interactive_sender,
    set_question_sender, validate_questions, wildcard_match, Decision, InteractiveAsk,
    InteractiveDecision, PendingQuestion, QuestionAnswer, QuestionItem, QuestionOption,
    QuestionResult, Rule,
};
