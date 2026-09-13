//! Agent modes: named tool allowlists + focused system prompt.
//!
//! A mode answers "what kind of work is this session for?" and trims the
//! harness to fit: the model only *sees* the mode's tools (restrictive
//! allowlist), gets mode-specific guidance, and auto-loads the mode's
//! skills. Modes never widen `vioraharness.json` permissions — the base
//! policy file stays the ceiling; a mode can only narrow.
//!
//! Conflict rules (modes share one harness, one task registry, one /tmp):
//! - The tool registry is built fresh per turn from the resolved mode —
//!   no shared mutable registry, no cross-mode bleed.
//! - Subagents inherit the intersection (parent mode tools ∩ kind tools).
//! - Background tasks are tagged with their origin mode and keep it.
//! - Sandbox extras and artifact paths are resolved from the active mode.

use crate::tools::ToolRegistry;

pub const DEFAULT_MODE: &str = "eda";
pub const MODE_ENV_VAR: &str = "VIORAHARNESS_MODE";

/// Web mode allowlist: core tools + web research + headless Chrome +
/// local dev servers. EDA sim/PCB tools are hidden (restrictive).
const WEB_TOOLS: &[&str] = &[
    "read",
    "write",
    "glob",
    "grep",
    "bash",
    "edit",
    "apply_patch",
    "todowrite",
    "question",
    "task",
    "skill",
    "webfetch",
    "websearch",
    "browser_screenshot",
    "browser_dom",
    "browser_pdf",
    "browser_open",
    "dev_serve",
];

#[derive(Debug, Clone, Copy)]
pub struct Mode {
    pub name: &'static str,
    pub description: &'static str,
    /// None = full registry (today's behavior). Some(list) = the model
    /// only sees these tools, and anything else fails closed at dispatch.
    pub tools: Option<&'static [&'static str]>,
    /// Skills force-loaded whenever the mode is active (in addition to
    /// intent-triggered skills, which are gated to this set).
    pub skills: &'static [&'static str],
    pub system_extra: &'static str,
}

pub fn builtin_modes() -> Vec<Mode> {
    vec![
        Mode {
            name: "eda",
            description: "VioraEDA hardware work (schematic/PCB/SPICE) + general coding. Full tool access.",
            tools: None,
            skills: &["sim", "pcb", "erc", "flux"],
            system_extra: "",
        },
        Mode {
            name: "web",
            description: "Web development: dev servers, headless Chrome screenshots/DOM, docs. No EDA sim/PCB tools.",
            tools: Some(WEB_TOOLS),
            skills: &["web"],
            system_extra: "Web loop: dev_serve the folder (port 0 picks one) -> browser_screenshot for vision -> browser_dom to assert text -> edit -> re-screenshot. After every web edit, re-screenshot; describe what you see, don't dump pixels. Kill dev servers via /tasks when done. Visible Chrome needs browser_open (Ask-gated); never xdotool windowclose.",
        },
    ]
}

pub fn normalize_mode_name(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn is_known_mode(name: &str) -> bool {
    let n = normalize_mode_name(name);
    builtin_modes().iter().any(|m| m.name == n)
}

pub fn find_mode(name: &str) -> Option<Mode> {
    let n = normalize_mode_name(name);
    builtin_modes().into_iter().find(|m| m.name == n)
}

/// `activeMode` project default from the first `vioraharness.json` found
/// (same candidate chain as permissions). Unknown values are ignored.
pub fn active_mode_from_config() -> Option<String> {
    for cand in crate::loop_mod::config_candidates() {
        if let Ok(content) = std::fs::read_to_string(&cand) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(m) = val.get("activeMode").and_then(|v| v.as_str()) {
                    let norm = normalize_mode_name(m);
                    if is_known_mode(&norm) {
                        return Some(norm);
                    }
                    tracing::warn!("ignoring unknown activeMode '{m}' in {cand}");
                }
            }
        }
    }
    None
}

/// Resolve the active mode. Precedence: explicit arg > `VIORAHARNESS_MODE`
/// env > `activeMode` config > `eda`. Unknown values fall back to `eda`
/// with a warning (callers that take user input should validate with
/// [`is_known_mode`] first for a hard error instead).
pub fn resolve_mode(explicit: Option<&str>) -> String {
    if let Some(e) = explicit {
        let norm = normalize_mode_name(e);
        if is_known_mode(&norm) {
            return norm;
        }
        tracing::warn!("unknown mode '{e}' — falling back to {}", DEFAULT_MODE);
        return DEFAULT_MODE.into();
    }
    if let Ok(env) = std::env::var(MODE_ENV_VAR) {
        if !env.trim().is_empty() {
            let norm = normalize_mode_name(&env);
            if is_known_mode(&norm) {
                return norm;
            }
            tracing::warn!("unknown {MODE_ENV_VAR}='{env}' — falling back");
        }
    }
    active_mode_from_config().unwrap_or_else(|| DEFAULT_MODE.into())
}

/// Like [`resolve_mode`], but also reports where the value came from (for
/// `doctor` and turn headers so precedence fights stay diagnosable).
pub fn resolve_mode_with_source(explicit: Option<&str>) -> (String, &'static str) {
    if let Some(e) = explicit {
        let norm = normalize_mode_name(e);
        if is_known_mode(&norm) {
            return (norm, "explicit");
        }
        return (DEFAULT_MODE.into(), "fallback-unknown-explicit");
    }
    if let Ok(env) = std::env::var(MODE_ENV_VAR) {
        if !env.trim().is_empty() {
            let norm = normalize_mode_name(&env);
            if is_known_mode(&norm) {
                return (norm, MODE_ENV_VAR);
            }
            return (DEFAULT_MODE.into(), "fallback-unknown-env");
        }
    }
    if let Some(cfg) = active_mode_from_config() {
        return (cfg, "activeMode-config");
    }
    (DEFAULT_MODE.into(), "default")
}

/// Build the tool registry for a mode. Unknown names behave like `eda`
/// (full registry) — validation happens at the UX layer.
pub fn registry_for_mode(name: &str) -> ToolRegistry {
    let full = ToolRegistry::new();
    match find_mode(name).and_then(|m| m.tools) {
        None => full,
        Some(allow) => {
            let mut r = ToolRegistry::new_empty();
            for t in full.all() {
                if allow.contains(&t.name.as_str()) {
                    r.add_tool(t.clone());
                }
            }
            r
        }
    }
}

/// Intersect a subagent kind's tool list with the parent mode's allowlist:
/// a subagent can never see tools its parent mode hides. `None` on both
/// sides means full access (today's coder/planner behavior under eda).
pub fn intersect_tools(
    kind_allow: Option<&[String]>,
    parent_allow: Option<&[String]>,
) -> Option<Vec<String>> {
    match (kind_allow, parent_allow) {
        (None, None) => None,
        (Some(k), None) => Some(k.to_vec()),
        (None, Some(p)) => Some(p.to_vec()),
        (Some(k), Some(p)) => Some(k.iter().filter(|t| p.contains(t)).cloned().collect()),
    }
}

/// Holds `VIORAHARNESS_MODE` for the duration of a turn so downstream
/// tools (background tasks, subagents, screenshot naming) tag/scope with
/// the turn's mode. Restored on drop, mirroring `ApprovalGuard`.
pub(crate) struct ModeGuard {
    prev: Option<String>,
}

impl ModeGuard {
    pub(crate) fn hold(mode: &str) -> Self {
        let prev = std::env::var(MODE_ENV_VAR).ok();
        std::env::set_var(MODE_ENV_VAR, mode);
        Self { prev }
    }

    /// Current turn's mode for code without loop context (background task
    /// tagging, artifact naming). Defaults to `eda` outside a turn.
    pub fn current() -> String {
        std::env::var(MODE_ENV_VAR)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| normalize_mode_name(&s))
            .filter(|s| is_known_mode(s))
            .unwrap_or_else(|| DEFAULT_MODE.into())
    }
}

impl Drop for ModeGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(MODE_ENV_VAR, v),
            None => std::env::remove_var(MODE_ENV_VAR),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn eda_is_full_registry_web_is_filtered() {
        let eda = registry_for_mode("eda");
        assert_eq!(eda.all().len(), ToolRegistry::new().all().len());
        let web = registry_for_mode("web");
        assert!(web.all().len() < eda.all().len());
        let names: Vec<&str> = web.all().iter().map(|t| t.name.as_str()).collect();
        for must in [
            "read",
            "bash",
            "browser_screenshot",
            "browser_dom",
            "browser_open",
            "dev_serve",
            "webfetch",
            "websearch",
            "task",
            "skill",
        ] {
            assert!(names.contains(&must), "web missing {must}");
        }
        for hidden in [
            "netlist_run",
            "schematic_render",
            "pcb_render",
            "pcb_compose",
            "erc",
            "flux",
            "viora",
        ] {
            assert!(!names.contains(&hidden), "web leaks {hidden}");
        }
    }

    #[test]
    fn unknown_mode_behaves_like_eda() {
        assert_eq!(
            registry_for_mode("nope").all().len(),
            ToolRegistry::new().all().len()
        );
        assert!(!is_known_mode("nope"));
        assert!(is_known_mode("WEB"));
    }

    #[test]
    fn resolve_precedence_explicit_env_config_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(MODE_ENV_VAR).ok();
        std::env::remove_var(MODE_ENV_VAR);
        assert_eq!(resolve_mode(Some("web")), "web");
        assert_eq!(resolve_mode(Some("  WEB ")), "web");
        assert_eq!(resolve_mode(Some("nope")), "eda");
        std::env::set_var(MODE_ENV_VAR, "web");
        assert_eq!(resolve_mode(None), "web");
        assert_eq!(resolve_mode(Some("eda")), "eda");
        std::env::set_var(MODE_ENV_VAR, "nope");
        let got = resolve_mode(None);
        assert!(
            got == "eda" || active_mode_from_config().as_deref() == Some(&got),
            "unknown env falls back: {got}"
        );
        match prev {
            Some(v) => std::env::set_var(MODE_ENV_VAR, v),
            None => std::env::remove_var(MODE_ENV_VAR),
        }
    }

    #[test]
    fn intersect_never_widens() {
        assert!(intersect_tools(None, None).is_none());
        let k: Vec<String> = vec!["read".into(), "bash".into()];
        let p: Vec<String> = vec!["read".into(), "glob".into()];
        assert_eq!(intersect_tools(Some(&k), None), Some(k.clone()));
        assert_eq!(intersect_tools(None, Some(&p)), Some(p.clone()));
        assert_eq!(
            intersect_tools(Some(&k), Some(&p)),
            Some(vec!["read".to_string()])
        );
        let empty: Vec<String> = vec![];
        assert_eq!(
            intersect_tools(Some(&k), Some(&empty)),
            Some(vec![]),
            "disjoint parent hides everything"
        );
    }

    #[test]
    fn mode_guard_sets_and_restores() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(MODE_ENV_VAR).ok();
        {
            let _hold = ModeGuard::hold("web");
            assert_eq!(ModeGuard::current(), "web");
        }
        match prev {
            Some(v) => {
                std::env::set_var(MODE_ENV_VAR, v.clone());
                assert_eq!(ModeGuard::current(), normalize_mode_name(&v));
            }
            None => {
                assert_eq!(ModeGuard::current(), "eda");
            }
        }
    }
}
