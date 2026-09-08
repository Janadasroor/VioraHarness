use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "vioraharness",
    version,
    about = "VioraHarness — custom coding agent harness for VioraEDA",
    long_about = "Full-control harness (Rust + Ratatui + OpenRouter/Gemini) for VioraEDA. \
                  See roadmap.md for architecture and phases."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, env = "VIORAHARNESS_MODEL")]
    model: Option<String>,

    #[arg(long)]
    config: Option<String>,

    #[arg(long)]
    cwd: Option<String>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    #[command(alias = "chat", alias = "ui")]
    Tui {
        #[arg(long)]
        model: Option<String>,
    },

    #[command(alias = "server")]
    Serve {
        #[arg(long, default_value = "4096")]
        port: u16,
    },

    #[command(visible_alias = "ask", alias = "prompt")]
    Run {
        prompt: String,

        #[arg(long)]
        model: Option<String>,

        #[arg(long, alias = "session-id", alias = "sid")]
        session: Option<String>,

        #[arg(long = "continue", short = 'c')]
        cont: bool,

        #[arg(long, short, env = "VIORAHARNESS_AUTO_ALLOW")]
        yes: bool,
    },

    Exec {
        prompt: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        session: Option<String>,

        #[arg(long = "continue", short = 'c')]
        cont: bool,

        #[arg(long, short, env = "VIORAHARNESS_AUTO_ALLOW")]
        yes: bool,
    },

    #[command(visible_alias = "check")]
    Doctor,

    #[command(
        visible_alias = "chats",
        visible_alias = "list",
        alias = "ls",
        alias = "conversations",
        alias = "convs"
    )]
    Sessions {
        #[arg(long)]
        all: bool,

        #[arg(long)]
        search: Option<String>,

        #[arg(long, default_value = "20")]
        limit: usize,
    },

    #[command(
        visible_alias = "clean",
        alias = "prune",
        about = "Garbage-collect stale snapshot state (DB rows + .vioraharness/snapshots dirs)"
    )]
    Gc {
        #[arg(long, default_value = "7")]
        days: u64,

        #[arg(long)]
        dry_run: bool,
    },

    #[command(
        visible_alias = "open",
        visible_alias = "r",
        alias = "continue",
        alias = "restore"
    )]
    Resume {
        #[arg(default_value = "")]
        session: String,
    },

    #[command(alias = "branch", alias = "copy")]
    Fork {
        session: String,

        #[arg(long)]
        at: Option<i64>,
    },

    #[command(alias = "title", alias = "mv")]
    Rename { session: String, title: String },

    #[command(alias = "hide")]
    Archive { session: String },

    #[command(visible_alias = "rm", alias = "remove", alias = "drop")]
    Delete { session: String },

    #[command(alias = "dump", alias = "save")]
    Export {
        session: String,

        #[arg(long)]
        output: Option<String>,
    },

    #[command(alias = "revert", alias = "unwind")]
    Undo {
        #[arg(default_value = "")]
        session: String,
    },

    #[command(visible_alias = "log", alias = "messages", alias = "show")]
    History {
        #[arg(default_value = "")]
        session: String,
    },

    #[command(hide = true)]
    LastSession,
}

/// Rotate `log_path` to `{log_path}.1` when bigger than `max_bytes`.
/// Keeps one generation; failures are silent (logging must never crash boot).
fn rotate_log_if_big(log_path: &str, max_bytes: u64) {
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() > max_bytes {
            let prev = format!("{log_path}.1");
            let _ = std::fs::remove_file(&prev);
            let _ = std::fs::rename(log_path, &prev);
        }
    }
}

fn last_tui_model() -> Option<String> {
    let base = std::env::var("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".local/share")
        });
    let txt = std::fs::read_to_string(base.join("vioraharness/tui_state.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&txt)
        .ok()?
        .get("last_model")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

fn resolve_continue(session: Option<String>, cont: bool) -> Option<String> {
    if session.is_some() || !cont {
        return session;
    }
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".into());
    let db_path = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    match vioraharness_core::session::SessionStore::new(&db_path)
        .map_err(|e| e.to_string())
        .and_then(|s| s.latest_for_cwd(&cwd).map_err(|e| e.to_string()))
    {
        Ok(Some(sess)) => {
            println!(
                "Continuing latest chat in {}: {} ({})",
                cwd,
                &sess.id[..8.min(sess.id.len())],
                sess.title.as_deref().unwrap_or("untitled")
            );
            Some(sess.id)
        }
        _ => {
            eprintln!("no previous chat in this folder ({cwd}) — run without -c to start one");
            std::process::exit(3);
        }
    }
}

async fn cmd_run(
    prompt: String,
    model: Option<String>,
    session: Option<String>,
    cont: bool,
    yes: bool,
    cli_model: Option<String>,
) -> anyhow::Result<()> {
    if yes {
        std::env::set_var("VIORAHARNESS_AUTO_ALLOW", "1");
    }
    let m = model
        .or(cli_model)
        .or_else(last_tui_model)
        .unwrap_or_default();
    if m.trim().is_empty() {
        eprintln!(
            "no model set — pass --model provider/model-id, export VIORAHARNESS_MODEL=..., or pick once via the TUI /model picker"
        );
        std::process::exit(2);
    }
    let sid = resolve_continue(session, cont).unwrap_or_else(|| {
        format!(
            "{:010x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                & 0xffffffffff
        )
    });
    println!(
        "VioraHarness run [{m}] session={}  •  cwd: {}{}",
        sid,
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into()),
        if yes { "  •  auto-allow ON" } else { "" },
    );
    println!("Prompt: {prompt}\n--- streaming ---\n");
    let loop_ = vioraharness_core::loop_mod::AgentLoop::new();
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<vioraharness_core::provider::ProviderEvent>(256);

    let progress = tokio::spawn(async move {
        use vioraharness_core::provider::ProviderEvent as E;
        while let Some(ev) = rx.recv().await {
            match ev {
                E::TextDelta(t) => {
                    print!("{t}");
                    use std::io::Write as _;
                    let _ = std::io::stdout().flush();
                }
                E::ToolCallDelta { name, args, .. } => {
                    let short = args.chars().take(160).collect::<String>();
                    eprintln!("\n◆ {name} {short}");
                }
                E::ToolResultDelta {
                    id: _,
                    content: _,
                    ok,
                } => {
                    eprintln!("  → {}", if ok { "ok" } else { "error" });
                }
                E::Notice(msg) => {
                    eprintln!("\n… {msg}");
                }
                E::ReasoningDelta(_) | E::Done => {}
            }
        }
    });
    match loop_
        .run_streaming(&prompt, &m, Some(sid.clone()), tx)
        .await
    {
        Ok(final_text) => {
            let _ = progress.await;
            println!("\n--- done ---\n{final_text}");
            println!("\n────────────────────────────────────────────────");
            println!(
                "Chat {} • cwd: {}",
                &sid[..8.min(sid.len())],
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| ".".into())
            );
            println!("To resume: vioraharness resume {sid}  |  vioraharness run \"next\" --session {sid}");
            println!("All chats: vioraharness sessions");
            println!("────────────────────────────────────────────────");
        }
        Err(e) => {
            let _ = progress.await;
            eprintln!("\n--- error ---\n{e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    {
        let env_path = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                std::path::PathBuf::from(home).join(".config")
            })
            .join("vioraharness/.env");
        let _ = dotenvy::from_path(&env_path);
        let _ = dotenvy::dotenv();
    }

    let cli = Cli::parse();

    if let Some(dir) = &cli.cwd {
        if let Err(e) = std::env::set_current_dir(dir) {
            eprintln!("--cwd: cannot enter {dir}: {e}");
            std::process::exit(2);
        }
    }
    let is_tui = matches!(&cli.command, None | Some(Commands::Tui { .. }));
    if is_tui {
        let log_path = std::env::var("VIORAHARNESS_LOG").unwrap_or_else(|_| {
            let base = std::env::var("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    std::path::PathBuf::from(home).join(".local/share")
                });
            base.join("vioraharness/tui.log")
                .to_string_lossy()
                .to_string()
        });
        if let Some(parent) = std::path::Path::new(&log_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Bounded log (#12): rotate at 1 MiB keeping one generation.
        rotate_log_if_big(&log_path, 1024 * 1024);

        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false)
                .try_init();
        } else {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(tracing_subscriber::EnvFilter::new("off"))
                .try_init();
        }
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .try_init();
    }

    match cli.command.unwrap_or(Commands::Tui { model: None }) {
        Commands::Tui { model } => {
            let m = model.or(cli.model);
            let sid = vioraharness_tui::run(m).await?;
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| ".".into());

            let db_path = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let is_empty = match vioraharness_core::session::SessionStore::new(&db_path) {
                Ok(store) => match store.get_session(&sid) {
                    Ok(Some(_)) => store
                        .get_messages_detailed(&sid)
                        .map(|msgs| msgs.is_empty())
                        .unwrap_or(true),
                    Ok(None) => true,
                    Err(_) => true,
                },
                Err(_) => true,
            };
            if is_empty {
                println!("\n────────────────────────────────────────────────");
                println!("Exited without chat — empty, not saved.");
                println!("Start new: vioraharness tui  or  vioraharness run \"hello\"");
                println!("Project folder: {}", cwd);
                println!("────────────────────────────────────────────────");
            } else {
                let last_path = std::env::var("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        std::path::PathBuf::from(home).join(".local/share")
                    })
                    .join("vioraharness/last_session");
                if let Some(parent) = last_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&last_path, &sid);

                let msg_count = vioraharness_core::session::SessionStore::new(&db_path)
                    .ok()
                    .and_then(|s| s.get_messages_detailed(&sid).ok())
                    .map(|v| v.len())
                    .unwrap_or(0);
                println!("\n────────────────────────────────────────────────");
                println!(
                    "Chat {}  •  {} msgs  •  cwd: {}",
                    &sid[..8.min(sid.len())],
                    msg_count,
                    cwd
                );
                println!("To resume this chat:");
                println!("  vioraharness resume {}", sid);
                println!(
                    "  vioraharness resume {}  # short id",
                    &sid[..8.min(sid.len())]
                );
                println!(
                    "  vioraharness tui  # then /resume {}",
                    &sid[..8.min(sid.len())]
                );
                println!("Project folder: {}", cwd);
                println!("All chats: vioraharness sessions  |  vioraharness chats  |  vioraharness history");
                println!("────────────────────────────────────────────────");
            }
        }
        Commands::Serve { port } => {
            vioraharness_server::serve(port).await?;
        }
        Commands::Run {
            prompt,
            model,
            session,
            cont,
            yes,
        } => cmd_run(prompt, model, session, cont, yes, cli.model).await?,
        Commands::Exec {
            prompt,
            model,
            session,
            cont,
            yes,
        } => cmd_run(prompt, model, session, cont, yes, cli.model).await?,
        Commands::Doctor => {
            println!("VioraHarness doctor");

            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let viospice_root = std::env::var("VIOSPICE_ROOT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "{}/qt_projects/viospice",
                        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
                    )
                });
            let cwd_is_viospice =
                cwd.to_string_lossy().contains("viospice") || cwd.starts_with(&viospice_root);
            let mut candidates: Vec<String> = Vec::new();

            if let Ok(path_var) = std::env::var("PATH") {
                for dir in path_var.split(':') {
                    let p = format!("{dir}/viora");
                    if std::path::Path::new(&p).exists() {
                        candidates.push(p);
                        break;
                    }
                }
            }
            let home_local = format!(
                "{}/.local/bin/viora",
                std::env::var("HOME").unwrap_or_default()
            );
            if std::path::Path::new(&home_local).exists() {
                candidates.push(home_local);
            }

            if cwd_is_viospice {
                candidates.push(format!("{viospice_root}/build/viora"));
                candidates.push(format!("{viospice_root}/build-debug/viora"));
            }
            candidates.push("viora".into());
            let mut found = None;
            for c in candidates {
                if std::path::Path::new(&c).exists() || c == "viora" {
                    if let Ok(out) = std::process::Command::new(&c).arg("--version").output() {
                        if out.status.success() {
                            found =
                                Some((c, String::from_utf8_lossy(&out.stdout).trim().to_string()));
                            break;
                        }
                    }
                }
            }
            match found {
                Some((p, v)) => println!("  viora: {p} ({v})"),
                None => println!(
                    "  viora: not found (global `viora` in PATH not found, or build with `cmake -B build` in ~/qt_projects/viospice when in VioraEDA)"
                ),
            }

            let cwd_examples = cwd.join("examples");
            let viospice_examples = format!("{viospice_root}/examples");
            let demo = if cwd_examples.exists() {
                cwd_examples.to_string_lossy().to_string()
            } else {
                viospice_examples.to_string()
            };
            println!(
                "  examples: {} ({})",
                if std::path::Path::new(&demo).exists() {
                    "present"
                } else {
                    "missing"
                },
                demo
            );
            let open_set = std::env::var("OPENROUTER_API_KEY")
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);
            let gem_set = std::env::var("GEMINI_API_KEY")
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);

            let mask = |k: String| {
                if k.len() <= 8 {
                    format!("{}…", &k[..k.len().min(4)])
                } else {
                    format!("{}…{}", &k[..4], &k[k.len() - 4..])
                }
            };
            if open_set {
                let k = std::env::var("OPENROUTER_API_KEY").unwrap();
                println!("  openrouter: set ({}) — use /providers to manage", mask(k));

                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(3))
                    .build()
                {
                    match client
                        .get("https://openrouter.ai/api/v1/models")
                        .header(
                            "Authorization",
                            format!("Bearer {}", std::env::var("OPENROUTER_API_KEY").unwrap()),
                        )
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                println!("    → validated ✓ {} models", cnt);
                            }
                        }
                        Ok(resp) => println!("    → validation ✗ {} (check key)", resp.status()),
                        Err(e) => println!("    → validation skipped ({e})"),
                    }
                }
            } else {
                println!("  openrouter: missing (export OPENROUTER_API_KEY or /providers)");
            }
            if gem_set {
                let k = std::env::var("GEMINI_API_KEY").unwrap();
                println!("  gemini:     set ({})", mask(k.clone()));

                println!("    → debug: curl direct API isolation (like tui live fetch)");
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                {
                    let url1 = "https://generativelanguage.googleapis.com/v1beta/openai/models";
                    println!(
                        "      curl -H \"Authorization: Bearer $GEMINI_API_KEY\" {}",
                        url1
                    );
                    match client
                        .get(url1)
                        .header("Authorization", format!("Bearer {k}"))
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                let sample: Vec<String> = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .take(5)
                                            .filter_map(|v| {
                                                v.get("id")
                                                    .and_then(|x| x.as_str())
                                                    .map(|s| s.to_string())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                println!(
                                    "        ✓ openai compat {} models — sample {:?}",
                                    cnt, sample
                                );
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            println!(
                                "        ✗ openai compat {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => println!("        ✗ openai compat request failed: {}", e),
                    }

                    let url2 =
                        "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000";
                    println!(
                        "      curl -H \"x-goog-api-key: $GEMINI_API_KEY\" \"{}\"",
                        url2
                    );
                    match client.get(url2).header("x-goog-api-key", &k).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("models")
                                    .and_then(|m| m.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                let has36 = j
                                    .get("models")
                                    .and_then(|m| m.as_array())
                                    .map(|a| {
                                        a.iter().any(|v| {
                                            v.get("name")
                                                .and_then(|n| n.as_str())
                                                .map(|s| s.contains("3.6"))
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false);
                                println!(
                                    "        ✓ native header {} models — has 3.6={} nextToken={}",
                                    cnt,
                                    has36,
                                    j.get("nextPageToken")
                                        .and_then(|t| t.as_str())
                                        .map(|s| !s.is_empty())
                                        .unwrap_or(false)
                                );
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            println!(
                                "        ✗ native header {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => println!("        ✗ native header request failed: {}", e),
                    }

                    let url3 = format!("https://generativelanguage.googleapis.com/v1beta/models?key={k}&pageSize=5");
                    println!("      curl \"https://generativelanguage.googleapis.com/v1beta/models?key=\\$GEMINI_API_KEY&pageSize=5\"");
                    match client.get(&url3).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("models")
                                    .and_then(|m| m.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                println!(
                                    "        ✓ native query param {} models (pageSize=5 sample)",
                                    cnt
                                );
                            }
                        }
                        Ok(resp) => println!("        ✗ native query {} ", resp.status()),
                        Err(e) => println!("        ✗ native query failed: {}", e),
                    }
                }
            } else {
                println!("  gemini:     missing (export GEMINI_API_KEY or /providers)");
            }
            let gateway_set = std::env::var("OPENCODE_API_KEY")
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
                || std::env::var("ZEN_API_KEY")
                    .map(|k| !k.trim().is_empty())
                    .unwrap_or(false);
            if gateway_set {
                let k = std::env::var("OPENCODE_API_KEY")
                    .or_else(|_| std::env::var("ZEN_API_KEY"))
                    .unwrap();
                println!(
                    "  gateway:    set ({}) — managed pay-per-use + subscription tiers",
                    mask(k.clone())
                );
                println!("    → debug: curl direct gateway isolation (like tui live fetch)");
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                {
                    let url_z = "https://opencode.ai/zen/v1/models";
                    println!(
                        "      curl -H \"Authorization: Bearer $OPENCODE_API_KEY\" {}",
                        url_z
                    );
                    match client
                        .get(url_z)
                        .header("Authorization", format!("Bearer {k}"))
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                let sample: Vec<String> = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .take(4)
                                            .filter_map(|v| {
                                                v.get("id")
                                                    .and_then(|x| x.as_str())
                                                    .map(|s| s.to_string())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let free: Vec<String> = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|v| {
                                                v.get("id")
                                                    .and_then(|x| x.as_str())
                                                    .filter(|s| s.contains("free"))
                                                    .map(|s| s.to_string())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                println!(
                                    "        ✓ gateway {} models — sample {:?} — free {:?}",
                                    cnt, sample, free
                                );
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            println!(
                                "        ✗ gateway {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => println!("        ✗ gateway request failed: {}", e),
                    }
                    let url_g = "https://opencode.ai/zen/go/v1/models";
                    println!(
                        "      curl -H \"Authorization: Bearer $OPENCODE_API_KEY\" {}",
                        url_g
                    );
                    match client
                        .get(url_g)
                        .header("Authorization", format!("Bearer {k}"))
                        .send()
                        .await
                    {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                let sample: Vec<String> = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .take(4)
                                            .filter_map(|v| {
                                                v.get("id")
                                                    .and_then(|x| x.as_str())
                                                    .map(|s| s.to_string())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                println!("        ✓ go {} models — sample {:?}", cnt, sample);
                            }
                        }
                        Ok(resp) => {
                            let status = resp.status();
                            let txt = resp.text().await.unwrap_or_default();
                            println!(
                                "        ✗ go {} — {}",
                                status,
                                txt.chars().take(200).collect::<String>()
                            );
                        }
                        Err(e) => println!("        ✗ go request failed: {}", e),
                    }
                }
            } else {
                println!("  gateway:    no key — free-tier (*-free) models work with public");
                if let Ok(client) = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(3))
                    .build()
                {
                    match client.get("https://opencode.ai/zen/v1/models").send().await {
                        Ok(resp) if resp.status().is_success() => {
                            if let Ok(j) = resp.json::<serde_json::Value>().await {
                                let cnt = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| a.len())
                                    .unwrap_or(0);
                                let free: Vec<String> = j
                                    .get("data")
                                    .and_then(|d| d.as_array())
                                    .map(|a| {
                                        a.iter()
                                            .filter_map(|v| {
                                                v.get("id")
                                                    .and_then(|x| x.as_str())
                                                    .filter(|s| s.contains("free"))
                                                    .map(|s| s.to_string())
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                println!("    → gateway public ✓ {} models — free {:?}", cnt, free);
                                if let Some(first) = free.first() {
                                    let id = if first.contains('/') {
                                        first.clone()
                                    } else {
                                        format!("opencode/{first}")
                                    };
                                    println!("    → try: vioraharness run \"hi\" --model {id}  (no key needed, public)");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                println!("    → for paid tiers: get a key at https://opencode.ai/auth, then add it via TUI /providers or export OPENCODE_API_KEY=...");
            }

            let env_path = std::env::var("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let h = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    std::path::PathBuf::from(h).join(".config")
                })
                .join("vioraharness/.env");
            println!(
                "  providers env: {} ({})",
                env_path.display(),
                if env_path.exists() {
                    "present"
                } else {
                    "not yet — /providers will create"
                }
            );
            println!(
                "  config:     {}",
                cli.config
                    .unwrap_or_else(|| "vioraharness.json (hierarchical)".into())
            );
            for cand in vioraharness_core::loop_mod::config_candidates() {
                if std::path::Path::new(&cand).exists() {
                    println!("  config file: {cand} (found)");
                    if let Ok(s) = std::fs::read_to_string(&cand) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                            if let Some(perms) = v.get("permissions") {
                                println!(
                                    "    permissions: {} rules",
                                    perms.as_object().map(|o| o.len()).unwrap_or(0)
                                );
                            }
                            if let Some(prov) = v.get("provider") {
                                println!(
                                    "    provider keys: {}",
                                    prov.as_object()
                                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                                        .unwrap_or_default()
                                );
                            }
                        }
                    }
                    break;
                }
            }
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            println!("  db: {db}");
            match vioraharness_core::session::SessionStore::new(&db) {
                Ok(store) => {
                    let list = store.list_sessions().unwrap_or_default();
                    println!("    sessions: {}", list.len());
                }
                Err(e) => println!("    db error: {e}"),
            }
        }
        Commands::Gc { days, dry_run } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let snapshots_root = cwd.join(".vioraharness/snapshots");
            let max_idle = std::time::Duration::from_secs(days.saturating_mul(24 * 3600));
            let rep = vioraharness_core::session::gc::gc_snapshots(
                &store,
                &snapshots_root,
                max_idle,
                dry_run,
            )?;
            if dry_run {
                println!(
                    "gc --dry-run (idle > {days}d, root {})",
                    snapshots_root.display()
                );
            } else {
                println!("gc (idle > {days}d, root {})", snapshots_root.display());
            }
            println!(
                "  sessions pruned: {}{}",
                rep.snapshot_sessions_pruned.len(),
                if rep.snapshot_sessions_pruned.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", rep.snapshot_sessions_pruned.join(", "))
                }
            );
            println!("  snapshot rows: {}", rep.snapshot_rows_deleted);
            println!(
                "  snapshot dirs: {} ({:.1} KiB freed)",
                rep.snapshot_dirs_removed,
                rep.bytes_freed as f64 / 1024.0
            );
            if dry_run {
                println!("  (nothing removed — rerun without --dry-run to apply)");
            }
        }
        Commands::Sessions { all, search, limit } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let sessions = store.list_sessions_filtered(None, search.as_deref(), all, limit, 0)?;
            if sessions.is_empty() {
                println!(
                    "No chats yet. Run `vioraharness run \"hello\"` or TUI /new to create one."
                );
            } else {
                println!(
                    "Chats (sorted by updated_at) — {} total (showing {}):",
                    sessions.len(),
                    sessions.len().min(limit)
                );
                for s in sessions {
                    let upd = {
                        let age = (std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64
                            - s.updated_at)
                            .max(0);
                        if age < 60 {
                            format!("{}s ago", age)
                        } else if age < 3600 {
                            format!("{}m ago", age / 60)
                        } else if age < 86400 {
                            format!("{}h ago", age / 3600)
                        } else {
                            format!("{}d ago", age / 86400)
                        }
                    };
                    let parent = s
                        .parent_id
                        .as_deref()
                        .map(|p| format!(" fork:{}", &p[..8.min(p.len())]))
                        .unwrap_or_default();
                    let arch = if s.archived_at.is_some() {
                        " [archived]"
                    } else {
                        ""
                    };
                    println!(
                        "{} | {} | {} | {}{} | upd:{} | {}",
                        &s.id[..8.min(s.id.len())],
                        s.model,
                        s.status,
                        s.title.clone().unwrap_or_else(|| "(untitled)".into()),
                        parent,
                        upd,
                        arch,
                    );
                    println!(
                        "  id={} updated_at={} created_at={}",
                        s.id, s.updated_at, s.created_at
                    );
                }
            }
        }
        Commands::Resume { session } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            if session.trim().is_empty() {
                let sessions = store.list_sessions_filtered(None, None, false, 10, 0)?;
                if sessions.is_empty() {
                    println!(
                        "No chats yet. Run `vioraharness tui` or `vioraharness run \"hello\"`"
                    );
                } else {
                    println!("Recent chats (use `vioraharness resume <id>`):");
                    for s in sessions {
                        println!(
                            "  {}  {}  {}  {}",
                            &s.id[..8.min(s.id.len())],
                            s.model,
                            s.title.clone().unwrap_or_else(|| "(untitled)".into()),
                            s.id
                        );
                    }
                    println!("\nTo resume: vioraharness resume <id>  or  vioraharness tui  then /resume <id>");
                }
                return Ok(());
            }

            let id = if let Ok(Some(s)) = store.get_session(&session) {
                s.id
            } else {
                let list = store.list_sessions_filtered(None, Some(&session), true, 10, 0)?;
                list.into_iter()
                    .find(|s| s.id.starts_with(&session))
                    .map(|s| s.id)
                    .unwrap_or(session.clone())
            };
            let sess = store.get_session(&id)?.ok_or_else(|| {
                anyhow::anyhow!("session {id} not found — try `vioraharness sessions` to list")
            })?;
            println!(
                "Resumed chat {} — model {} title {:?} updated_at {}",
                sess.id, sess.model, sess.title, sess.updated_at
            );
            println!(
                "To continue in TUI: vioraharness tui  then /resume {}",
                sess.id
            );
            println!(
                "To continue headless: vioraharness run \"your prompt\" --session {}",
                sess.id
            );
            let msgs = store.get_messages_detailed(&id)?;
            for m in msgs {
                println!(
                    "[{}] {}: {}",
                    m.seq,
                    m.role,
                    m.content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>()
                );
                if let Some(r) = m.reasoning {
                    if !r.is_empty() {
                        println!("  reasoning: {} chars", r.len());
                    }
                }
            }
        }
        Commands::Fork { session, at } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let new_id = format!(
                "{:010x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
                    & 0xffffffffff
            );
            store.fork_session(&session, &new_id, at)?;
            println!("Forked {} at seq {:?} → {}", session, at, new_id);
        }
        Commands::Rename { session, title } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            store.rename_session(&session, &title)?;
            println!("Renamed {} → '{}'", session, title);
        }
        Commands::Archive { session } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            store.archive_session(&session)?;
            println!("Archived {}", session);
        }
        Commands::Delete { session } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            store.delete_session(&session)?;
            println!("Deleted {} (CASCADE)", session);
        }
        Commands::Export { session, output } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let jsonl = store.export_jsonl(&session)?;
            if let Some(path) = output {
                std::fs::write(&path, &jsonl)?;
                println!(
                    "Exported {} → {} ({} bytes, {} lines)",
                    session,
                    path,
                    jsonl.len(),
                    jsonl.lines().count()
                );
            } else {
                print!("{}", jsonl);
            }
        }
        Commands::History { session } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let sid = if session.trim().is_empty() {
                let last_path = std::env::var("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        std::path::PathBuf::from(home).join(".local/share")
                    })
                    .join("vioraharness/last_session");
                if let Ok(last) = std::fs::read_to_string(&last_path) {
                    last.trim().to_string()
                } else {
                    let recents = store.list_sessions_filtered(None, None, false, 1, 0)?;
                    recents
                        .into_iter()
                        .next()
                        .map(|s| s.id)
                        .unwrap_or_else(|| "".into())
                }
            } else {
                session.clone()
            };
            if sid.is_empty() {
                anyhow::bail!("no session id and no last session — try `vioraharness sessions`");
            }
            let id = if let Ok(Some(s)) = store.get_session(&sid) {
                s.id
            } else {
                let list = store.list_sessions_filtered(None, Some(&sid), true, 10, 0)?;
                list.into_iter()
                    .find(|s| s.id.starts_with(&sid))
                    .map(|s| s.id)
                    .unwrap_or(sid.clone())
            };
            let msgs = store.get_messages_detailed(&id)?;
            println!("History for {} ({} msgs):", id, msgs.len());
            for m in msgs {
                println!(
                    "seq={} role={} model={:?} reasoning={} chars\n{}",
                    m.seq,
                    m.role,
                    m.model,
                    m.reasoning.as_deref().map(|r| r.len()).unwrap_or(0),
                    m.content
                );
                println!("---");
            }
            println!("\nTo resume: vioraharness resume {}", id);
        }
        Commands::Undo { session } => {
            let db = std::env::var("VIORAHARNESS_DB")
                .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
            let store = vioraharness_core::session::SessionStore::new(&db)?;
            let sid = if session.trim().is_empty() {
                let last_path = std::env::var("XDG_DATA_HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        std::path::PathBuf::from(home).join(".local/share")
                    })
                    .join("vioraharness/last_session");
                if let Ok(last) = std::fs::read_to_string(&last_path) {
                    last.trim().to_string()
                } else {
                    store
                        .list_sessions_filtered(None, None, false, 1, 0)?
                        .into_iter()
                        .next()
                        .map(|s| s.id)
                        .unwrap_or_default()
                }
            } else if let Ok(Some(s)) = store.get_session(session.trim()) {
                s.id
            } else {
                store
                    .list_sessions_filtered(None, None, true, 50, 0)?
                    .into_iter()
                    .find(|s| s.id.starts_with(session.trim()))
                    .map(|s| s.id)
                    .unwrap_or_default()
            };
            if sid.is_empty() {
                anyhow::bail!("no such session — try `vioraharness sessions`");
            }
            match vioraharness_core::session::snapshot::restore_latest(&store, &sid)? {
                Some(path) => {
                    println!("undo: restored {path} — repeat to go further back");
                }
                None => println!("undo: nothing to undo in chat {}", &sid[..8.min(sid.len())]),
            }
        }
        Commands::LastSession => {
            let last_path = std::env::var("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    std::path::PathBuf::from(home).join(".local/share")
                })
                .join("vioraharness/last_session");
            if let Ok(sid) = std::fs::read_to_string(&last_path) {
                let sid = sid.trim().to_string();
                if sid.is_empty() {
                    println!("No last session yet — run TUI first");
                } else {
                    println!("{}", sid);
                    println!("\nTo resume: vioraharness resume {}", sid);
                    println!("Short: vioraharness resume {}", &sid[..8.min(sid.len())]);
                }
            } else {
                println!("No last session file — try `vioraharness sessions`");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

    #[test]
    fn cli_parses_subcommands_and_aliases() {
        let cli = Cli::try_parse_from(["vh", "run", "hello", "-c", "-y"]).unwrap();
        match cli.command {
            Some(Commands::Run {
                prompt, cont, yes, ..
            }) => {
                assert_eq!(prompt, "hello");
                assert!(cont);
                assert!(yes);
            }
            other => panic!("unexpected: {other:?}"),
        }
        let cli = Cli::try_parse_from(["vh", "ask", "hi"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Run { .. })));
        let cli = Cli::try_parse_from(["vh", "server"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Serve { .. })));
        let cli = Cli::try_parse_from(["vh", "serve", "--port", "5000"]).unwrap();
        match cli.command {
            Some(Commands::Serve { port }) => assert_eq!(port, 5000),
            other => panic!("unexpected: {other:?}"),
        }
        let cli = Cli::try_parse_from(["vh", "check"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Doctor)));
        let cli = Cli::try_parse_from(["vh", "tui", "--model", "m"]).unwrap();
        match cli.command {
            Some(Commands::Tui { model }) => assert_eq!(model.as_deref(), Some("m")),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(Cli::try_parse_from(["vh", "nope"]).is_err());
    }

    #[test]
    fn cli_parses_gc_with_defaults_and_alias() {
        match Cli::try_parse_from(["vh", "gc"]).unwrap().command {
            Some(Commands::Gc { days, dry_run }) => {
                assert_eq!(days, 7);
                assert!(!dry_run);
            }
            other => panic!("unexpected: {other:?}"),
        }
        match Cli::try_parse_from(["vh", "clean", "--days", "30", "--dry-run"])
            .unwrap()
            .command
        {
            Some(Commands::Gc { days, dry_run }) => {
                assert_eq!(days, 30);
                assert!(dry_run);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn resolve_continue_pure_paths() {
        assert_eq!(
            resolve_continue(Some("abc".into()), true),
            Some("abc".into())
        );
        assert_eq!(
            resolve_continue(Some("abc".into()), false),
            Some("abc".into())
        );
        assert_eq!(resolve_continue(None, false), None);
    }

    #[test]
    fn resolve_continue_finds_latest_in_cwd() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("VIORAHARNESS_DB").ok();
        let db = std::env::temp_dir().join(format!("vh_main_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        std::env::set_var("VIORAHARNESS_DB", &db);
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let store = vioraharness_core::session::SessionStore::new(db.to_str().unwrap()).unwrap();
        store
            .create_session_full(
                "sess-main",
                "m",
                Some("t"),
                Some(&cwd),
                None,
                None,
                None,
                None,
            )
            .unwrap();
        store.append_message("sess-main", "user", "hi").unwrap();
        assert_eq!(resolve_continue(None, true), Some("sess-main".into()));
        match prev {
            Some(v) => std::env::set_var("VIORAHARNESS_DB", v),
            None => std::env::remove_var("VIORAHARNESS_DB"),
        }
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn log_rotates_past_limit() {
        let dir = std::env::temp_dir().join(format!("vh-logrot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("tui.log");
        let log_s = log.to_string_lossy().to_string();
        std::fs::write(&log, vec![b'x'; 100]).unwrap();
        rotate_log_if_big(&log_s, 1024 * 1024);
        assert!(log.exists(), "small log kept");
        assert!(!dir.join("tui.log.1").exists());
        std::fs::write(&log, vec![b'y'; 200]).unwrap();
        rotate_log_if_big(&log_s, 100);
        assert!(!log.exists(), "big log moved away");
        assert_eq!(std::fs::read(dir.join("tui.log.1")).unwrap().len(), 200);
        rotate_log_if_big(&dir.join("missing.log").to_string_lossy(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
