use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "vioraharness",
    version,
    about = "VioraHarness — custom coding agent harness for VioraEDA",
    long_about = "Full-control harness (Rust + Ratatui + OpenRouter/Gemini) for VioraEDA. \
                  See roadmap.md for architecture and phases."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[arg(long, env = "VIORAHARNESS_MODEL")]
    pub(crate) model: Option<String>,

    #[arg(long)]
    pub(crate) config: Option<String>,

    #[arg(long)]
    pub(crate) cwd: Option<String>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
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
        visible_alias = "batch",
        about = "Run tasks/pending/*.md overnight: one fresh session per task, validate, scoped git commit, quarantine failures"
    )]
    RunLoop {
        #[arg(long, default_value = "tasks")]
        queue: String,

        #[arg(long)]
        model: Option<String>,

        #[arg(long)]
        max_tasks: Option<usize>,

        #[arg(long, default_value = "0")]
        token_budget: u64,

        #[arg(long, short, env = "VIORAHARNESS_AUTO_ALLOW")]
        yes: bool,
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
pub(crate) fn rotate_log_if_big(log_path: &str, max_bytes: u64) {
    if let Ok(meta) = std::fs::metadata(log_path) {
        if meta.len() > max_bytes {
            let prev = format!("{log_path}.1");
            let _ = std::fs::remove_file(&prev);
            let _ = std::fs::rename(log_path, &prev);
        }
    }
}

pub(crate) fn last_tui_model() -> Option<String> {
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

pub(crate) fn resolve_continue(session: Option<String>, cont: bool) -> Option<String> {
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
    fn cli_parses_run_loop() {
        match Cli::try_parse_from(["vh", "run-loop"]).unwrap().command {
            Some(Commands::RunLoop {
                queue,
                model,
                max_tasks,
                token_budget,
                yes,
            }) => {
                assert_eq!(queue, "tasks");
                assert!(model.is_none());
                assert!(max_tasks.is_none());
                assert_eq!(token_budget, 0);
                assert!(!yes);
            }
            other => panic!("unexpected: {other:?}"),
        }
        match Cli::try_parse_from([
            "vh",
            "batch",
            "--queue",
            "work",
            "--max-tasks",
            "3",
            "--token-budget",
            "50000",
            "-y",
        ])
        .unwrap()
        .command
        {
            Some(Commands::RunLoop {
                queue,
                max_tasks,
                token_budget,
                yes,
                ..
            }) => {
                assert_eq!(queue, "work");
                assert_eq!(max_tasks, Some(3));
                assert_eq!(token_budget, 50000);
                assert!(yes);
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
