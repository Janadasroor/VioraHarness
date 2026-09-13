use clap::Parser;
use cli::{rotate_log_if_big, Cli, Commands};
use cmd::run::cmd_run;

mod cli;
mod cmd;

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
            // Explicit --mode wins over the saved TUI default: seed the env
            // chain before App::new resolves (saved last_mode < --mode).
            if let Some(ref md) = cli.mode {
                if !vioraharness_core::mode::is_known_mode(md) {
                    eprintln!("unknown mode: {md} (known: eda, web) — see /mode in the TUI");
                    std::process::exit(2);
                }
                std::env::set_var(vioraharness_core::mode::MODE_ENV_VAR, md);
            }
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
        Commands::Shim { port } => {
            cmd::shim::cmd_shim(port).await?;
        }
        Commands::Run {
            prompt,
            model,
            mode,
            session,
            cont,
            yes,
        } => {
            cmd_run(
                prompt,
                model,
                session,
                cont,
                yes,
                cli.model,
                mode.or(cli.mode),
            )
            .await?
        }
        Commands::Exec {
            prompt,
            model,
            mode,
            session,
            cont,
            yes,
        } => {
            cmd_run(
                prompt,
                model,
                session,
                cont,
                yes,
                cli.model,
                mode.or(cli.mode),
            )
            .await?
        }
        Commands::Doctor => cmd::doctor::cmd_doctor(cli.config).await?,
        Commands::Gc { days, dry_run } => cmd::manage::cmd_gc(days, dry_run)?,
        Commands::RunLoop {
            queue,
            model,
            max_tasks,
            token_budget,
            yes,
        } => {
            cmd::runloop::cmd_run_loop(queue, model, max_tasks, token_budget, yes, cli.model)
                .await?
        }
        Commands::Sessions { all, search, limit } => cmd::manage::cmd_sessions(all, search, limit)?,
        Commands::Resume { session } => cmd::manage::cmd_resume(session)?,
        Commands::Fork { session, at } => cmd::manage::cmd_fork(session, at)?,
        Commands::Rename { session, title } => cmd::manage::cmd_rename(session, title)?,
        Commands::Archive { session } => cmd::manage::cmd_archive(session)?,
        Commands::Delete { session } => cmd::manage::cmd_delete(session)?,
        Commands::Export { session, output } => cmd::manage::cmd_export(session, output)?,
        Commands::History { session } => cmd::manage::cmd_history(session)?,
        Commands::Undo { session } => cmd::manage::cmd_undo(session)?,
        Commands::LastSession => cmd::manage::cmd_last_session()?,
    }

    Ok(())
}
