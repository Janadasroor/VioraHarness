use serde_json::Value;
use std::path::Path;

pub fn retrieve_flxsch_context(path: &str) -> Option<Value> {
    let p = Path::new(path);
    if !p.exists() {
        return None;
    }
    let content = std::fs::read_to_string(p).ok()?;
    serde_json::from_str::<Value>(&content).ok()
}
