// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

//! Reasoning-depth dial ("thinking level"): one setting, every provider.
//!
//! Levels form a fixed spectrum from cheapest to deepest:
//! `off < minimal < low < medium < high < xhigh < max`.
//! The dial resolves once per turn (env > TUI state file > project
//! config > `medium`) and each provider body builder maps it onto the
//! vendor's own knob: a unified `reasoning.effort` value for
//! effort-style APIs, a token budget for budget-style ones.

/// Env override for headless runs (wins over files).
pub const THINKING_ENV_VAR: &str = "VIORAHARNESS_THINKING_LEVEL";
/// Level used when nothing is configured anywhere.
pub const DEFAULT_THINKING_LEVEL: &str = "medium";
/// Full spectrum, cheapest first. Cycled in this order.
pub const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Lowercase + trim + accept `none` as `off`. Unknown input falls back
/// to the default (callers validating user input should check
/// [`is_known_thinking_level`] first for a hard error instead).
pub fn normalize_thinking_level(raw: &str) -> String {
    let n = raw.trim().to_lowercase();
    let n = if n == "none" { "off".to_string() } else { n };
    if THINKING_LEVELS.contains(&n.as_str()) {
        n
    } else {
        DEFAULT_THINKING_LEVEL.into()
    }
}

pub fn is_known_thinking_level(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "off" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    )
}

/// Step through the spectrum, wrapping at the ends.
pub fn cycle_thinking_level(current: &str, dir: i32) -> String {
    let cur = THINKING_LEVELS
        .iter()
        .position(|l| *l == normalize_thinking_level(current).as_str())
        .unwrap_or(3) as i32;
    let n = THINKING_LEVELS.len() as i32;
    THINKING_LEVELS[((cur + dir).rem_euclid(n)) as usize].to_string()
}

fn tui_state_thinking_level() -> Option<String> {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        });
    let txt = std::fs::read_to_string(base.join("vioraharness/tui_state.json")).ok()?;
    let level = serde_json::from_str::<serde_json::Value>(&txt)
        .ok()?
        .get("thinking_level")?
        .as_str()?
        .to_string();
    let norm = normalize_thinking_level(&level);
    if THINKING_LEVELS.contains(&norm.as_str()) {
        Some(norm)
    } else {
        None
    }
}

fn config_thinking_level() -> Option<String> {
    for cand in crate::loop_mod::config_candidates() {
        if let Ok(content) = std::fs::read_to_string(&cand) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(l) = val
                    .get("tui")
                    .and_then(|t| t.get("thinking_level"))
                    .and_then(|v| v.as_str())
                {
                    let norm = normalize_thinking_level(l);
                    if THINKING_LEVELS.contains(&norm.as_str()) {
                        return Some(norm);
                    }
                    tracing::warn!("ignoring unknown tui.thinking_level '{l}' in {cand}");
                }
            }
        }
    }
    None
}

/// Resolve the active level. Precedence: `VIORAHARNESS_THINKING_LEVEL`
/// env > TUI state file > project config > `medium`.
pub fn resolve_thinking_level() -> String {
    if let Ok(env) = std::env::var(THINKING_ENV_VAR) {
        if !env.trim().is_empty() {
            return normalize_thinking_level(&env);
        }
    }
    if let Some(l) = tui_state_thinking_level() {
        return l;
    }
    config_thinking_level().unwrap_or_else(|| DEFAULT_THINKING_LEVEL.into())
}

/// Share of the output budget spent on reasoning, per the unified
/// effort vocabulary (`max`/`xhigh` 95%, `high` 80%, `medium` 50%,
/// `low` 20%, `minimal` 10%). `off` yields 0.0.
pub fn effort_ratio(level: &str) -> f64 {
    match normalize_thinking_level(level).as_str() {
        "off" => 0.0,
        "minimal" => 0.1,
        "low" => 0.2,
        "medium" => 0.5,
        "high" => 0.8,
        "xhigh" | "max" => 0.95,
        _ => 0.5,
    }
}

/// Token budget for budget-style reasoning knobs. `None` for `off`
/// (caller omits the knob); otherwise `max_tokens * ratio` clamped to
/// [1024, 128000]. The caller must keep the budget below its output
/// limit so tokens remain for the final answer.
pub fn thinking_budget(max_tokens: u32, level: &str) -> Option<u32> {
    if normalize_thinking_level(level) == "off" {
        return None;
    }
    let raw = max_tokens as f64 * effort_ratio(level);
    Some(raw.clamp(1024.0, 128_000.0) as u32)
}

/// Native-budget steps for the direct Gemini knob. `None` for `off`.
pub fn native_thinking_budget(level: &str) -> Option<u32> {
    match normalize_thinking_level(level).as_str() {
        "off" => None,
        "minimal" => Some(1024),
        "low" => Some(2048),
        "medium" => Some(8192),
        "high" => Some(16384),
        "xhigh" => Some(24576),
        "max" => Some(32768),
        _ => Some(8192),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn normalize_accepts_spectrum_and_aliases() {
        for l in THINKING_LEVELS {
            assert_eq!(normalize_thinking_level(l), l);
            assert_eq!(normalize_thinking_level(&l.to_uppercase()), l);
            assert_eq!(normalize_thinking_level(&format!("  {l} ")), l);
        }
        assert_eq!(normalize_thinking_level("none"), "off");
        assert_eq!(normalize_thinking_level("bogus"), "medium");
        assert_eq!(normalize_thinking_level(""), "medium");
    }

    #[test]
    fn cycle_wraps_both_ways() {
        assert_eq!(cycle_thinking_level("medium", 1), "high");
        assert_eq!(cycle_thinking_level("medium", -1), "low");
        assert_eq!(cycle_thinking_level("max", 1), "off");
        assert_eq!(cycle_thinking_level("off", -1), "max");
        assert_eq!(cycle_thinking_level("bogus", 1), "high");
    }

    #[test]
    fn ratios_and_budgets_match_unified_vocabulary() {
        assert_eq!(effort_ratio("off"), 0.0);
        assert_eq!(effort_ratio("minimal"), 0.1);
        assert_eq!(effort_ratio("low"), 0.2);
        assert_eq!(effort_ratio("medium"), 0.5);
        assert_eq!(effort_ratio("high"), 0.8);
        assert_eq!(effort_ratio("xhigh"), 0.95);
        assert_eq!(effort_ratio("max"), 0.95);
        assert_eq!(thinking_budget(10_000, "off"), None);
        assert_eq!(thinking_budget(10_000, "medium"), Some(5000));
        assert_eq!(thinking_budget(10_000, "low"), Some(2000));
        assert_eq!(thinking_budget(100, "high"), Some(1024), "floor");
        assert_eq!(thinking_budget(1_000_000, "max"), Some(128_000), "ceiling");
        assert_eq!(native_thinking_budget("off"), None);
        assert_eq!(native_thinking_budget("medium"), Some(8192));
    }

    #[test]
    fn resolve_prefers_env_over_everything() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(THINKING_ENV_VAR).ok();
        std::env::set_var(THINKING_ENV_VAR, "high");
        assert_eq!(resolve_thinking_level(), "high");
        std::env::set_var(THINKING_ENV_VAR, "HIGH");
        assert_eq!(resolve_thinking_level(), "high");
        std::env::set_var(THINKING_ENV_VAR, "bogus");
        assert_eq!(resolve_thinking_level(), "medium");
        match prev {
            Some(v) => std::env::set_var(THINKING_ENV_VAR, v),
            None => std::env::remove_var(THINKING_ENV_VAR),
        }
    }

    #[test]
    fn resolve_defaults_without_env_or_files() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_env = std::env::var(THINKING_ENV_VAR).ok();
        let prev_cfg = std::env::var("VIORAHARNESS_CONFIG").ok();
        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        std::env::remove_var(THINKING_ENV_VAR);
        let dir = std::env::temp_dir().join(format!("vh_think_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("vioraharness.json");
        std::fs::write(&cfg, "{}").unwrap();
        std::env::set_var("VIORAHARNESS_CONFIG", &cfg);
        std::env::set_var("XDG_DATA_HOME", &dir);
        // cwd walk may still find the repo config, which carries no
        // thinking_level — either way the answer must be the default.
        assert_eq!(resolve_thinking_level(), "medium");
        match prev_env {
            Some(v) => std::env::set_var(THINKING_ENV_VAR, v),
            None => std::env::remove_var(THINKING_ENV_VAR),
        }
        match prev_cfg {
            Some(v) => std::env::set_var("VIORAHARNESS_CONFIG", v),
            None => std::env::remove_var("VIORAHARNESS_CONFIG"),
        }
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
