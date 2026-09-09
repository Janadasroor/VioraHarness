use serde_json::{json, Value};

pub const VALID_STATUSES: &[&str] = &["pending", "in_progress", "completed"];
pub const VALID_PRIORITIES: &[&str] = &["high", "medium", "low"];
pub const MAX_TODOS: usize = 100;

fn db_path() -> String {
    std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into())
}

fn validate_item(v: &Value) -> Result<(String, String, Option<String>), String> {
    let content = v
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("todo item has empty content".into());
    }
    if content.chars().count() > 500 {
        return Err(format!(
            "todo content too long ({} chars, max 500): {}…",
            content.chars().count(),
            content.chars().take(60).collect::<String>()
        ));
    }
    let status = v
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("pending")
        .to_lowercase();
    if !VALID_STATUSES.contains(&status.as_str()) {
        return Err(format!(
            "invalid status {status:?} (want one of {})",
            VALID_STATUSES.join("|")
        ));
    }
    let priority = v.get("priority").and_then(|p| p.as_str()).map(|p| {
        let p = p.to_lowercase();
        if VALID_PRIORITIES.contains(&p.as_str()) {
            p
        } else {
            String::new()
        }
    });
    let priority = priority.filter(|p| !p.is_empty());
    Ok((content, status, priority))
}

pub async fn run_todos(args: Value) -> Value {
    let session_id = args
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("default")
        .to_string();
    let arr = match args.get("todos").and_then(|t| t.as_array()) {
        Some(a) => a.clone(),
        None => {
            return json!({"ok": false, "error": "missing required field 'todos' (array of {content, status})"});
        }
    };
    if arr.len() > MAX_TODOS {
        return json!({"ok": false, "error": format!("too many todos ({} > {MAX_TODOS})", arr.len())});
    }
    let merge = args.get("merge").and_then(|m| m.as_bool()).unwrap_or(false);

    let store = match crate::session::SessionStore::new(&db_path()) {
        Ok(s) => s,
        Err(e) => return json!({"ok": false, "error": format!("todo store unavailable: {e}")}),
    };

    let mut items: Vec<(String, String, Option<String>)> = if merge {
        store.get_todos(&session_id).unwrap_or_default()
    } else {
        Vec::new()
    };
    for (i, item) in arr.iter().enumerate() {
        match validate_item(item) {
            Ok((content, status, priority)) => {
                if merge {
                    if let Some(slot) = items.iter_mut().find(|(c, _, _)| c == &content) {
                        slot.1 = status;
                        slot.2 = priority;
                        continue;
                    }
                }

                if !merge {
                    items.retain(|(c, _, _)| c != &content);
                }
                items.push((content, status, priority));
            }
            Err(e) => {
                return json!({"ok": false, "error": format!("todos[{i}]: {e}")});
            }
        }
    }
    if items.len() > MAX_TODOS {
        return json!({"ok": false, "error": format!("too many todos after merge ({} > {MAX_TODOS})", items.len())});
    }
    if let Err(e) = store.set_todos(&session_id, &items) {
        return json!({"ok": false, "error": format!("failed to persist todos: {e}")});
    }
    let done = items.iter().filter(|(_, s, _)| s == "completed").count();
    let active = items
        .iter()
        .find(|(_, s, _)| s == "in_progress")
        .map(|(c, _, _)| c.clone());
    json!({
        "ok": true,
        "session_id": session_id,
        "todos": items.iter().map(|(c, s, p)| {
            let mut o = serde_json::Map::new();
            o.insert("content".into(), Value::String(c.clone()));
            o.insert("status".into(), Value::String(s.clone()));
            if let Some(p) = p { o.insert("priority".into(), Value::String(p.clone())); }
            Value::Object(o)
        }).collect::<Vec<_>>(),
        "done": done,
        "total": items.len(),
        "active": active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_statuses() {
        assert!(validate_item(&json!({"content": "x", "status": "pending"})).is_ok());
        assert!(validate_item(&json!({"content": "x", "status": "IN_PROGRESS"})).is_ok());
        assert!(validate_item(&json!({"content": "x"})).is_ok());
        assert!(validate_item(&json!({"content": "x", "status": "done"})).is_err());
        assert!(validate_item(&json!({"content": "  "})).is_err());
        assert!(validate_item(
            &json!({"content": "x", "status": "completed", "priority": "bogus"})
        )
        .unwrap()
        .2
        .is_none());
    }

    #[tokio::test]
    async fn roundtrip_replace_and_merge() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("vh-todo-test-{}", std::process::id()));
        // Remove first: a killed run + pid reuse would otherwise inherit rows.
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let db = dir.join("t.db");
        std::env::set_var("VIORAHARNESS_DB", db.to_string_lossy().to_string());
        let sid = "sess-test-todos";

        let r = run_todos(json!({
            "session_id": sid,
            "todos": [
                {"content": "a", "status": "completed"},
                {"content": "b", "status": "in_progress", "priority": "high"},
            ]
        }))
        .await;
        assert_eq!(r["ok"], true);
        assert_eq!(r["done"], 1);
        assert_eq!(r["total"], 2);
        assert_eq!(r["active"], Value::String("b".into()));

        let r = run_todos(json!({
            "session_id": sid,
            "merge": true,
            "todos": [
                {"content": "b", "status": "completed"},
                {"content": "c", "status": "pending"},
            ]
        }))
        .await;
        assert_eq!(r["done"], 2);
        assert_eq!(r["total"], 3);

        let r = run_todos(json!({"session_id": sid, "todos": [{"content": "z"}]})).await;
        assert_eq!(r["total"], 1);

        let r = run_todos(json!({"session_id": sid})).await;
        assert_eq!(r["ok"], false);

        std::env::remove_var("VIORAHARNESS_DB");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
