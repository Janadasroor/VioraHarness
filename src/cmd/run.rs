// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::super::cli::{last_tui_model, resolve_continue};

pub(crate) async fn cmd_run(
    prompt: String,
    model: Option<String>,
    session: Option<String>,
    cont: bool,
    yes: bool,
    cli_model: Option<String>,
    cli_mode: Option<String>,
) -> anyhow::Result<()> {
    if yes {
        std::env::set_var("VIORAHARNESS_AUTO_ALLOW", "1");
    }
    if let Some(ref md) = cli_mode {
        if !vioraharness_core::mode::is_known_mode(md) {
            eprintln!("unknown mode: {md} (known: eda, web, android) — see /mode in the TUI");
            std::process::exit(2);
        }
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
    // Mode precedence: --mode/VIORAHARNESS_MODE > resumed session's stored
    // mode > project activeMode > eda.
    let mode = match cli_mode {
        Some(md) => vioraharness_core::mode::normalize_mode_name(&md),
        None => {
            let stored = std::env::var("VIORAHARNESS_DB")
                .ok()
                .and_then(|db| {
                    vioraharness_core::session::SessionStore::new(&db)
                        .ok()
                        .and_then(|s| s.get_session(&sid).ok().flatten())
                        .and_then(|sess| sess.mode)
                })
                .filter(|mm| vioraharness_core::mode::is_known_mode(mm));
            stored.unwrap_or_else(|| vioraharness_core::mode::resolve_mode(None))
        }
    };
    println!(
        "VioraHarness run [{m}] mode={mode} session={}  •  cwd: {}{}",
        sid,
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into()),
        if yes { "  •  auto-allow ON" } else { "" },
    );
    println!("Prompt: {prompt}\n--- streaming ---\n");
    let loop_ = vioraharness_core::loop_mod::AgentLoop::with_mode(&mode);
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
