pub(crate) fn render_skill_file(name: &str, description: &str, triggers: &[String]) -> String {
    let trig = if triggers.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", triggers.join(", "))
    };
    let trig_line = if triggers.is_empty() {
        "—".to_string()
    } else {
        triggers.join(", ")
    };
    format!(
        "---\nname: {name}\ndescription: {description}\ntriggers: {trig}\n---\n\
         # {cap} Skill\n\
         > Fill in when-to-use and step-by-step instructions. This skill auto-loads when the user mentions: {trig_line}.\n\
         \n\
         ## When to use\n\
         {description}\n\
         \n\
         ## Instructions\n\
         1. TODO: describe the workflow step by step, naming the exact tools/commands.\n\
         2. TODO: add validation or output expectations.\n\
         \n\
         ## Examples\n\
         - TODO: one example trigger phrase → expected behavior.\n",
        cap = {
            let mut c = name.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    )
}

pub(crate) fn skill_new_request(
    args: &[&str],
    cwd: &std::path::Path,
) -> Result<(String, std::path::PathBuf), String> {
    let mut local = false;
    let mut desc_parts: Vec<&str> = Vec::new();
    for a in args {
        if matches!(*a, "--local" | "--here" | "--project" | "-l") {
            local = true;
        } else {
            desc_parts.push(a);
        }
    }
    let description = desc_parts.join(" ");
    if description.trim().is_empty() {
        return Err("usage: /skill-new [--local] <what the skill should do>".into());
    }
    let target = if local {
        cwd.join("skills")
    } else {
        vioraharness_core::skills::global_skills_dir()
    };
    Ok((description, target))
}

pub(crate) fn skill_creator_prompt(description: &str, target: &std::path::Path) -> String {
    let example = render_skill_file(
        "waveforms",
        "Plot oscilloscope traces",
        &["waveform".into(), "plot".into()],
    );
    format!(
        "Create a new skill from this description: \"{description}\".\n\
         \n\
         Rules:\n\
         - YOU choose a short lowercase name (letters, digits, `-`, `_`).\n\
         - First check the target dir with glob: if a skill with that name exists, pick another name (never overwrite).\n\
         - Write exactly one file: {target}/<name>/SKILL.md (write creates parents).\n\
         - Frontmatter format (follow exactly, with your own values):\n\
         ```\n{example}```\n\
         - Body sections: `## When to use`, `## Instructions` (concrete steps naming exact tools/commands), `## Examples`.\n\
         - Triggers: lowercase single words from the description.\n\
         - Verify with the read tool, then reply one line: Skill '<name>' ready at <path>.\n\
         - Do nothing else.",
        target = target.display()
    )
}

pub(crate) fn has_key(env: &str) -> bool {
    std::env::var(env)
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

pub(crate) fn provider_catalog() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "opencode",
            "Gateway",
            "OPENCODE_API_KEY",
            "Managed gateway — pay-per-use, free tier available",
        ),
        (
            "opencode-go",
            "Gateway Go",
            "OPENCODE_API_KEY",
            "Managed open-model endpoint — same key (Go catalog)",
        ),
        (
            "openrouter",
            "OpenRouter",
            "OPENROUTER_API_KEY",
            "75+ models via one key — recommended",
        ),
        (
            "gemini",
            "Google Gemini",
            "GEMINI_API_KEY",
            "Native Gemini thinking",
        ),
        (
            "anthropic",
            "Anthropic",
            "ANTHROPIC_API_KEY",
            "Direct Claude",
        ),
        ("openai", "OpenAI", "OPENAI_API_KEY", "Direct GPT"),
    ]
}

pub(crate) fn providers_env_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("vioraharness/.env")
}

pub(crate) fn persist_provider_key(
    env_name: &str,
    key: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let path = providers_env_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let line = format!("{env_name}={key}");

    let mut filtered: Vec<String> = existing
        .lines()
        .filter(|l| !l.trim_start().starts_with(&format!("{env_name}=")))
        .map(|s| s.to_string())
        .collect();
    filtered.push(line);
    let content = filtered.join("\n") + "\n";
    std::fs::write(&path, content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    std::env::set_var(env_name, key);
    Ok(path)
}

pub(crate) async fn validate_provider_key(provider_id: &str, key: &str) -> Result<usize, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    match provider_id {
        "opencode" => {
            let resp = client
                .get("https://opencode.ai/zen/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "opencode-go" => {
            let resp = client
                .get("https://opencode.ai/zen/go/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "openrouter" => {
            let resp = client
                .get("https://openrouter.ai/api/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "gemini" => {
            let url = format!("https://generativelanguage.googleapis.com/v1beta/models?key={key}");
            let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(format!(
                    "{status} — {}",
                    txt.chars().take(120).collect::<String>()
                ));
            }
            let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let cnt = json
                .get("models")
                .and_then(|m| m.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(cnt)
        }
        "anthropic" | "openai" => {
            if key.trim().len() < 10 {
                return Err("key too short".into());
            }
            Ok(0)
        }
        _ => Err("unknown provider".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_new_request_parsing() {
        use std::path::PathBuf;
        let cwd = PathBuf::from("/proj");

        assert!(skill_new_request(&[], &cwd).is_err());
        assert!(skill_new_request(&["--local"], &cwd).is_err());

        let (desc, target) = skill_new_request(&["plot", "waveforms"], &cwd).unwrap();
        assert_eq!(desc, "plot waveforms");
        assert!(
            target.ends_with(".config/vioraharness/skills"),
            "{target:?}"
        );

        for flag in ["--local", "--here", "--project", "-l"] {
            let (desc, target) = skill_new_request(&[flag, "plot", "x"], &cwd).unwrap();
            assert_eq!(target, PathBuf::from("/proj/skills"), "{flag}");
            assert_eq!(desc, "plot x");
        }
        let (_, target) = skill_new_request(&["plot", "--local"], &cwd).unwrap();
        assert_eq!(target, PathBuf::from("/proj/skills"));
    }

    #[test]
    fn skill_creator_prompt_content() {
        use std::path::PathBuf;
        let p = skill_creator_prompt("plot waveforms", &PathBuf::from("/g/skills"));
        assert!(p.contains("plot waveforms"), "carries the description");
        assert!(p.contains("/g/skills"), "carries the target dir");
        assert!(
            p.contains("YOU choose a short lowercase name"),
            "model names it"
        );
        assert!(p.contains("name: waveforms"), "format example present");
        assert!(p.contains("never overwrite"), "no clobber rule");
        assert!(p.contains("read tool"), "verification step");
    }

    #[test]
    fn skill_file_renders_frontmatter() {
        let body = render_skill_file("demo", "Does demo things", &["demo".into(), "try".into()]);
        assert!(body.starts_with("---\nname: demo\n"));
        assert!(body.contains("description: Does demo things"));
        assert!(body.contains("triggers: [demo, try]"));
        assert!(body.contains("# Demo Skill"));
        let empty = render_skill_file("e", "d", &[]);
        assert!(empty.contains("triggers: []"));
    }
}
