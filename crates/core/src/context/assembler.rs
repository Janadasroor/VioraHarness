use std::path::Path;

pub struct AssembledContext {
    pub system: String,
    pub user_extra: String,
}

pub fn assemble_context(prompt: &str) -> AssembledContext {
    let mut system = String::new();

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    system.push_str(&format!(
        "You are VioraHarness, an expert coding agent. You work in the current project folder: {cwd}\n",
    ));
    system.push_str(
        "Use tools to read/write files relative to this folder. Always use --json and validate inputs.\n",
    );
    system.push_str("If the project is VioraEDA (contains .cir, .flxsch, viora CLI), follow Netlist-first: write .cir -> netlist_run --measure --assert -> raw_export -> schematic_render.\n");
    system.push_str("After any mutation, call schematic_render/pcb_render for vision feedback when applicable.\n");
    system.push_str("Validate erc/drc before pcb-compose --auto-route when doing PCB work.\n");
    system.push_str("Edits: prefer the edit tool (exact old_string) or apply_patch (*** Begin Patch) over rewrite-via-write for small changes; both snapshot for /undo.\n");
    system.push_str("Plans: for multi-step work keep a todowrite list (one in_progress at a time, mark completed as you go). When genuinely ambiguous, ask the user with the question tool instead of guessing.\n");
    system.push_str("Format: always close ``` code fences (every opener needs its closer); render comparison data as GFM pipe tables with a header row and a --- delimiter row.\n");
    system.push_str("CRITICAL: Bash tool ALWAYS saves full stdout+stderr to log file. Result JSON always contains `log` path + `lines` + `bytes` + `truncated` flag. You MUST notice this `log` field automatically — do not ignore it. If `truncated==true` or you need more than the 3-line preview, IMMEDIATELY use `read` with offset/limit or `grep` on that `log` path to get full data. Never re-run the same bash command to get data you can grep/read from log. This is automatic — check `log` after every bash.\n\n");

    let mut agents_cands = vec!["AGENTS.md".to_string()];
    if let Ok(cwd) = std::env::current_dir() {
        agents_cands.push(cwd.join("AGENTS.md").to_string_lossy().to_string());
    }

    if cwd.contains("viospice") {
        agents_cands.push(format!(
            "{}/AGENTS.md",
            crate::tools::viora::viospice_root()
        ));
    }
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

    let triggered = crate::skills::load_triggered_skills(prompt);
    for skill in &triggered {
        system.push_str(&format!(
            "\n--- Skill {} ({}) ---\n",
            skill.name, skill.description
        ));
        system.push_str(&skill.content.chars().take(3000).collect::<String>());
        system.push('\n');
        tracing::debug!("skill loaded: {} from {}", skill.name, skill.path.display());
    }
    if !triggered.is_empty() {
        tracing::info!(
            "skills triggered: {:?}",
            triggered.iter().map(|s| &s.name).collect::<Vec<_>>()
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
