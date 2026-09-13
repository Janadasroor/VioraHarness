use super::super::popups::rewind_checkpoints;
use super::super::*;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

impl App {
    pub(crate) fn draw_popup(&self, frame: &mut Frame, area: Rect) {
        let popup_area = centered_rect(60, 60, area);
        frame.render_widget(Clear, popup_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(match self.popup {
                Popup::Help => " Help — VioraHarness TUI ",
                Popup::Sessions => " Sessions (SQLite) ",
                Popup::ModelPicker => " Model Picker ",
                Popup::Permissions => " Permissions ",
                Popup::ThemePicker => " Theme Picker ",
                Popup::Providers => " Providers — API Keys ",
                Popup::Diff => " Diff — Last Write ",
                Popup::PermissionAsk => " Permission required ",
                Popup::Question => " Agent question ",
                Popup::ToolOutput => " Tool Output — Full ",
                Popup::Skills => " Skills — auto-loaded on intent ",
                Popup::Tasks => " Background Tasks ",
                Popup::Errors => " Errors ",
                Popup::Rewind => " Rewind — restore checkpoint ",
                Popup::None => " ",
            })
            .border_style(Theme::popup_border())
            .style(Style::default().bg(Color::Rgb(30, 30, 46)));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        let content = match self.popup {
            Popup::Help => vec![
                Line::from(Span::styled("Keybinds", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  Enter      send prompt (Tab to autocomplete) • Alt+Enter newline • Enter while busy queues next"),
                Line::from("  Esc        clear selection / input / cancel busy / clear pending image"),
                Line::from("  Ctrl-C     copy selection (input or chat), else quit"),
                Line::from("  Ctrl-Alt-C copy selection (never quits)"),
                Line::from("  Shift+←/→/Home/End  select input text (type/BS replaces, Ctrl-C copies)"),
                Line::from("  Mouse      drag select+auto-copy in chat and input (edge auto-scroll) • wheel scroll • middle-click pastes • Shift+drag native select"),
                Line::from("  Tab        autocomplete slash / toggle plan/build"),
                Line::from("  ↑/↓        history (prev/next prompt) • PageUp/Down scroll chat • Ctrl+J/K vim scroll"),
                Line::from("  Ctrl-U/K   delete to start/end • Ctrl-W del word • Alt+D del next word"),
                Line::from("  Ctrl-A/E   start/end • Ctrl-H backspace • Ctrl-D delete • Alt+B/F word move"),
                Line::from("  Ctrl-V     paste text • Ctrl+Alt+V text only • Ctrl+V image → vision (pending)"),
                Line::from("  Ctrl-G/O   toggle thinking • Ctrl-F search • Ctrl-L clear • Ctrl-T theme"),
                Line::from("  Ctrl-X     expand/collapse long message • /compact now or auto at 80%"),
                Line::from("  F1         this help • F2 providers • F9 tool output • F12 screenshot"),
                Line::from("  F9/Shift+V /output  view last bash/tool full output (scroll for very long)"),
                Line::from("  Permissions  Ask popup → Allow once (a) / Allow always (A) saves to vioraharness.json"),
                Line::from("  Running      header `running: bash …` + chat `◐ bash … (3s)` spinner when shell is executing"),
                Line::from(""),
                Line::from(Span::styled("Slash commands", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  /help      this help"),
                Line::from("  /clear     clear chat"),
                Line::from("  /sessions /chats /history /ls  list chats (SQLite)"),
                Line::from("  /resume /r /open <id>  resume chat (prefix ok)"),
                Line::from("  /new /fork /rename /archive /delete /export  chats"),
                Line::from("  /providers manage API keys (F2)"),
                Line::from("  /model <name>  switch model (e.g. provider/model-id from /model picker)"),
                Line::from("  /theme [name]  switch theme (tokyonight[-soft]/eye-comfort/warm-dark/catppuccin/dracula/gruvbox/nord/system)"),
                Line::from("  /thinking on/off  toggle thinking block (or Ctrl+O)"),
                Line::from("  /undo      undo last file snapshot"),
                Line::from("  /rewind    restore checkpoints at any message (dialog)"),
                Line::from("  /compact   summarize + squash history (auto at 80%)"),
                Line::from("  /output    view last tool output (bash very long, F9)"),
                Line::from("  /tasks     background tasks — list, logs, kill (bash background:true)"),
                Line::from("  /errors    error log — list, full text, clear"),
                Line::from("  /skills    list skills • /skill-new [--local] <what it should do> (AI writes it)"),
                Line::from("  /quit /q /exit  quit"),
                Line::from(""),
                Line::from(Span::styled("Markdown", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  **bold** *italic* `code` # header - list • live streaming styled"),
                Line::from(""),
                Line::from(Span::styled("Workflow", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  Netlist-first: .cir → netlist_run --measure --assert → raw_export → schematic_render"),
                Line::from(""),
                Line::from(Span::styled(format!("Theme: {}  Thinking: {} ({} / {})", crate::theme::ThemeConfig::load().theme, if self.show_thinking { "on" } else { "off" }, self.thinking_title, self.thinking_label), Style::default().fg(Color::DarkGray))),
            ],
            Popup::Skills => {
                let skills = vioraharness_core::skills::list_skills();
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(
                    format!(
                        " Skills — {} found (auto-load on intent, /skill-new to create)",
                        skills.len()
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
                if skills.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No skills yet — run /skill-new to create one.",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                for sk in &skills {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("● {} ", sk.name),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(sk.description.clone()),
                    ]));
                    lines.push(Line::from(Span::styled(
                        format!("  triggers: {}  •  {}", sk.triggers.join(", "), sk.path.display()),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " any key closes • /skill-new creates (global default, --local for ./skills)",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                lines
            }
            Popup::Sessions => {
                let db = std::env::var("VIORAHARNESS_DB").unwrap_or_else(|_| "~/.local/share/vioraharness/sessions.db".into());
                let mut lines: Vec<Line> = Vec::new();

                let sess_result = vioraharness_core::session::SessionStore::new(&db)
                    .and_then(|store| store.list_sessions_filtered(self.session_scope_filter().as_deref(), None, true, 50, 0));
                match sess_result {
                    Ok(all) if all.is_empty() => {
                        lines.push(Line::from(Span::styled("No chats yet. Run a prompt or /new to create one.", Style::default().fg(Color::Yellow))));
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(" Shortcuts: n=new  Enter=resume (when list has items)", Style::default().fg(Color::DarkGray))));
                    }
                    Ok(all) => {

                        let filter = self.session_filter.to_lowercase();
                        let filtered: Vec<_> = all.into_iter().filter(|s| {
                            if filter.is_empty() { true } else {
                                s.id.to_lowercase().contains(&filter) || s.title.as_ref().map(|t| t.to_lowercase().contains(&filter)).unwrap_or(false) || s.model.to_lowercase().contains(&filter)
                            }
                        }).collect();
                        let total = filtered.len();

                        let filter_display = if self.session_filter.is_empty() { "type to filter…".to_string() } else { self.session_filter.clone() };
                        let filter_style = if self.session_filter.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) };
                        lines.push(Line::from(vec![
                            Span::styled(" Filter: ", Style::default().fg(Color::Yellow)),
                            Span::styled(filter_display, filter_style),
                            Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                            Span::raw(format!("  ({} shown{} • {})", total, if filter.is_empty() { "" } else { " filtered" }, if self.show_all_sessions { "all folders" } else { "this folder" })),
                        ]));
                        lines.push(Line::from(Span::styled(
                            format!(" Chats — {} saved • {} msgs current • {}  • Enter resume • n new • f fork • r rename • a archive • d delete • p project/all • Esc close ", total, self.messages.len(), &self.session_id[..8.min(self.session_id.len())]),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                        lines.push(Line::from(Span::styled(" ───────────────────────────────────────────────────────", Style::default().fg(Color::DarkGray))));
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                        if filtered.is_empty() {
                            lines.push(Line::from(Span::styled(format!("No match for '{}'", self.session_filter), Style::default().fg(Color::Red))));
                        } else {


                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(5).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(6).max(3);
                        }
                        let cursor = self.session_cursor.min(total.saturating_sub(1));
                        let mut start = 0;
                        if total > visible {
                            if cursor >= visible {
                                start = cursor + 1 - visible;
                            }
                            if start + visible > total {
                                start = total - visible;
                            }
                        }
                        let end = (start + visible).min(total);
                        for (idx, sess) in filtered[start..end].iter().enumerate() {
                            let i = start + idx;
                            let is_selected = i == cursor;
                            let is_current = sess.id == self.session_id;
                            let style = if is_selected { Theme::selection() } else { Style::default() };
                            let star = if is_selected { "▶ " } else { "  " };
                            let cur = if is_current { "● " } else { "  " };
                            let dt = {
                                let age = now.saturating_sub(sess.updated_at);
                                if age < 60 { format!("{}s ago", age) }
                                else if age < 3600 { format!("{}m ago", age/60) }
                                else if age < 86400 { format!("{}h ago", age/3600) }
                                else { format!("{}d ago", age/86400) }
                            };
                            let title = sess.title.clone().unwrap_or_else(|| "(untitled)".into());
                            let title_short = if title.chars().count() > 28 { format!("{}…", title.chars().take(28).collect::<String>()) } else { title };
                            let model_short = sess.model.split('/').next_back().unwrap_or(&sess.model);
                            let parent_mark = if sess.parent_id.is_some() { "⑂ " } else { "" };
                            lines.push(Line::from(vec![
                                Span::raw(star),
                                Span::styled(format!("{cur}{} ", &sess.id[..8.min(sess.id.len())]), style),
                                Span::styled(format!("{:<14}", model_short), Style::default().fg(Color::Cyan)),
                                Span::styled(title_short.clone(), if is_selected { style } else { Style::default().fg(Color::White) }),
                                Span::raw(" "),
                                Span::styled(format!("{parent_mark}{dt}"), Style::default().fg(Color::DarkGray)),
                            ]));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(format!(" — {}/{} chats (showing {}-{})", end, total, start + 1, end), Style::default().fg(Color::DarkGray))));
                        }
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(" ↑/↓ • PgUp/PgDn page • Enter resume • n new • f fork • r rename • a archive • d delete • Esc close ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    }
                    }
                    Err(e) => {
                        lines.push(Line::from(Span::styled(format!("db error: {e}"), Style::default().fg(Color::Red))));
                    }
                }
                lines
            }
            Popup::ModelPicker => {
                let has_gemini = has_key("GEMINI_API_KEY");
                let has_openrouter = has_key("OPENROUTER_API_KEY");
                let has_gateway = has_key("OPENCODE_API_KEY") || has_key("ZEN_API_KEY");
                let mut lines: Vec<Line> = Vec::new();

                let open_dot = if has_openrouter { "●" } else { "○" };
                let gem_dot = if has_gemini { "●" } else { "○" };
                let gw_dot = if has_gateway { "●" } else { "○" };
                lines.push(Line::from(vec![
                    Span::styled(" Keys: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("OpenRouter {open_dot} "), Style::default().fg(if has_openrouter { Color::Green } else { Color::DarkGray })),
                    Span::styled(format!("Gemini {gem_dot} "), Style::default().fg(if has_gemini { Color::Green } else { Color::DarkGray })),
                    Span::styled(format!("Gateway {gw_dot} "), Style::default().fg(if has_gateway { Color::Green } else { Color::DarkGray })),
                    Span::styled("— /providers to add keys (free tier is public)", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                ]));
                lines.push(Line::from(""));

                let filter_display = if self.model_filter.is_empty() { "type to filter…".to_string() } else { self.model_filter.clone() };
                let filter_style = if self.model_filter.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) };
                lines.push(Line::from(vec![
                    Span::styled(" Filter: ", Style::default().fg(Color::Yellow)),
                    Span::styled(filter_display, filter_style),
                    Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("  ({} shown)", {
                        let f = self.model_filter.to_lowercase();
                        if f.is_empty() { self.available_models.len() } else { self.available_models.iter().filter(|m| m.to_lowercase().contains(&f)).count() }
                    })),
                ]));
                lines.push(Line::from(Span::styled(" ─────────────────────────────────", Style::default().fg(Color::DarkGray))));
                if self.available_models.is_empty() {
                    lines.push(Line::from(Span::styled("No models available — set a key and restart TUI", Style::default().fg(Color::Red))));
                } else {
                    let filter = self.model_filter.to_lowercase();
                    let filtered: Vec<(usize, &String)> = self.available_models.iter().enumerate().filter(|(_, m)| filter.is_empty() || m.to_lowercase().contains(&filter)).collect();
                    if filtered.is_empty() {
                        lines.push(Line::from(Span::styled(format!("No match for '{}'", self.model_filter), Style::default().fg(Color::Red))));
                    } else {



                        let total = filtered.len();
                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(6).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(7).max(3);
                        }
                        let cursor = self.model_cursor.min(total.saturating_sub(1));
                        let mut start = 0;
                        if total > visible {
                            if cursor >= visible {
                                start = cursor + 1 - visible;
                            }

                            if start + visible > total {
                                start = total - visible;
                            }
                        }
                        let end = (start + visible).min(total);
                        for (filtered_idx, (_, m)) in filtered[start..end].iter().enumerate() {
                            let actual_idx = start + filtered_idx;
                            let is_selected = actual_idx == cursor;
                            let style = if is_selected { Theme::selection() } else { Style::default() };
                            let prefix = if **m == self.model { "● " } else { "  " };
                            let star = if is_selected { "▶ " } else { "  " };
                            let provider = if m.starts_with("opencode/") {
                                "Gateway"
                            } else if m.starts_with("opencode-go/") {
                                "Gateway-Go"
                            } else if m.contains("gemini") || m.starts_with("google/") {
                                "Gemini"
                            } else if m.contains("Muse") || m.contains("anthropic") {
                                "Anthropic"
                            } else {
                                "OpenRouter"
                            };

                            let is_free = vioraharness_core::provider::opencode::is_free_model(m);
                            let needs_key = (m.starts_with("opencode/") || m.starts_with("opencode-go/")) && !is_free && !has_gateway;
                            let mut disp = format!("{prefix}{m}");
                            if needs_key {
                                disp = format!("{disp} (needs OPENCODE_API_KEY)");
                            }
                            let mut spans = vec![
                                Span::raw(star),
                                Span::styled(disp, if needs_key && !is_selected { Style::default().fg(Color::DarkGray) } else { style }),
                                Span::raw("  "),
                                Span::styled(format!("({provider})"), Style::default().fg(Color::DarkGray)),
                            ];

                            if is_free {
                                spans.push(Span::styled(" free", Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)));
                            }
                            lines.push(Line::from(spans));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(
                                format!(" — {}/{} models (↑/↓ PgUp/PgDn scroll, showing {}-{})", end, total, start + 1, end),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(" Type to search • ↑/↓ • PgUp/PgDn page • Enter select • Esc clear/close • Backspace delete", Style::default().fg(Color::DarkGray))));
                lines
            }
            Popup::Providers => {
                let catalog = provider_catalog();
                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(" Providers — select then paste API key", Style::default().add_modifier(Modifier::BOLD))));
                lines.push(Line::from(Span::styled(format!(" Config: {} (0600) + env var for current process — no restart needed after save", providers_env_path().display()), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                lines.push(Line::from(""));
                if self.provider_input_active {
                    if let Some(pid) = &self.provider_selected {
                        let prov = catalog.iter().find(|(id,_,_,_)| *id == pid.as_str());
                        let env_name = prov.map(|(_,_,env,_)| *env).unwrap_or("API_KEY");
                        let masked = "•".repeat(self.provider_key_input.len().min(24));
                        let shown = if self.provider_key_input.is_empty() { "paste key…".to_string() } else { masked };
                        let style = if self.provider_key_input.is_empty() { Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC) } else { Style::default().fg(Color::Yellow) };
                        lines.push(Line::from(vec![
                            Span::styled(format!(" {env_name}: "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                            Span::styled(shown, style),
                            Span::styled(" ▌", Style::default().fg(Color::Cyan)),
                        ]));
                        lines.push(Line::from(Span::styled(" Enter save • Esc cancel • Ctrl-V paste (masked) ", Style::default().fg(Color::DarkGray))));
                        if self.provider_validating {
                            lines.push(Line::from(Span::styled(" Validating…", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC))));
                        }
                        if let Some((msg, ok)) = &self.provider_msg {
                            let col = if *ok { Color::Green } else { Color::Red };
                            let icon = if *ok { "✓" } else { "✗" };
                            lines.push(Line::from(Span::styled(format!(" {icon} {msg}"), Style::default().fg(col).add_modifier(Modifier::BOLD))));
                        }
                    }
                } else {
                    for (i, (_pid, name, env_name, desc)) in catalog.iter().enumerate() {
                        let is_selected = i == self.provider_cursor;
                        let style = if is_selected { Theme::selection() } else { Style::default() };
                        let star = if is_selected { "▶ " } else { "  " };
                        let has = has_key(env_name);
                        let dot = if has { "●" } else { "○" };
                        let dot_col = if has { Color::Green } else { Color::DarkGray };
                        let preview = if has {
                            let v = std::env::var(env_name).unwrap_or_default();
                            let end = v.chars().rev().take(4).collect::<String>().chars().rev().collect::<String>();
                            format!("set …{end}")
                        } else { "missing".into() };
                        lines.push(Line::from(vec![
                            Span::raw(star),
                            Span::styled(format!("{dot} "), Style::default().fg(dot_col).add_modifier(Modifier::BOLD)),
                            Span::styled(format!("{name} "), style),
                            Span::styled(format!("({env_name}) "), Style::default().fg(Color::DarkGray)),
                            Span::styled(preview, Style::default().fg(if has { Color::Green } else { Color::Yellow })),
                            Span::raw("  "),
                            Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(" ↑/↓ select • Enter paste key • Esc close • Validates against the provider then writes to .env + PATH ", Style::default().fg(Color::DarkGray))));
                    if let Some((msg, ok)) = &self.provider_msg {
                        let col = if *ok { Color::Green } else { Color::Red };
                        let icon = if *ok { "✓" } else { "✗" };
                        lines.push(Line::from(Span::styled(format!(" {icon} {msg}"), Style::default().fg(col))));
                    }
                }
                lines
            }
            Popup::ThemePicker => {
                let current = crate::theme::ThemeConfig::load().theme;
                self.available_themes.iter().enumerate().map(|(i, name)| {
                    let style = if i == self.theme_cursor { Theme::selection() } else { Style::default() };
                    let prefix = if *name == current { "● " } else { "  " };
                    let star = if i == self.theme_cursor { "▶ " } else { "  " };
                    let desc = match name.as_str() {
                        "tokyonight" => "vivid blue — Tokyo Night",
                        "catppuccin" => "warm pastel — Catppuccin Mocha",
                        "dracula" => "vivid purple — Dracula",
                        "gruvbox" => "retro warm — Gruvbox",
                        "nord" => "arctic blue — Nord",
                        "system" => "adapts to terminal palette",
                        _ => "",
                    };
                    Line::from(vec![
                        Span::raw(star),
                        Span::styled(format!("{prefix}{name}"), style),
                        Span::raw("  "),
                        Span::styled(desc, Style::default().fg(Color::DarkGray)),
                    ])
                }).collect::<Vec<_>>()
            },
            Popup::Permissions => vec![
                Line::from(Span::styled("Permissions active (vioraharness.json)", Style::default().add_modifier(Modifier::BOLD))),
                Line::from("  read, glob, grep, schematic_query, symbol_search → allow"),
                Line::from("  netlist_run, task, schematic_render → allow"),
                Line::from("  write, bash → ask (auto-allow ls/pwd in headless, /tmp write)"),
                Line::from("  bash rm:* / sudo:* → deny"),
                Line::from(""),
                Line::from(Span::styled(format!("Thinking: {} ({} / {})", if self.show_thinking { "on" } else { "off" }, self.thinking_title, self.thinking_label), Style::default().fg(Color::Yellow))),
                Line::from("Press any key to close. Edit vioraharness.json to change."),
            ],
            Popup::Diff => {
                if let Some(diff) = &self.last_diff {
                    let mut lines = vec![Line::from(Span::styled(format!("Last write diff — {}", crate::widgets::diff::diff_summary(diff)), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)))];
                    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(Color::DarkGray))));
                    lines.extend(crate::widgets::diff::diff_lines(diff));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled("Press Esc to close • /undo to restore", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    lines
                } else {
                    vec![
                        Line::from(Span::styled("No diff yet", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))),
                        Line::from(""),
                        Line::from(Span::styled("Write a file first (write tool or agent turn), then /diff will show unified diff.", Style::default().fg(Color::DarkGray))),
                        Line::from(""),
                        Line::from(Span::styled("Tip: /undo restores last snapshot", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))),
                    ]
                }
            }
            Popup::PermissionAsk => {
                if let Some(req) = &self.pending_perm {
                    let mut lines = Vec::new();
                    lines.push(Line::from(Span::styled(
                        " Agent wants to run a tool that requires approval ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(vec![
                        Span::styled(" Tool: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(req.tool.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    ]));
                    let args_pretty = if req.args.len() > 200 {
                        format!("{}…", &req.args[..200])
                    } else {
                        req.args.clone()
                    };

                    let args_display = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&args_pretty) {
                        if let Some(cmd) = v.get("command").and_then(|c| c.as_str()) {
                            cmd.to_string()
                        } else if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                            format!("path={}", path)
                        } else {
                            args_pretty
                        }
                    } else {
                        args_pretty
                    };
                    let args_short = if args_display.len() > 80 { format!("{}…", &args_display[..80]) } else { args_display };
                    lines.push(Line::from(vec![
                        Span::styled(" Args: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(args_short, Style::default().fg(Color::White)),
                    ]));

                    let pat = if req.tool == "bash" {
                        let cmd = if let Ok(v) = serde_json::from_str::<serde_json::Value>(&req.args) {
                            v.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string()
                        } else { String::new() };
                        let first = cmd.split_whitespace().next().unwrap_or("bash").split('/').next_back().unwrap_or("bash");
                        format!("bash {first}*")
                    } else { req.tool.clone() };
                    lines.push(Line::from(vec![
                        Span::styled(" Save as: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("\"{pat}\": \"allow\""), Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC)),
                        Span::styled(" in vioraharness.json", Style::default().fg(Color::DarkGray)),
                    ]));
                    lines.push(Line::from(""));
                    let opts = ["Allow once", "Allow always (save)", "Deny"];
                    for (i, opt) in opts.iter().enumerate() {
                        let is_sel = i == self.perm_cursor;
                        let style = if is_sel { crate::theme::Theme::selection() } else { Style::default() };
                        let prefix = if is_sel { "▶ " } else { "  " };
                        let key_hint = match i { 0 => " (a)", 1 => " (A)", 2 => " (d)", _ => "" };
                        lines.push(Line::from(vec![
                            Span::raw(prefix),
                            Span::styled(format!("{opt}{key_hint}"), style),
                        ]));
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " ↑/↓ select • Enter confirm • a=allow once • A=allow always • d/n/Esc deny ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines
                } else {
                    vec![Line::from(Span::styled("No pending permission", Style::default().fg(Color::DarkGray)))]
                }
            }
            Popup::Question => {
                if let Some(req) = &self.pending_q {
                    if req.questions.is_empty() {
                        vec![Line::from(Span::styled(
                            "Invalid question (no questions)",
                            Style::default().fg(Color::Red),
                        ))]
                    } else {
                    let mut lines = Vec::new();
                    lines.push(Line::from(Span::styled(
                        " Agent asks — answer to continue ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )));
                    lines.push(Line::from(""));
                    let qi = self.q_answered.len().min(req.questions.len().saturating_sub(1));
                    let q = &req.questions[qi];
                    if !q.header.trim().is_empty() {
                        lines.push(Line::from(Span::styled(
                            q.header.clone(),
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("Q{}/{}: ", qi + 1, req.questions.len()),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            q.question.clone(),
                            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    if q.multi_select {
                        lines.push(Line::from(Span::styled(
                            " (multi-select: Space toggles, Enter confirms)",
                            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                        )));
                    }
                    lines.push(Line::from(""));

                    for (i, opt) in q.options.iter().enumerate() {
                        let is_sel = i == self.q_cursor && !self.q_custom_active;
                        let style = if is_sel { crate::theme::Theme::selection() } else { Style::default() };
                        let prefix = if is_sel { "▶ " } else { "  " };
                        let mark = if q.multi_select {
                            if self.q_toggled.contains(&i) { "[x] " } else { "[ ] " }
                        } else if is_sel {
                            "○ "
                        } else {
                            "  "
                        };
                        let mut row = vec![
                            Span::raw(prefix),
                            Span::styled(mark, Style::default().fg(Color::Cyan)),
                            Span::styled(opt.label.clone(), style),
                        ];
                        if !opt.description.trim().is_empty() {
                            row.push(Span::styled(
                                format!(" — {}", opt.description.trim()),
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        lines.push(Line::from(row));
                    }
                    let custom_idx = q.options.len();
                    let is_custom = self.q_cursor == custom_idx;
                    let cstyle = if is_custom && !self.q_custom_active {
                        crate::theme::Theme::selection()
                    } else {
                        Style::default()
                    };
                    let shown = if self.q_custom.is_empty() && !self.q_custom_active {
                        "Custom answer…".to_string()
                    } else {
                        format!("{}{}", self.q_custom, if self.q_custom_active { "▌" } else { "" })
                    };
                    lines.push(Line::from(vec![
                        Span::raw(if is_custom { "▶ " } else { "  " }),
                        Span::styled("✎ ", Style::default().fg(Color::Cyan)),
                        Span::styled(
                            shown,
                            if self.q_custom.is_empty() && !self.q_custom_active {
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)
                            } else {
                                cstyle
                            },
                        ),
                    ]));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        if self.q_custom_active {
                            " type answer • Enter submit • Esc back • Backspace delete "
                        } else {
                            " ↑/↓ move • Space/Enter select • type on Custom • Esc cancel "
                        },
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines
                    }
                } else {
                    vec![Line::from(Span::styled("No pending question", Style::default().fg(Color::DarkGray)))]
                }
            }
            Popup::ToolOutput => {
                if let Some((id, name, out, ok)) = &self.last_tool_output {
                    let mut lines = Vec::new();
                    let status = if *ok { "✓ ok" } else { "✗ error" };
                    let status_col = if *ok { Color::Green } else { Color::Red };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {name} "), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("{status} "), Style::default().fg(status_col).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("id={} ", &id[..8.min(id.len())]), Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{} chars, {} lines", out.len(), out.lines().count()), Style::default().fg(Color::DarkGray)),
                    ]));
                    lines.push(Line::from(Span::styled("─".repeat(50), Style::default().fg(Color::DarkGray))));
                    if out.is_empty() {
                        lines.push(Line::from(Span::styled("(empty output)", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    } else {
                        let all: Vec<&str> = out.lines().collect();
                        let total = all.len();


                        let inner_h = inner.height as usize;
                        let mut visible = inner_h.saturating_sub(4).max(3);
                        if total > visible {
                            visible = inner_h.saturating_sub(5).max(3);
                        }
                        let start = self.tool_output_scroll.min(total.saturating_sub(1));
                        let end = (start + visible).min(total);
                        for (i, line) in all[start..end].iter().enumerate() {
                            let line_no = start + i + 1;

                            let display = if line.len() > 100 { format!("{}…", &line[..100]) } else { (*line).to_string() };
                            lines.push(Line::from(vec![
                                Span::styled(format!("{:4} │ ", line_no), Style::default().fg(Color::DarkGray)),
                                Span::raw(display),
                            ]));
                        }
                        if total > visible {
                            lines.push(Line::from(Span::styled(
                                format!(" — {}/{} lines (↑/↓ PageUp/PageDown scroll, Esc close)", end, total),
                                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(" ↑/↓ • PgUp/PgDn scroll • Esc/q close • s save to /tmp ", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC))));
                    lines
                } else {
                    vec![Line::from(Span::styled("No tool output yet", Style::default().fg(Color::Yellow)))]
                }
            }
            Popup::Tasks => {
                use vioraharness_core::tools::tasks::{list_tasks, BgStatus};
                let all = list_tasks();
                let mut lines: Vec<Line> = Vec::new();
                if all.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No background tasks yet.",
                        Style::default().fg(Color::Yellow),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " Esc close ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                } else {
                let total = all.len();
                let now =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                let running = all.iter().filter(|t| t.status == BgStatus::Running).count();
                let failed = all
                    .iter()
                    .filter(|t| {
                        t.status == BgStatus::Error || t.status == BgStatus::Killed
                    })
                    .count();
                let done = total.saturating_sub(running).saturating_sub(failed);

                let mut head: Vec<Span> = vec![Span::styled(
                    format!(" {running} running "),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )];
                if done > 0 {
                    head.push(Span::styled(
                        format!("• {done} done "),
                        Style::default().fg(Color::Green),
                    ));
                }
                if failed > 0 {
                    head.push(Span::styled(
                        format!("• {failed} failed "),
                        Style::default().fg(Color::Red),
                    ));
                }
                head.push(Span::styled(
                    format!("• {total} tracked"),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::from(head));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<10}", "ID"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<9}", "STATUS"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "COMMAND",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                ]));
                let rule = "─".repeat((inner.width as usize).saturating_sub(2).max(10));
                lines.push(Line::from(Span::styled(
                    format!(" {rule}"),
                    Style::default().fg(Color::DarkGray),
                )));
                let inner_h = inner.height as usize;
                let mut visible = inner_h.saturating_sub(5).max(3);
                if total > visible {
                    visible = inner_h.saturating_sub(6).max(3);
                }
                let cursor = self.task_cursor.min(total.saturating_sub(1));
                let mut start = 0;
                if total > visible {
                    if cursor >= visible {
                        start = cursor + 1 - visible;
                    }
                    if start + visible > total {
                        start = total - visible;
                    }
                }
                let end = (start + visible).min(total);
                for (idx, t) in all[start..end].iter().enumerate() {
                    let i = start + idx;
                    let is_selected = i == cursor;
                    let style = if is_selected {
                        Theme::selection()
                    } else {
                        Style::default()
                    };
                    let (icon, col) = match t.status {
                        BgStatus::Running => ("◐", Color::Yellow),
                        BgStatus::Done => ("●", Color::Green),
                        BgStatus::Error => ("✖", Color::Red),
                        BgStatus::Killed => ("○", Color::DarkGray),
                    };
                    let status_word = match t.status {
                        BgStatus::Running => "running",
                        BgStatus::Done => "done",
                        BgStatus::Error => "error",
                        BgStatus::Killed => "killed",
                    };
                    let star = if is_selected { "▶ " } else { "  " };
                    let age = now.saturating_sub(t.started_at);
                    let age_s = if age < 60 {
                        format!("{}s", age)
                    } else if age < 3600 {
                        format!("{}m", age / 60)
                    } else {
                        format!("{}h", age / 3600)
                    };
                    let (exit_s, exit_col) = match (t.status, t.exit_code) {
                        (BgStatus::Done, _) => ("exit 0".to_string(), Color::Green),
                        (_, Some(c)) => (format!("exit {c}"), Color::Red),
                        _ => ("—".to_string(), Color::DarkGray),
                    };



                    let cmd_cap = (inner.width as usize).saturating_sub(42).max(12);
                    let cmd_raw =
                        t.command.split_whitespace().collect::<Vec<_>>().join(" ");
                    let cmd = if cmd_raw.chars().count() > cmd_cap {
                        format!(
                            "{}…",
                            cmd_raw.chars().take(cmd_cap.saturating_sub(1)).collect::<String>()
                        )
                    } else {
                        cmd_raw
                    };
                    lines.push(Line::from(vec![
                        Span::raw(star),
                        Span::styled(
                            format!("{icon} {} ", short_task_id(&t.id)),
                            Style::default().fg(col).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{status_word:<9}"),
                            Style::default().fg(col),
                        ),
                        Span::styled(
                            format!("{cmd:<cmd_cap$}"),
                            if is_selected {
                                style
                            } else {
                                Style::default().fg(Color::White)
                            },
                        ),
                        Span::styled(format!(" {exit_s:<8}"), Style::default().fg(exit_col)),
                        Span::styled(age_s, Style::default().fg(Color::DarkGray)),
                    ]));
                }
                if total > visible {
                    lines.push(Line::from(Span::styled(
                        format!(" — {}/{} tasks (showing {}-{})", end, total, start + 1, end),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " ↑/↓ navigate • Enter log • k kill • Esc close ",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                }
                lines
            }
            Popup::Errors => {
                use vioraharness_core::observe::list_errors;
                let all = list_errors();
                let mut lines: Vec<Line> = Vec::new();
                if all.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No errors recorded.",
                        Style::default().fg(Color::Green),
                    )));
                    lines.push(Line::from(Span::styled(
                        "Turn and command failures land here with full text.",
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " Esc close ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                } else {
                let total = all.len();
                let now =
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(
                            " {total} error{s} ",
                            s = if total == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "newest first",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<10}", "ID"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{:<9}", "SOURCE"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "SUMMARY",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                ]));
                let rule = "─".repeat((inner.width as usize).saturating_sub(2).max(10));
                lines.push(Line::from(Span::styled(
                    format!(" {rule}"),
                    Style::default().fg(Color::DarkGray),
                )));
                let inner_h = inner.height as usize;
                let mut visible = inner_h.saturating_sub(5).max(3);
                if total > visible {
                    visible = inner_h.saturating_sub(6).max(3);
                }
                let cursor = self.error_cursor.min(total.saturating_sub(1));
                let mut start = 0;
                if total > visible {
                    if cursor >= visible {
                        start = cursor + 1 - visible;
                    }
                    if start + visible > total {
                        start = total - visible;
                    }
                }
                let end = (start + visible).min(total);
                let sum_cap = (inner.width as usize).saturating_sub(42).max(12);
                for (idx, e) in all[start..end].iter().enumerate() {
                    let i = start + idx;
                    let is_selected = i == cursor;
                    let style = if is_selected {
                        Theme::selection()
                    } else {
                        Style::default()
                    };
                    let star = if is_selected { "▶ " } else { "  " };
                    let age = now.saturating_sub(e.at);
                    let age_s = if age < 60 {
                        format!("{}s", age)
                    } else if age < 3600 {
                        format!("{}m", age / 60)
                    } else {
                        format!("{}h", age / 3600)
                    };
                    let summary = if e.summary.chars().count() > sum_cap {
                        format!("{}…", e.summary.chars().take(sum_cap.saturating_sub(1)).collect::<String>())
                    } else {
                        e.summary.clone()
                    };
                    lines.push(Line::from(vec![
                        Span::raw(star),
                        Span::styled(
                            format!("✖ {} ", short_err_id(&e.id)),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{:<9}", e.source.chars().take(9).collect::<String>()),
                            Style::default().fg(Color::Yellow),
                        ),
                        Span::styled(
                            format!("{summary:<sum_cap$}"),
                            if is_selected {
                                style
                            } else {
                                Style::default().fg(Color::White)
                            },
                        ),
                        Span::styled(format!(" {age_s}"), Style::default().fg(Color::DarkGray)),
                    ]));
                }
                if total > visible {
                    lines.push(Line::from(Span::styled(
                        format!(" — {}/{} errors (showing {}-{})", end, total, start + 1, end),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " ↑/↓ navigate • Enter full text • c clear • Esc close ",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                } // else (non-empty list)
                lines
            }
            Popup::Rewind => {
                let points = rewind_checkpoints(&self.session_id);
                let mut lines: Vec<Line> = Vec::new();
                if points.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No messages in this chat yet — nothing to rewind to.",
                        Style::default().fg(Color::Green),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " Esc close ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                } else {
                let total = points.len();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(
                            " {total} checkpoint{s} ",
                            s = if total == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "one per message you sent — files return to the selected checkpoint",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<7}", "#MSG"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "YOUR MESSAGE",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                    ),
                ]));
                let rule = "─".repeat((inner.width as usize).saturating_sub(2).max(10));
                lines.push(Line::from(Span::styled(
                    format!(" {rule}"),
                    Style::default().fg(Color::DarkGray),
                )));
                let inner_h = inner.height as usize;
                let mut visible = inner_h.saturating_sub(6).max(3);
                if total > visible {
                    visible = inner_h.saturating_sub(7).max(3);
                }
                let cursor = self.rewind_cursor.min(total.saturating_sub(1));
                let mut start = 0;
                if total > visible {
                    if cursor >= visible {
                        start = cursor + 1 - visible;
                    }
                    if start + visible > total {
                        start = total - visible;
                    }
                }
                let end = (start + visible).min(total);
                let prev_cap = (inner.width as usize).saturating_sub(30).max(12);
                for (idx, p) in points[start..end].iter().enumerate() {
                    let i = start + idx;
                    let is_selected = i == cursor;
                    let style = if is_selected {
                        Theme::selection()
                    } else {
                        Style::default()
                    };
                    let star = if is_selected { "▶ " } else { "  " };
                    let armed = self.rewind_armed == Some(p.target_seq);
                    let prev = if p.preview.chars().count() > prev_cap {
                        format!("{}…", p.preview.chars().take(prev_cap.saturating_sub(1)).collect::<String>())
                    } else {
                        p.preview.clone()
                    };
                    let snap_tag = if p.snap_count > 0 {
                        format!(" ◆{}", p.snap_count)
                    } else {
                        String::new()
                    };
                    lines.push(Line::from(vec![
                        Span::raw(star),
                        Span::styled(
                            format!("{:<7}", format!("#{}", p.user_seq)),
                            if armed {
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Yellow)
                            },
                        ),
                        Span::styled(
                            format!("{prev}{snap_tag}"),
                            if is_selected {
                                style
                            } else {
                                Style::default().fg(Color::White)
                            },
                        ),
                    ]));
                }
                if total > visible {
                    lines.push(Line::from(Span::styled(
                        format!(" — {}/{} checkpoints (showing {}-{})", end, total, start + 1, end),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                lines.push(Line::from(""));
                if let Some(seq) = self.rewind_armed {
                    let detail = self
                        .rewind_armed_note
                        .as_deref()
                        .unwrap_or("restore checkpoint");
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!(" ARMED at #{seq} ({detail}) — Enter again, Esc cancels "),
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    lines.push(Line::from(""));
                    lines.push(Self::hint_bar(&[
                        ("enter", "Confirm rewind"),
                        ("esc", "Cancel"),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(
                        " ↑/↓ select checkpoint • Enter arm • Enter again restores • Esc close ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines.push(Line::from(Span::styled(
                        " ◆N = file snapshots at that message • later messages are dropped ",
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                    lines.push(Line::from(""));
                    lines.push(Self::hint_bar(&[
                        ("↑/↓", "Navigate"),
                        ("enter", "Select"),
                        ("esc", "Cancel"),
                    ]));
                }
                } // else (has checkpoints)
                lines
            }
            Popup::None => vec![],
        };
        let para = Paragraph::new(content)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        frame.render_widget(para, inner);
    }
}
