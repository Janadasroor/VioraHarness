// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const LIST_URL: &str = "https://opencode.ai/zen/v1/models";
const TTL: Duration = Duration::from_secs(600);

struct Cache {
    at: Option<Instant>,
    models: Vec<String>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHED: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHED.get_or_init(|| {
        Mutex::new(Cache {
            at: None,
            models: Vec::new(),
        })
    })
}

pub fn parse_model_list(v: &serde_json::Value) -> Vec<String> {
    v.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()))
                .map(|id| id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

async fn fetch_live() -> Vec<String> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    match client.get(LIST_URL).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(v) => parse_model_list(&v),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

pub async fn gateway_models_cached() -> Vec<String> {
    if let Ok(g) = cache().lock() {
        if let Some(at) = g.at {
            if at.elapsed() < TTL && !g.models.is_empty() {
                return g.models.clone();
            }
        }
    }
    let models = fetch_live().await;
    if models.is_empty() {
        if let Ok(g) = cache().lock() {
            return g.models.clone();
        }
        return Vec::new();
    }
    if let Ok(mut g) = cache().lock() {
        g.at = Some(Instant::now());
        g.models = models.clone();
    }
    models
}

pub async fn pick_free_model() -> Option<String> {
    gateway_models_cached()
        .await
        .into_iter()
        .find(|id| id.to_lowercase().ends_with("-free"))
        .map(|id| format!("opencode/{id}"))
}

pub fn normalize_catalog_id(model_id: &str) -> String {
    let lower = model_id.to_lowercase();
    let mut s = lower.as_str();
    for pref in [
        "opencode-go/",
        "opencode_go/",
        "opencode/go/",
        "opencode:",
        "opencode/",
        "zen/",
        "go/",
    ] {
        if let Some(rest) = s.strip_prefix(pref) {
            s = rest;
            break;
        }
    }
    s.strip_suffix("-free").unwrap_or(s).to_string()
}

pub fn match_or_context(
    model_id: &str,
    or_sizes: &std::collections::HashMap<String, usize>,
) -> Option<usize> {
    if or_sizes.is_empty() {
        return None;
    }
    let cand = normalize_catalog_id(model_id);
    if cand.is_empty() {
        return None;
    }
    let mut best: Option<usize> = None;
    for (or_id, size) in or_sizes {
        let lower = or_id.to_lowercase();
        let family = lower.rsplit('/').next().unwrap_or(&lower);
        if lower == cand || family == cand {
            best = Some(best.map_or(*size, |b| b.max(*size)));
        }
    }
    best.filter(|s| *s > 0)
}

fn or_cache_path() -> std::path::PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        })
        .join("vioraharness/openrouter_ctx.json")
}

const OR_CTX_TTL: Duration = Duration::from_secs(6 * 3600);

fn read_or_cache() -> std::collections::HashMap<String, usize> {
    let path = or_cache_path();
    let mtime_ok = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().unwrap_or(Duration::MAX) < OR_CTX_TTL)
        .unwrap_or(false);
    if !mtime_ok {
        return std::collections::HashMap::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.as_object().map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n as usize)))
                    .collect()
            })
        })
        .unwrap_or_default()
}

pub async fn openrouter_context_cached() -> std::collections::HashMap<String, usize> {
    struct Mem {
        at: Option<Instant>,
        map: std::collections::HashMap<String, usize>,
    }
    static MEM: OnceLock<Mutex<Mem>> = OnceLock::new();
    let mem = || {
        MEM.get_or_init(|| {
            Mutex::new(Mem {
                at: None,
                map: Default::default(),
            })
        })
    };

    if let Ok(g) = mem().lock() {
        if !g.map.is_empty() {
            return g.map.clone();
        }
        if g.at.is_some_and(|at| at.elapsed() < OR_CTX_TTL) {
            return std::collections::HashMap::new();
        }
    }
    let mark_attempt = || {
        if let Ok(mut g) = mem().lock() {
            g.at = Some(Instant::now());
        }
    };
    let disk = read_or_cache();
    if !disk.is_empty() {
        if let Ok(mut g) = mem().lock() {
            g.at = Some(Instant::now());
            g.map = disk.clone();
        }
        return disk;
    }
    let key = match std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
    {
        Some(k) => k,
        None => return std::collections::HashMap::new(),
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return std::collections::HashMap::new(),
    };
    let map: std::collections::HashMap<String, usize> = match client
        .get("https://openrouter.ai/api/v1/models")
        .header("Authorization", format!("Bearer {key}"))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(v) => {
                v.get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| {
                                let id = m.get("id")?.as_str()?;
                                let n = m.get("context_length").and_then(|x| x.as_u64()).or_else(
                                    || {
                                        m.get("top_provider")
                                            .and_then(|t| t.get("context_length"))
                                            .and_then(|x| x.as_u64())
                                    },
                                )?;
                                (n > 0).then(|| (id.to_string(), n as usize))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Err(_) => std::collections::HashMap::new(),
        },
        _ => std::collections::HashMap::new(),
    };
    mark_attempt();
    if !map.is_empty() {
        if let Ok(mut g) = mem().lock() {
            g.map = map.clone();
        }
        if let Some(parent) = or_cache_path().parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let obj: serde_json::Map<String, serde_json::Value> = map
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::from(*v as u64)))
            .collect();
        let _ = std::fs::write(
            or_cache_path(),
            serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_default(),
        );
    }
    map
}

pub async fn context_limit_for_model(model_id: &str, fallback: usize) -> usize {
    let sizes = openrouter_context_cached().await;
    match_or_context(model_id, &sizes).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_id_matching() {
        use std::collections::HashMap;
        assert_eq!(
            normalize_catalog_id("opencode/muse-spark-1.3-contributor-free"),
            "muse-spark-1.3-contributor"
        );
        assert_eq!(normalize_catalog_id("opencode-go/kimi-test"), "kimi-test");
        assert_eq!(
            normalize_catalog_id("google/gemini-3-flash"),
            "google/gemini-3-flash"
        );
        let sizes: HashMap<String, usize> = [
            ("meta/muse-spark-1.3-contributor".into(), 1048576),
            ("nvidia/nemotron-3.5-lightning".into(), 262144),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            match_or_context("opencode/muse-spark-1.3-contributor-free", &sizes),
            Some(1048576)
        );
        assert_eq!(
            match_or_context("muse-spark-1.3-contributor-free", &sizes),
            Some(1048576)
        );
        assert_eq!(
            match_or_context("opencode/nemotron-3.5-lightning-free", &sizes),
            Some(262144)
        );

        assert_eq!(match_or_context("opencode/nope-zzz-free", &sizes), None);
        assert_eq!(
            match_or_context("opencode/muse-spark-1.3-contributor-free", &HashMap::new()),
            None
        );

        assert_eq!(match_or_context("opencode/qwen-test-free", &sizes), None);
    }

    #[test]
    fn parse_model_list_shape() {
        let v = json!({"object": "list", "data": [
            {"id": "unit-alpha", "object": "model"},
            {"id": "unit-beta-free", "object": "model"},
            {"nope": true},
        ]});
        assert_eq!(parse_model_list(&v), vec!["unit-alpha", "unit-beta-free"]);
        assert!(parse_model_list(&json!({})).is_empty());
        assert!(parse_model_list(&json!({"data": {}})).is_empty());
    }
}
