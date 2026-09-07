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
