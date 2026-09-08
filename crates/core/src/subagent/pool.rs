use crate::loop_mod::AgentLoop;
use crate::tools::ToolRegistry;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[derive(Debug, Clone, Copy)]
pub enum SubagentKind {
    Explore,
    Planner,
    Coder,
    Reviewer,
}

impl SubagentKind {
    pub async fn default_model_dynamic(&self) -> anyhow::Result<String> {
        if let Ok(m) = std::env::var("VIORAHARNESS_SUBAGENT_MODEL") {
            if !m.trim().is_empty() {
                return Ok(m);
            }
        }
        match crate::provider::catalog::pick_free_model().await {
            Some(m) => Ok(m),
            None => anyhow::bail!(
                "no model for {:?} subagent: gateway catalog unreachable (offline?) — set VIORAHARNESS_SUBAGENT_MODEL or pass model: to the task tool",
                self
            ),
        }
    }

    pub fn allowed_tools(&self) -> Option<Vec<String>> {
        match self {
            Self::Explore => Some(vec![
                "read".into(),
                "glob".into(),
                "grep".into(),
                "schematic_query".into(),
                "symbol_search".into(),
                "footprint_list".into(),
                "netlist_validate".into(),
            ]),
            Self::Planner => None,
            Self::Coder => None,
            Self::Reviewer => Some(vec!["read".into(), "grep".into(), "glob".into()]),
        }
    }

    pub fn system_extra(&self) -> &'static str {
        match self {
            Self::Explore => "You are an explore subagent. Read-only. Never edit/write/bash rm. Return a concise structured summary.",
            Self::Planner => "You are a planner subagent. Draft a plan, do not mutate files. Output a markdown plan.",
            Self::Coder => "You are a coder subagent. Full tools allowed. Implement precisely.",
            Self::Reviewer => "You are a reviewer subagent. Check logic/security/quality. Return findings as JSON.",
        }
    }
}

pub struct SubagentPool {
    sem: Arc<Semaphore>,
}

impl SubagentPool {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            sem: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    pub async fn spawn_one(
        &self,
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
    ) -> anyhow::Result<String> {
        let _permit = self.sem.acquire().await?;
        let model = match model_override {
            Some(m) => m,
            None => kind.default_model_dynamic().await?,
        };
        let registry = if let Some(allow) = kind.allowed_tools() {
            let full = ToolRegistry::new();
            let filtered: Vec<_> = full
                .all()
                .iter()
                .filter(|t| allow.contains(&t.name))
                .cloned()
                .collect();
            let mut r = ToolRegistry::new_empty();
            for t in filtered {
                r.add_tool(t);
            }
            r
        } else {
            ToolRegistry::new()
        };

        let loop_ = AgentLoop::with_registry(registry);

        let full_prompt = format!("{}\n\nTask: {}", kind.system_extra(), prompt);
        loop_.run(&full_prompt, &model, None).await
    }

    pub async fn spawn_many(
        &self,
        jobs: Vec<(SubagentKind, String)>,
    ) -> Vec<anyhow::Result<String>> {
        let mut handles = Vec::new();
        for (kind, prompt) in jobs {
            let sem = self.sem.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                let model = match kind.default_model_dynamic().await {
                    Ok(m) => m,
                    Err(e) => return Err(e),
                };
                let registry = if let Some(allow) = kind.allowed_tools() {
                    let full = ToolRegistry::new();
                    let filtered: Vec<_> = full
                        .all()
                        .iter()
                        .filter(|t| allow.contains(&t.name))
                        .cloned()
                        .collect();
                    let mut r = ToolRegistry::new_empty();
                    for t in filtered {
                        r.add_tool(t);
                    }
                    r
                } else {
                    ToolRegistry::new()
                };
                let loop_ = AgentLoop::with_registry(registry);
                let full_prompt = format!("{}\n\nTask: {}", kind.system_extra(), prompt);
                loop_.run(&full_prompt, &model, None).await
            }));
        }
        let mut out = Vec::new();
        for h in handles {
            out.push(
                h.await
                    .unwrap_or_else(|e| Err(anyhow::anyhow!("join: {e}"))),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn tool_allowance_per_kind() {
        let explore = SubagentKind::Explore.allowed_tools().unwrap();
        for t in ["read", "glob", "grep"] {
            assert!(explore.contains(&t.to_string()), "{t}");
        }
        for t in ["write", "edit", "apply_patch", "bash", "task"] {
            assert!(
                !explore.contains(&t.to_string()),
                "explore must not have {t}"
            );
        }
        assert!(SubagentKind::Planner.allowed_tools().is_none());
        assert!(SubagentKind::Coder.allowed_tools().is_none());
        let reviewer = SubagentKind::Reviewer.allowed_tools().unwrap();
        assert!(reviewer.contains(&"read".to_string()));
        assert!(!reviewer.contains(&"bash".to_string()));
    }

    #[test]
    fn system_prompts_distinct_and_nonempty() {
        let texts = [
            SubagentKind::Explore.system_extra(),
            SubagentKind::Planner.system_extra(),
            SubagentKind::Coder.system_extra(),
            SubagentKind::Reviewer.system_extra(),
        ];
        for t in texts {
            assert!(!t.trim().is_empty());
        }
        let mut uniq = texts.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 4);
        assert!(SubagentKind::Explore.system_extra().contains("Read-only"));
    }

    #[tokio::test]
    async fn default_model_env_override_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_SUBAGENT_MODEL").ok();
        std::env::set_var("VIORAHARNESS_SUBAGENT_MODEL", "test/model-x");
        assert_eq!(
            SubagentKind::Explore.default_model_dynamic().await.unwrap(),
            "test/model-x"
        );
        assert_eq!(
            SubagentKind::Coder.default_model_dynamic().await.unwrap(),
            "test/model-x",
            "override applies to all kinds"
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_MODEL", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_MODEL"),
        }
    }
}
