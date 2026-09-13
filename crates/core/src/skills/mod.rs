use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub path: PathBuf,
    pub content: String,
}

fn parse_frontmatter(
    content: &str,
) -> (Option<String>, Option<String>, Option<Vec<String>>, String) {
    if !content.starts_with("---") {
        return (None, None, None, content.to_string());
    }

    let mut lines = content.lines();
    let first = lines.next();
    if first != Some("---") {
        return (None, None, None, content.to_string());
    }
    let mut front_lines = Vec::new();
    let mut body_start = None;
    for (i, line) in content.lines().enumerate().skip(1) {
        if line.trim() == "---" {
            body_start = Some(i + 1);
            break;
        }
        front_lines.push(line);
    }
    let body = if let Some(start) = body_start {
        content
            .lines()
            .skip(start + 1)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content.to_string()
    };

    let mut name = None;
    let mut description = None;
    let mut triggers = None;

    for line in front_lines {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = line.strip_prefix("triggers:") {
            let val = val.trim();

            let triggers_str = val.trim_matches(|c| c == '[' || c == ']').trim();
            if !triggers_str.is_empty() {
                let list: Vec<String> = triggers_str
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                triggers = Some(list);
            }
        }
    }

    (name, description, triggers, body)
}

fn load_skill_from_path(path: &Path) -> Option<Skill> {
    let content = std::fs::read_to_string(path).ok()?;
    let (name, description, triggers, body) = parse_frontmatter(&content);

    let derived_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());
    let skill_name = name.unwrap_or(derived_name);
    let skill_desc = description.unwrap_or_default();
    let skill_triggers = triggers.unwrap_or_default();

    let full_content = if body.trim().is_empty() {
        content
    } else {
        format!("# Skill: {}\n{} \n\n{}", skill_name, skill_desc, body)
    };

    Some(Skill {
        name: skill_name,
        description: skill_desc,
        triggers: skill_triggers,
        path: path.to_path_buf(),
        content: full_content,
    })
}

fn skill_candidates() -> Vec<PathBuf> {
    let mut cands = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(entries) = std::fs::read_dir(cwd.join("skills")) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let p = entry.path().join("SKILL.md");
                    if p.exists() {
                        cands.push(p);
                    }
                }
            }
        }

        let cwd_skill = cwd.join("SKILL.md");
        if cwd_skill.exists() {
            cands.push(cwd_skill);
        }
    }

    let harness_skills_fallback = std::env::var("VIORAHARNESS_ROOT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|r| format!("{r}/skills"))
        .or_else(|| {
            let m = env!("CARGO_MANIFEST_DIR");
            let ws = if m.ends_with("crates/core") {
                m.trim_end_matches("/crates/core").to_string()
            } else {
                m.to_string()
            };
            Some(format!("{ws}/skills"))
        })
        .unwrap_or_default();
    let global_dir = global_skills_dir();
    let global_str = global_dir.to_string_lossy().to_string();
    for harness_root in [harness_skills_fallback.as_str(), global_str.as_str()] {
        if let Ok(entries) = std::fs::read_dir(harness_root) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let p = entry.path().join("SKILL.md");
                    if p.exists() && !cands.contains(&p) {
                        cands.push(p);
                    }
                }
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let maybe = parent.join("../skills");
            if maybe.exists() {
                if let Ok(entries) = std::fs::read_dir(maybe) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let p = entry.path().join("SKILL.md");
                            if p.exists() && !cands.contains(&p) {
                                cands.push(p);
                            }
                        }
                    }
                }
            }
        }
    }
    cands
}

pub fn discover_skills() -> Vec<Skill> {
    let mut skills = Vec::new();
    for path in skill_candidates() {
        if let Some(skill) = load_skill_from_path(&path) {
            skills.push(skill);
        }
    }

    let mut seen = std::collections::HashSet::new();
    skills.retain(|s| seen.insert(s.name.clone()));
    skills
}

pub fn match_skills(prompt: &str, skills: &[Skill]) -> Vec<Skill> {
    let prompt_lc = prompt.to_lowercase();
    let mut matched = Vec::new();
    for skill in skills {
        let mut triggered = false;
        for trigger in &skill.triggers {
            if prompt_lc.contains(&trigger.to_lowercase()) {
                triggered = true;
                break;
            }
        }

        if !triggered && prompt_lc.contains(&skill.name.to_lowercase()) {
            triggered = true;
        }

        if triggered {
            matched.push(skill.clone());
        }
    }
    matched
}

pub fn load_triggered_skills(prompt: &str) -> Vec<Skill> {
    let skills = discover_skills();
    match_skills(prompt, &skills)
}

/// Intent-triggered skills gated to a mode's skill set: when `allowed` is
/// `Some`, only skills named in it can trigger (a web session never
/// inherits the PCB skill via keyword coincidence). `None` = unfiltered.
pub fn load_triggered_skills_in_mode(prompt: &str, allowed: Option<&[&str]>) -> Vec<Skill> {
    let skills = discover_skills();
    let matched = match_skills(prompt, &skills);
    match allowed {
        None => matched,
        Some(allow) => matched
            .into_iter()
            .filter(|s| allow.iter().any(|a| a.eq_ignore_ascii_case(&s.name)))
            .collect(),
    }
}

pub fn list_skills() -> Vec<Skill> {
    discover_skills()
}

pub fn global_skills_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::PathBuf::from(home).join(".config/vioraharness/skills")
}

pub fn get_skill(name: &str) -> Option<Skill> {
    let lower = name.to_lowercase();
    discover_skills()
        .into_iter()
        .find(|skill| skill.name.to_lowercase() == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_skills_dir_shape() {
        let d = global_skills_dir();
        assert!(d.ends_with(".config/vioraharness/skills"), "{d:?}");
    }

    #[test]
    fn parse_frontmatter_sim() {
        let content = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../skills/sim/SKILL.md"
        ))
        .unwrap_or_else(|_| {
            std::fs::read_to_string("skills/sim/SKILL.md")
                .unwrap_or_else(|_| std::fs::read_to_string("../skills/sim/SKILL.md").unwrap())
        });
        let (name, desc, triggers, body) = parse_frontmatter(&content);
        assert_eq!(name.unwrap(), "sim");
        assert!(desc.unwrap().contains("Netlist"));
        assert!(triggers.unwrap().contains(&"netlist".to_string()));
        assert!(body.contains("write .cir"));
    }

    #[test]
    fn discover_all() {
        let skills = discover_skills();
        assert!(skills.len() >= 4);
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"sim"));
        assert!(names.contains(&"pcb"));
        assert!(names.contains(&"flux"));
        assert!(names.contains(&"erc"));
        assert!(names.contains(&"web"));
    }

    #[test]
    fn match_flux() {
        let skills = discover_skills();
        let m = match_skills("I need to write flux script", &skills);
        assert!(m.iter().any(|s| s.name == "flux"));
    }

    #[test]
    fn mode_gating_never_widens() {
        let gated = load_triggered_skills_in_mode("fix the pcb board", Some(&["web"]));
        assert!(
            gated.iter().all(|s| s.name == "web"),
            "only web skills pass a web gate: {:?}",
            gated.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        let open = load_triggered_skills_in_mode("fix the pcb board", None);
        assert!(
            open.iter().any(|s| s.name == "pcb"),
            "ungated still triggers pcb"
        );
    }
}
