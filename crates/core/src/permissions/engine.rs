use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

impl std::str::FromStr for Decision {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            _ => Err(format!("unknown decision: {s}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub decision: Decision,
    pub pattern: String,
    pub tool: Option<String>,
}

impl Rule {
    pub fn new(decision: Decision, pattern: impl Into<String>) -> Self {
        Self {
            decision,
            pattern: pattern.into(),
            tool: None,
        }
    }
    pub fn for_tool(mut self, tool: impl Into<String>) -> Self {
        self.tool = Some(tool.into());
        self
    }
}

pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == text;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        return text.starts_with(parts[0]) && text.ends_with(parts[1]);
    }

    let mut idx = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 && !text.starts_with(part) {
            return false;
        }
        if i == parts.len() - 1 && !text.ends_with(part) {
            return false;
        }
        if let Some(pos) = text[idx..].find(part) {
            idx += pos + part.len();
        } else {
            return false;
        }
    }
    true
}

pub fn decide(rules: &[Rule], tool_name: &str, args_str: &str) -> Decision {
    let args_text = if args_str.trim_start().starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(args_str) {
            if let Some(cmd) = v.get("command").and_then(|v| v.as_str()) {
                cmd.to_string()
            } else if let Some(file) = v.get("file").and_then(|v| v.as_str()) {
                file.to_string()
            } else if let Some(cmd) = v.get("cmd").and_then(|v| v.as_str()) {
                cmd.to_string()
            } else {
                args_str.to_string()
            }
        } else {
            args_str.to_string()
        }
    } else {
        args_str.to_string()
    };

    let haystack = if args_text.starts_with(tool_name) {
        args_text.clone()
    } else {
        format!("{tool_name} {args_text}")
    };

    let mut last: Option<Decision> = None;
    for r in rules {
        if let Some(ref t) = r.tool {
            if !wildcard_match(t, tool_name) {
                continue;
            }
        }

        if wildcard_match(&r.pattern, tool_name)
            || wildcard_match(&r.pattern, &args_text)
            || wildcard_match(&r.pattern, &haystack)
            || r.pattern == tool_name
        {
            last = Some(r.decision);
        }
    }
    last.unwrap_or(Decision::Ask)
}

pub fn rules_from_json(obj: &serde_json::Value) -> Vec<Rule> {
    let mut out = Vec::new();
    if let Some(map) = obj.as_object() {
        for (k, v) in map {
            let decision: Decision = v.as_str().unwrap_or("ask").parse().unwrap_or(Decision::Ask);

            out.push(Rule::new(decision, k.clone()));
        }
    }
    out
}

#[derive(Default)]
pub struct PendingMap {
    inner: HashMap<String, oneshot::Sender<Decision>>,
}

impl PendingMap {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }
    pub fn insert(&mut self, id: String, tx: oneshot::Sender<Decision>) {
        self.inner.insert(id, tx);
    }
    pub fn resolve(&mut self, id: &str, decision: Decision) -> bool {
        if let Some(tx) = self.inner.remove(id) {
            let _ = tx.send(decision);
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct InteractiveAsk {
    pub id: String,
    pub tool: String,
    pub args: String,
    pub tx: oneshot::Sender<InteractiveDecision>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractiveDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

static INTERACTIVE_SENDER: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<InteractiveAsk>>> =
    std::sync::Mutex::new(None);

pub fn set_interactive_sender(tx: tokio::sync::mpsc::Sender<InteractiveAsk>) {
    if let Ok(mut g) = INTERACTIVE_SENDER.lock() {
        *g = Some(tx);
    }
}

pub fn get_interactive_sender() -> Option<tokio::sync::mpsc::Sender<InteractiveAsk>> {
    INTERACTIVE_SENDER.lock().ok().and_then(|g| g.clone())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionItem {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub question: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

#[derive(Debug)]
pub struct PendingQuestion {
    pub id: String,
    pub questions: Vec<QuestionItem>,
    pub tx: oneshot::Sender<QuestionResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QuestionResult {
    Answered(Vec<QuestionAnswer>),
    Cancelled,
}

pub const MAX_QUESTIONS: usize = 4;
pub const MAX_OPTIONS: usize = 8;

pub fn validate_questions(v: &serde_json::Value) -> Result<Vec<QuestionItem>, String> {
    let arr = v
        .get("questions")
        .and_then(|q| q.as_array())
        .ok_or_else(|| "missing required field 'questions' (array)".to_string())?;
    if arr.is_empty() {
        return Err("'questions' must not be empty".into());
    }
    if arr.len() > MAX_QUESTIONS {
        return Err(format!(
            "too many questions ({} > {MAX_QUESTIONS})",
            arr.len()
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    for (i, q) in arr.iter().enumerate() {
        let question = q
            .get("question")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if question.is_empty() {
            return Err(format!("questions[{i}]: missing 'question' text"));
        }
        if question.chars().count() > 500 {
            return Err(format!("questions[{i}]: question too long (max 500 chars)"));
        }
        let header = q
            .get("header")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if header.chars().count() > 80 {
            return Err(format!("questions[{i}]: header too long (max 80 chars)"));
        }
        let multi_select = q
            .get("multiSelect")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let opts = q
            .get("options")
            .and_then(|o| o.as_array())
            .ok_or_else(|| format!("questions[{i}]: missing 'options' array"))?;
        if opts.is_empty() {
            return Err(format!("questions[{i}]: 'options' must not be empty"));
        }
        if opts.len() > MAX_OPTIONS {
            return Err(format!(
                "questions[{i}]: too many options ({} > {MAX_OPTIONS})",
                opts.len()
            ));
        }
        let mut options = Vec::with_capacity(opts.len());
        for (j, o) in opts.iter().enumerate() {
            let label = o
                .get("label")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if label.is_empty() {
                return Err(format!("questions[{i}].options[{j}]: missing 'label'"));
            }
            if label.chars().count() > 200 {
                return Err(format!(
                    "questions[{i}].options[{j}]: label too long (max 200 chars)"
                ));
            }
            let description = o
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if description.chars().count() > 300 {
                return Err(format!(
                    "questions[{i}].options[{j}]: description too long (max 300 chars)"
                ));
            }
            options.push(QuestionOption { label, description });
        }
        out.push(QuestionItem {
            question,
            header,
            multi_select,
            options,
        });
    }
    Ok(out)
}

static QUESTION_SENDER: std::sync::Mutex<Option<tokio::sync::mpsc::Sender<PendingQuestion>>> =
    std::sync::Mutex::new(None);

pub fn set_question_sender(tx: tokio::sync::mpsc::Sender<PendingQuestion>) {
    if let Ok(mut g) = QUESTION_SENDER.lock() {
        *g = Some(tx);
    }
}

pub fn get_question_sender() -> Option<tokio::sync::mpsc::Sender<PendingQuestion>> {
    QUESTION_SENDER.lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
pub fn clear_question_sender() {
    if let Ok(mut g) = QUESTION_SENDER.lock() {
        *g = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn last_match_wins() {
        let rules = vec![
            Rule::new(Decision::Deny, "bash *"),
            Rule::new(Decision::Allow, "bash ls*"),
        ];

        assert_eq!(
            decide(&rules, "bash", r#"{"command":"ls -la"}"#),
            Decision::Allow
        );
        assert_eq!(decide(&rules, "bash", "bash ls -la"), Decision::Allow);
        assert_eq!(decide(&rules, "bash", "bash rm -rf /"), Decision::Deny);
        assert_eq!(
            decide(&rules, "bash", r#"{"command":"rm -rf /"}"#),
            Decision::Deny
        );
    }
    #[test]
    fn default_ask() {
        assert_eq!(decide(&[], "read", "read foo"), Decision::Ask);
    }
    #[test]
    fn wildcard_match_basic() {
        assert!(wildcard_match("*.cir", "test.cir"));
        assert!(!wildcard_match("*.cir", "test.png"));
        assert!(wildcard_match("bash *", "bash ls"));
    }
    #[test]
    fn validate_questions_ok_and_errors() {
        let v = serde_json::json!({"questions": [
            {"question": "Pick one", "header": "Choice", "options": [
                {"label": "A", "description": "first"},
                {"label": "B"},
            ]},
            {"question": "Pick many", "multiSelect": true, "options": [{"label": "X"}]},
        ]});
        let items = validate_questions(&v).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].options.len(), 2);
        assert!(items[1].multi_select);
        assert!(validate_questions(&serde_json::json!({})).is_err());
        assert!(validate_questions(&serde_json::json!({"questions": []})).is_err());
        assert!(validate_questions(
            &serde_json::json!({"questions": [{"question": "q", "options": []}]})
        )
        .is_err());
        assert!(validate_questions(
            &serde_json::json!({"questions": [{"question": "q", "options": [{"label": ""}]}]})
        )
        .is_err());
    }
}
