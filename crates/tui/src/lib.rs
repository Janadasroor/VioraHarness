// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

pub mod app;
pub mod theme;
pub mod widgets;

pub async fn run(model: Option<String>) -> anyhow::Result<String> {
    install_panic_hook();
    let m = model
        .or_else(|| std::env::var("VIORAHARNESS_MODEL").ok())
        .unwrap_or_default();
    let app = app::App::new(m);
    app.run().await
}

/// A TUI panic otherwise leaves no trace: the message goes to stderr
/// (not `tui.log`) and the alternate screen stays active, so the shell
/// looks "crashed" with no diagnosable line. Log the panic payload +
/// location, best-effort restore the terminal, then run the default
/// hook (still prints to stderr).
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!("tui panic: {info}");
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
            ratatui::restore();
            default(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_hook_installs_without_firing() {
        // Installing must be idempotent and must not invoke the hook.
        install_panic_hook();
        install_panic_hook();
        let out = std::panic::catch_unwind(|| 2 + 2);
        assert!(matches!(out, Ok(4)));
    }
}
