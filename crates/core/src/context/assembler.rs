use std::path::Path;

pub struct AssembledContext {
    pub system: String,
    pub user_extra: String,
}

pub fn assemble_context(prompt: &str) -> AssembledContext {
    assemble_context_in_mode(prompt, None)
}

/// Assemble with an explicit agent mode. `None` resolves from the current
/// turn (`VIORAHARNESS_MODE`, held by the loop's `ModeGuard`) and falls
/// back to `eda`. The mode contributes three things: a visible
/// `Active mode:` line so the model knows its remit, the mode's
/// `system_extra` guidance, and skill scoping — intent-triggered skills
/// are gated to the mode's skill set (a web session never inherits the
/// PCB skill because the prompt mentioned "board"), while the mode's own
/// skills always load.
pub fn assemble_context_in_mode(prompt: &str, mode: Option<&str>) -> AssembledContext {
    let mut system = String::new();

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    let mode_name = mode
        .map(crate::mode::normalize_mode_name)
        .filter(|m| crate::mode::is_known_mode(m))
        .unwrap_or_else(crate::mode::ModeGuard::current);
    let mode_def = crate::mode::find_mode(&mode_name);
    system.push_str(&format!(
        "You are VioraHarness, an expert coding agent. You work in the current project folder: {cwd}\n",
    ));
    system.push_str(&format!(
        "Active mode: {mode_name} — {}. Stay inside this mode's tools; anything outside it is unavailable.\n",
        mode_def
            .map(|m| m.description)
            .unwrap_or("full tool access"),
    ));
    if let Some(def) = mode_def {
        if !def.system_extra.is_empty() {
            system.push_str(def.system_extra);
            system.push('\n');
        }
    }
    system.push_str(
        "Use tools to read/write files relative to this folder. Always use --json and validate inputs.\n",
    );
    system.push_str("If the project is VioraEDA (contains .cir, .flxsch, viora CLI), follow Netlist-first: write .cir -> netlist_run --measure --assert -> raw_export -> schematic_render.\n");
    system.push_str("After any mutation, call schematic_render/pcb_render for vision feedback when applicable.\n");
    system.push_str("Validate erc/drc before pcb-compose --auto-route when doing PCB work.\n");
    system.push_str("Edits: prefer the edit tool (exact old_string) or apply_patch (*** Begin Patch) over rewrite-via-write for small changes; both snapshot for /undo.\n");
    system.push_str("Plans: for multi-step work keep a todowrite list (one in_progress at a time, mark completed as you go). When genuinely ambiguous, ask the user with the question tool instead of guessing.\n");
    system.push_str("Reads: every read tool result already shows the user a `path (N lines)` card. NEVER reproduce file contents in your replies — cite `path:line` instead and summarize. The user reads code themselves; pasting dumps wastes their context and attention.\n");
    system.push_str("Format: always close ``` code fences (every opener needs its closer); render comparison data as GFM pipe tables with a header row and a --- delimiter row.\n");
    system.push_str("CRITICAL: Bash tool ALWAYS saves full stdout+stderr to log file. Result JSON always contains `log` path + `lines` + `bytes` + `truncated` flag. You MUST notice this `log` field automatically — do not ignore it. If `truncated==true` or you need more than the 3-line preview, IMMEDIATELY use `read` with offset/limit or `grep` on that `log` path to get full data. Never re-run the same bash command to get data you can grep/read from log. This is automatic — check `log` after every bash.\n\n");

    let mut agents_cands = vec!["AGENTS.md".to_string()];
    if let Ok(cwd) = std::env::current_dir() {
        agents_cands.push(cwd.join("AGENTS.md").to_string_lossy().to_string());
    }

    // NOTE: never pull AGENTS.md from the VioraEDA source tree — the harness
    // must not read the simulator's own sources. Project instructions come
    // from the working directory only.
    for cand in agents_cands {
        if Path::new(&cand).exists() {
            if let Ok(content) = std::fs::read_to_string(&cand) {
                system.push_str("--- AGENTS.md ---\n");
                system.push_str(&content.chars().take(4000).collect::<String>());
                system.push('\n');
                break;
            }
        }
    }

    if Path::new("vioraharness.json").exists() {
        system.push_str("(vioraharness.json present — permissions configured)\n");
    }

    let triggered =
        crate::skills::load_triggered_skills_in_mode(prompt, mode_def.map(|m| m.skills));
    // The mode's own skills always load, even without a keyword hit.
    let mut skills = triggered;
    if let Some(def) = mode_def {
        for name in def.skills {
            if !skills.iter().any(|s| s.name == *name) {
                if let Some(skill) = crate::skills::get_skill(name) {
                    skills.push(skill);
                }
            }
        }
    }
    for skill in &skills {
        system.push_str(&format!(
            "\n--- Skill {} ({}) ---\n",
            skill.name, skill.description
        ));
        system.push_str(&skill.content.chars().take(3000).collect::<String>());
        system.push('\n');
        tracing::debug!("skill loaded: {} from {}", skill.name, skill.path.display());
    }
    if !skills.is_empty() {
        tracing::info!(
            "skills triggered: {:?}",
            skills.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }

    let mut user_extra = String::new();

    if let Ok(branch) = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .output()
    {
        if branch.status.success() {
            let b = String::from_utf8_lossy(&branch.stdout).trim().to_string();
            if !b.is_empty() {
                user_extra.push_str(&format!("git branch: {b}\n"));
            }
        }
    }

    user_extra.push_str(&format!("date: {}\n", chrono_or_fallback()));

    user_extra.push_str(&format!(
        "platform: {} {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));

    AssembledContext { system, user_extra }
}

fn chrono_or_fallback() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M %Z")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK;

    #[test]
    fn system_covers_workflow_invariants() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = assemble_context("do pcb work");
        for needle in [
            "VioraHarness",
            "Netlist-first",
            "schematic_render",
            "erc",
            "todowrite",
            "question tool",
        ] {
            assert!(ctx.system.contains(needle), "missing: {needle}");
        }
        assert!(ctx
            .system
            .contains(&std::env::current_dir().unwrap().display().to_string()));
    }

    #[test]
    fn user_extra_has_platform_and_date() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = assemble_context("hi");
        assert!(ctx.user_extra.contains("platform:"));
        assert!(ctx.user_extra.contains(std::env::consts::OS));
        assert!(ctx.user_extra.contains("date:"));
    }

    #[test]
    fn web_mode_marks_remits_and_gates_skills() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(crate::mode::MODE_ENV_VAR).ok();
        std::env::remove_var(crate::mode::MODE_ENV_VAR);
        let ctx = assemble_context_in_mode("fix the pcb board layout", Some("web"));
        match prev {
            Some(v) => std::env::set_var(crate::mode::MODE_ENV_VAR, v),
            None => std::env::remove_var(crate::mode::MODE_ENV_VAR),
        }
        assert!(ctx.system.contains("Active mode: web"), "mode line");
        assert!(ctx.system.contains("dev_serve"), "web guidance");
        assert!(
            !ctx.system.contains("--- Skill pcb"),
            "pcb skill must not leak into web mode via keyword"
        );
        assert!(
            !ctx.system.contains("--- Skill erc"),
            "erc skill must not leak into web mode via keyword"
        );
    }

    #[test]
    fn eda_mode_keeps_full_prompt() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = assemble_context_in_mode("do pcb work", Some("eda"));
        assert!(ctx.system.contains("Active mode: eda"));
        assert!(ctx.system.contains("Netlist-first"));
    }
}
