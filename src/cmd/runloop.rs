// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::super::cli::last_tui_model;
use vioraharness_core::runloop::{run_batch, BatchOptions, LoopExecutor};

pub(crate) async fn cmd_run_loop(
    queue: String,
    model: Option<String>,
    max_tasks: Option<usize>,
    token_budget: u64,
    yes: bool,
    cli_model: Option<String>,
) -> anyhow::Result<()> {
    if !yes {
        eprintln!(
            "run-loop executes agents autonomously (edits files, runs commands, commits).\n\
             Re-run with -y/--yes (or VIORAHARNESS_AUTO_ALLOW=1) to consent.\n\
             Tip: start with `--max-tasks 1` for a supervised first run."
        );
        std::process::exit(2);
    }
    let m = model
        .or(cli_model)
        .or_else(last_tui_model)
        .unwrap_or_default();
    if m.trim().is_empty() {
        eprintln!("no model set — pass --model provider/model-id or export VIORAHARNESS_MODEL=...");
        std::process::exit(2);
    }
    let db = std::env::var("VIORAHARNESS_DB")
        .unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
    let store = vioraharness_core::session::SessionStore::new(&db)?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let queue_dir = if std::path::Path::new(&queue).is_absolute() {
        std::path::PathBuf::from(&queue)
    } else {
        cwd.join(&queue)
    };
    let agent = vioraharness_core::loop_mod::AgentLoop::new();
    let summary = run_batch(
        &BatchOptions {
            queue_dir,
            model: m,
            max_tasks: max_tasks.unwrap_or(usize::MAX),
            token_budget,
            workdir: cwd,
        },
        &store,
        &LoopExecutor { agent },
    )
    .await?;
    if !summary.failed.is_empty() {
        anyhow::bail!(
            "{} task(s) failed and were quarantined in tasks/failed/ ({} done)",
            summary.failed.len(),
            summary.done.len()
        );
    }
    Ok(())
}
