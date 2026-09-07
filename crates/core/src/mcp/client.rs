use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct JobHandle {
    pub id: String,
    pub status: String,
}

#[derive(Default)]
pub struct McpBridge {
    jobs: Arc<Mutex<HashMap<String, Value>>>,
    next_id: Arc<Mutex<u64>>,
}

impl McpBridge {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn netlist_run_async(&self, file: &str, analysis: Option<&str>) -> String {
        let id = {
            let mut n = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            let id = format!("sim_{}", *n);
            *n += 1;
            id
        };
        {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.insert(
                id.clone(),
                serde_json::json!({"status":"queued","file":file}),
            );
        }
        let file = file.to_string();
        let analysis = analysis.map(|s| s.to_string());
        let jobs = self.jobs.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            {
                let mut j = jobs.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(v) = j.get_mut(&id_clone) {
                    *v = serde_json::json!({"status":"running","file":file});
                }
            }
            let mut args = vec![
                "netlist-run".to_string(),
                file.clone(),
                "--json".to_string(),
            ];
            if let Some(a) = analysis {
                args.extend(["--analysis".into(), a]);
            }
            let out = crate::tools::viora::run_viora_command(&args, Some(120)).await;
            let result = serde_json::json!({"ok": out.ok, "stdout": out.stdout, "data": out.data});
            let mut j = jobs.lock().unwrap_or_else(|e| e.into_inner());
            j.insert(
                id_clone.clone(),
                serde_json::json!({"status":"done","result": result}),
            );
        });
        id
    }

    pub fn job_status(&self, id: &str) -> Option<Value> {
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub fn describe_tools() -> Vec<crate::tools::ToolDef> {
        vec![]
    }
}
