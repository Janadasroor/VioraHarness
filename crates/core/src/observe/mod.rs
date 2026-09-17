// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub struct CostTracker {
    pub input_tokens: AtomicUsize,
    pub output_tokens: AtomicUsize,
    pub tool_calls: AtomicUsize,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            input_tokens: AtomicUsize::new(0),
            output_tokens: AtomicUsize::new(0),
            tool_calls: AtomicUsize::new(0),
        }
    }
    pub fn record(&self, input: usize, output: usize) {
        self.input_tokens.fetch_add(input, Ordering::Relaxed);
        self.output_tokens.fetch_add(output, Ordering::Relaxed);
    }
    pub fn inc_tool_calls(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }
    pub fn summary(&self) -> String {
        format!(
            "in={} out={} tools={}",
            self.input_tokens.load(Ordering::Relaxed),
            self.output_tokens.load(Ordering::Relaxed),
            self.tool_calls.load(Ordering::Relaxed)
        )
    }
}

pub struct TraceManager {
    pub dir: String,
}

impl TraceManager {
    pub fn new(dir: impl Into<String>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn log(&self, session_id: &str, event: &str, payload: &serde_json::Value) {
        let path = format!("{}/{}.jsonl", self.dir, session_id);
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let line =
            serde_json::json!({"ts": chrono_ts(), "event": event, "payload": payload}).to_string();
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn chrono_ts() -> String {
    std::process::Command::new("date")
        .args(["-Iseconds"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

#[derive(Debug, Clone)]
pub struct CoreError {
    pub id: String,
    pub source: String,
    pub summary: String,
    pub full: String,
    pub at: i64,
}

static ERROR_LOG: std::sync::LazyLock<std::sync::Mutex<VecDeque<CoreError>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(VecDeque::new()));

pub const MAX_ERRORS: usize = 50;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn push_error(source: &str, full: &str) -> String {
    static CTR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let c = CTR.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let full_hex = format!("{nanos:x}{c:04x}");
    let id = format!("err_{}", &full_hex[full_hex.len().saturating_sub(12)..]);
    let first = full.lines().next().unwrap_or("").trim();
    let summary = if first.chars().count() > 120 {
        format!("{}…", first.chars().take(120).collect::<String>())
    } else if first.is_empty() {
        "(empty error)".to_string()
    } else {
        first.to_string()
    };
    let mut log = ERROR_LOG.lock().unwrap_or_else(|e| e.into_inner());
    log.push_front(CoreError {
        id: id.clone(),
        source: source.to_string(),
        summary,
        full: full.to_string(),
        at: now_secs(),
    });
    while log.len() > MAX_ERRORS {
        log.pop_back();
    }
    id
}

pub fn list_errors() -> Vec<CoreError> {
    ERROR_LOG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

pub fn clear_errors() {
    ERROR_LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

pub fn error_count() -> usize {
    ERROR_LOG.lock().unwrap_or_else(|e| e.into_inner()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_log_caps_and_orders() {
        clear_errors();
        let a = push_error("turn", "first boom");
        assert!(a.starts_with("err_"));
        push_error("slash", "second\nboom\nwith\nlines");
        let list = list_errors();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].summary, "second", "newest first, first line only");
        assert_eq!(list[0].source, "slash");
        assert_eq!(list[1].id, a);
        assert_eq!(error_count(), 2);
        for i in 0..60 {
            push_error("flood", &format!("e{i}"));
        }
        assert_eq!(error_count(), MAX_ERRORS, "bounded");
        assert!(
            list_errors().iter().all(|e| e.id != a),
            "oldest evicted first"
        );
        clear_errors();
        assert_eq!(error_count(), 0);
        assert!(push_error("x", "").contains("err_"), "empty full ok");
        clear_errors();
    }
}
