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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_invalid_files_are_none() {
        assert!(retrieve_flxsch_context("/nonexistent-vh-xyz/x.flxsch").is_none());
        let p = std::env::temp_dir().join("vh_ret_bad.flxsch");
        std::fs::write(&p, "not json{{").unwrap();
        assert!(retrieve_flxsch_context(p.to_str().unwrap()).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn valid_json_file_parses() {
        let p = std::env::temp_dir().join("vh_ret_ok.flxsch");
        std::fs::write(&p, r#"{"components": ["R1"], "nets": 3}"#).unwrap();
        let v = retrieve_flxsch_context(p.to_str().unwrap()).unwrap();
        assert_eq!(v["nets"], serde_json::json!(3));
        let _ = std::fs::remove_file(&p);
    }
}
