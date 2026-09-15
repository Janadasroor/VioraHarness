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
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explore => "explore",
            Self::Planner => "planner",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
        }
    }

    /// Strict parse: unknown kinds are an error (never silently fall back
    /// to explore — a typo'd `kind` must fail closed so the caller notices).
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "explore" => Ok(Self::Explore),
            "planner" => Ok(Self::Planner),
            "coder" => Ok(Self::Coder),
            "reviewer" => Ok(Self::Reviewer),
            other => anyhow::bail!(
                "unknown subagent kind '{other}' (expected explore|planner|coder|reviewer)"
            ),
        }
    }

    pub async fn default_model_dynamic(&self) -> anyhow::Result<String> {
        self.default_model_dynamic_with_parent(None).await
    }

    /// Model chain: explicit override > `VIORAHARNESS_SUBAGENT_MODEL` >
    /// parent turn's model (offline-safe reuse) > `VIORAHARNESS_MODEL` >
    /// gateway free catalog. The catalog is last because it needs network;
    /// the error names every knob when all are missing.
    pub async fn default_model_dynamic_with_parent(
        &self,
        parent_model: Option<&str>,
    ) -> anyhow::Result<String> {
        if let Ok(m) = std::env::var("VIORAHARNESS_SUBAGENT_MODEL") {
            if !m.trim().is_empty() {
                return Ok(m);
            }
        }
        if let Some(p) = parent_model {
            if !p.trim().is_empty() {
                return Ok(p.to_string());
            }
        }
        if let Ok(m) = std::env::var("VIORAHARNESS_MODEL") {
            if !m.trim().is_empty() {
                return Ok(m);
            }
        }
        match crate::provider::catalog::pick_free_model().await {
            Some(m) => Ok(m),
            None => anyhow::bail!(
                "no model for {:?} subagent: gateway catalog unreachable (offline?) — set VIORAHARNESS_SUBAGENT_MODEL, VIORAHARNESS_MODEL, or pass model: to the task tool",
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
                "skill".into(),
            ]),
            // Planner drafts, never mutates: read-only + planning aids.
            Self::Planner => Some(vec![
                "read".into(),
                "glob".into(),
                "grep".into(),
                "schematic_query".into(),
                "symbol_search".into(),
                "footprint_list".into(),
                "netlist_validate".into(),
                "skill".into(),
                "todowrite".into(),
            ]),
            // Coder is full access minus nesting/interaction: `None` here
            // means "start from the full registry", and `execute` strips
            // `task` (no nested spawns — depth guard is the backstop) and
            // `question` (subagents can't ask the user).
            Self::Coder => None,
            Self::Reviewer => Some(vec![
                "read".into(),
                "grep".into(),
                "glob".into(),
                "skill".into(),
            ]),
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

/// Parent mode's tool allowlist from the current turn (`None` = full
/// access). Read at spawn time so `task` calls inside a mode stay inside it.
fn parent_mode_allow() -> Option<Vec<String>> {
    let mode = crate::mode::ModeGuard::current();
    crate::mode::find_mode(&mode)
        .and_then(|m| m.tools)
        .map(|ts| ts.iter().map(|t| t.to_string()).collect())
}

/// Global cap shared by every spawn path (blocking + detached). The old
/// code built a fresh `SubagentPool::new(4)` per blocking call, so N
/// concurrent blocking tasks meant 4N permits — effectively unbounded.
/// One static semaphore makes the limit real.
static GLOBAL_SEM: std::sync::LazyLock<Semaphore> = std::sync::LazyLock::new(|| Semaphore::new(4));

/// Parent context captured at the `task` call site (the loop injects
/// `parent_depth`/`parent_model` into the tool args; direct callers use
/// depth 0 / no model). Depth drives both the tracker label and the
/// loop's `task recursion depth exceeded` guard.
#[derive(Debug, Clone, Default)]
pub struct ParentCtx {
    pub depth: usize,
    pub model: Option<String>,
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
        self.spawn_one_scoped(kind, prompt, model_override, parent_mode_allow())
            .await
    }

    /// Spawn with an explicit parent-mode allowlist (`None` = full access).
    /// The effective list is always kind ∩ parent: a subagent can never
    /// see tools its parent mode hides (no escape hatch across modes).
    pub async fn spawn_one_scoped(
        &self,
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
        parent_allow: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        self.spawn_one_full(
            kind,
            prompt,
            model_override,
            parent_allow,
            ParentCtx::default(),
        )
        .await
    }

    /// Full spawn with parent depth/model (the `task` tool path). Blocking
    /// but cancellable: the work runs on a spawned task with its abort
    /// handle registered, so `cancel_run`/`x` in /agents aborts it just
    /// like a detached run. Times out via `tracker::subagent_timeout()`.
    pub async fn spawn_one_full(
        &self,
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
        parent_allow: Option<Vec<String>>,
        parent: ParentCtx,
    ) -> anyhow::Result<String> {
        let _global = GLOBAL_SEM.acquire().await?;
        let _local = self.sem.acquire().await?;
        let mode_label = crate::mode::ModeGuard::current();
        let child_depth = parent.depth.saturating_add(1);
        let (run_id, _) = crate::subagent::tracker::track_start_at_depth(
            kind.as_str(),
            &prompt,
            &mode_label,
            child_depth,
        );
        let timeout = crate::subagent::tracker::subagent_timeout();
        let fut = Self::execute(
            kind,
            prompt,
            model_override,
            parent_allow,
            mode_label,
            child_depth,
            parent.model,
        );
        let handle = tokio::spawn(async move {
            match tokio::time::timeout(timeout, fut).await {
                Ok(r) => r,
                Err(_) => Err(anyhow::anyhow!(
                    "subagent timed out after {}s (VIORAHARNESS_SUBAGENT_TIMEOUT_SECS)",
                    timeout.as_secs()
                )),
            }
        });
        crate::subagent::tracker::register_handle(&run_id, handle.abort_handle());
        let res = match handle.await {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => Err(anyhow::anyhow!("cancelled by user")),
            Err(e) => Err(anyhow::anyhow!("join: {e}")),
        };
        match &res {
            Ok(text) => crate::subagent::tracker::track_finish(&run_id, true, text),
            Err(e) => {
                // Cancel-then-finish resolves to killed inside track_finish.
                crate::subagent::tracker::track_finish(&run_id, false, &format!("{e:#}"))
            }
        }
        res
    }

    /// Detached spawn for `task background:true`: registers the run and
    /// drives it on the shared background pool, returning the run id
    /// immediately. The caller's turn continues; completion lands in
    /// `tracker::take_completions()` for the UI to surface (notice +
    /// follow-up turn, like background bash tasks). Cancel via
    /// `tracker::cancel_run` (`x` in /agents). Times out via
    /// `tracker::subagent_timeout()`.
    pub fn spawn_background(
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
    ) -> String {
        Self::spawn_background_full(kind, prompt, model_override, ParentCtx::default())
    }

    /// Detached spawn with parent depth/model (the `task` tool path).
    pub fn spawn_background_full(
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
        parent: ParentCtx,
    ) -> String {
        // Capture the parent turn's mode here: the background task may
        // run on another thread where the mode guard is not held.
        let parent_allow = parent_mode_allow();
        let parent_mode = crate::mode::ModeGuard::current();
        let child_depth = parent.depth.saturating_add(1);
        let (run_id, _) = crate::subagent::tracker::track_start_at_depth(
            kind.as_str(),
            &prompt,
            &parent_mode,
            child_depth,
        );
        let run_id_inner = run_id.clone();
        let timeout = crate::subagent::tracker::subagent_timeout();
        let handle = tokio::spawn(async move {
            let Ok(_permit) = GLOBAL_SEM.acquire().await else {
                crate::subagent::tracker::track_finish(
                    &run_id_inner,
                    false,
                    "background pool shut down",
                );
                return;
            };
            let fut = Self::execute(
                kind,
                prompt,
                model_override,
                parent_allow,
                parent_mode,
                child_depth,
                parent.model,
            );
            let res = match tokio::time::timeout(timeout, fut).await {
                Ok(r) => r,
                Err(_) => Err(anyhow::anyhow!(
                    "subagent timed out after {}s (VIORAHARNESS_SUBAGENT_TIMEOUT_SECS)",
                    timeout.as_secs()
                )),
            };
            match &res {
                Ok(text) => crate::subagent::tracker::track_finish(&run_id_inner, true, text),
                Err(e) => {
                    crate::subagent::tracker::track_finish(&run_id_inner, false, &format!("{e:#}"))
                }
            }
        });
        crate::subagent::tracker::register_handle(&run_id, handle.abort_handle());
        run_id
    }

    /// Untracked runner shared by all spawn paths: model → filtered
    /// registry → loop run at `child_depth`. Callers own tracking
    /// (start/finish) and timeout.
    #[allow(clippy::too_many_arguments)]
    async fn execute(
        kind: SubagentKind,
        prompt: String,
        model_override: Option<String>,
        parent_allow: Option<Vec<String>>,
        mode_label: String,
        child_depth: usize,
        parent_model: Option<String>,
    ) -> anyhow::Result<String> {
        let model = match model_override {
            Some(m) => m,
            None => {
                kind.default_model_dynamic_with_parent(parent_model.as_deref())
                    .await?
            }
        };
        let mut effective =
            crate::mode::intersect_tools(kind.allowed_tools().as_deref(), parent_allow.as_deref());
        // Coder starts full (None ∩ parent) — strip nesting + interaction.
        if matches!(kind, SubagentKind::Coder) {
            effective = match effective {
                None => {
                    let all: Vec<String> = ToolRegistry::new()
                        .all()
                        .iter()
                        .map(|t| t.name.clone())
                        .filter(|n| n != "task" && n != "question")
                        .collect();
                    Some(all)
                }
                Some(allow) => Some(
                    allow
                        .into_iter()
                        .filter(|n| n != "task" && n != "question")
                        .collect(),
                ),
            };
        }
        let registry = match effective {
            None => ToolRegistry::new(),
            Some(allow) => {
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
            }
        };

        let mut loop_ = AgentLoop::with_registry(registry);
        // Label the subagent loop with the parent mode so its turns tag
        // tasks/artifacts consistently (registry already intersected).
        loop_.mode = mode_label;

        let full_prompt = format!("{}\n\nTask: {}", kind.system_extra(), prompt);
        loop_
            .run_with_depth(&full_prompt, &model, None, child_depth)
            .await
    }

    pub async fn spawn_many(
        &self,
        jobs: Vec<(SubagentKind, String)>,
    ) -> Vec<anyhow::Result<String>> {
        // Capture the parent mode once: concurrent turns may hold
        // different mode guards, so jobs must not re-read it later.
        // Siblings share one depth (parent+1), not start-order inflation.
        let parent_allow = parent_mode_allow();
        let parent_mode = crate::mode::ModeGuard::current();
        let timeout = crate::subagent::tracker::subagent_timeout();
        let mut handles = Vec::new();
        for (kind, prompt) in jobs {
            let sem = self.sem.clone();
            let parent_allow = parent_allow.clone();
            let parent_mode = parent_mode.clone();
            handles.push(tokio::spawn(async move {
                let _g = GLOBAL_SEM.acquire().await.unwrap();
                let _permit = sem.acquire_owned().await.unwrap();
                let (run_id, _) = crate::subagent::tracker::track_start_at_depth(
                    kind.as_str(),
                    &prompt,
                    &parent_mode,
                    1,
                );
                let fut = Self::execute(kind, prompt, None, parent_allow, parent_mode, 1, None);
                let res = match tokio::time::timeout(timeout, fut).await {
                    Ok(r) => r,
                    Err(_) => Err(anyhow::anyhow!(
                        "subagent timed out after {}s (VIORAHARNESS_SUBAGENT_TIMEOUT_SECS)",
                        timeout.as_secs()
                    )),
                };
                match &res {
                    Ok(text) => crate::subagent::tracker::track_finish(&run_id, true, text),
                    Err(e) => {
                        crate::subagent::tracker::track_finish(&run_id, false, &format!("{e:#}"))
                    }
                }
                res
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
        // Planner drafts, never mutates: no write/edit/bash/task.
        let planner = SubagentKind::Planner.allowed_tools().unwrap();
        for t in ["write", "edit", "apply_patch", "bash", "task"] {
            assert!(
                !planner.contains(&t.to_string()),
                "planner must not have {t}"
            );
        }
        assert!(planner.contains(&"read".to_string()));
        // Coder is full-minus-task/question (None = full at this layer;
        // execute() strips the two). Checked in coder_strips_task_question.
        assert!(SubagentKind::Coder.allowed_tools().is_none());
        let reviewer = SubagentKind::Reviewer.allowed_tools().unwrap();
        assert!(reviewer.contains(&"read".to_string()));
        assert!(!reviewer.contains(&"bash".to_string()));
    }

    #[test]
    fn kind_parse_is_strict() {
        assert!(matches!(
            SubagentKind::parse("explore").unwrap(),
            SubagentKind::Explore
        ));
        assert!(matches!(
            SubagentKind::parse("coder").unwrap(),
            SubagentKind::Coder
        ));
        assert!(SubagentKind::parse("explor").is_err(), "typo must fail");
        assert!(SubagentKind::parse("").is_err());
        assert!(SubagentKind::parse("EXPLORE").is_err(), "case-sensitive");
    }

    #[test]
    fn coder_strips_task_question() {
        // Mirrors the filter in execute(): full registry minus nesting and
        // user-interaction tools.
        let all: Vec<String> = ToolRegistry::new()
            .all()
            .iter()
            .map(|t| t.name.clone())
            .filter(|n| n != "task" && n != "question")
            .collect();
        assert!(!all.contains(&"task".to_string()));
        assert!(!all.contains(&"question".to_string()));
        assert!(all.contains(&"read".to_string()));
        assert!(all.contains(&"bash".to_string()));
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
    #[allow(clippy::await_holding_lock)]
    async fn default_model_env_override_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_SUBAGENT_MODEL").ok();
        let prev_parent = std::env::var("VIORAHARNESS_MODEL").ok();
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
        // Parent model is the offline fallback when no override is set.
        std::env::remove_var("VIORAHARNESS_SUBAGENT_MODEL");
        std::env::remove_var("VIORAHARNESS_MODEL");
        assert_eq!(
            SubagentKind::Explore
                .default_model_dynamic_with_parent(Some("parent/model-y"))
                .await
                .unwrap(),
            "parent/model-y"
        );
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_SUBAGENT_MODEL", v),
            None => std::env::remove_var("VIORAHARNESS_SUBAGENT_MODEL"),
        }
        match prev_parent {
            Some(v) => std::env::set_var("VIORAHARNESS_MODEL", v),
            None => std::env::remove_var("VIORAHARNESS_MODEL"),
        }
    }
}
