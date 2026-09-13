use super::super::*;
use crate::theme::Theme;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

impl App {
    pub(crate) fn hint_bar(hints: &[(&str, &str)]) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for (i, (key, label)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(Span::styled(
                format!(" {key} "),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Line::from(spans)
    }

    pub(crate) fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let spinner = if self.busy {
            const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[self.tick % FRAMES.len()]
        } else {
            "○"
        };
        let status_style = if self.busy {
            Theme::header_status_busy()
        } else if self.status == "error" {
            Theme::header_status_error()
        } else {
            Theme::header_status_ready()
        };
        let mode_style = if self.mode == "plan" {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        };

        let busy_button = if self.busy {
            if let Some((_, name, preview, start)) = &self.running_tool {
                let secs = start.elapsed().as_secs();
                let label = if name == "bash" {
                    let short = truncate_chars(preview, 30);
                    if secs > 0 {
                        format!(" {spinner} BUSY: bash {short} ({}s) ", secs)
                    } else {
                        format!(" {spinner} BUSY: bash {short} ")
                    }
                } else {
                    if secs > 0 {
                        format!(" {spinner} BUSY: {name} ({}s) ", secs)
                    } else {
                        format!(" {spinner} BUSY: {name} ")
                    }
                };
                Some((label, Theme::busy_button_shell()))
            } else {
                Some((format!(" {spinner} BUSY "), Theme::busy_button()))
            }
        } else {
            None
        };

        let mut header_spans = vec![
            Span::styled(" VioraHarness ", Theme::header_title()),
            Span::styled(format!("[{}] ", self.mode.to_uppercase()), mode_style),
        ];
        if let Some((label, style)) = busy_button {
            header_spans.push(Span::styled(label, style));

            header_spans.push(Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            header_spans.push(Span::styled(format!("{} ", self.status), status_style));
            header_spans.push(Span::styled(
                format!("{spinner} {}", if self.busy { "busy" } else { "idle" }),
                status_style,
            ));
        }
        let header = Paragraph::new(Line::from({
            let mut v = header_spans;
            let nerr = vioraharness_core::observe::error_count();
            if nerr > 0 {
                v.push(Span::styled(
                    format!("✖ {nerr} "),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
            v.push(Span::styled(
                {
                    let total_chars: usize = self
                        .messages
                        .iter()
                        .map(|m| {
                            let mut n = m.content.len();
                            for it in &m.items {
                                match it {
                                    Content::ToolCall { args, .. } => n += args.len(),
                                    Content::ToolResult { content, .. } => n += content.len(),
                                    Content::Reasoning(r) => n += r.len(),
                                    _ => {}
                                }
                            }
                            if let Some(r) = &m.reasoning {
                                n += r.len();
                            }
                            n
                        })
                        .sum::<usize>()
                        + self.input.text.len()
                        + self.thinking_buf.len()
                        + self.streaming_buf.len();
                    let tokens = (total_chars / 4).saturating_sub(self.ctx_freed_tokens);
                    let ctx_len = *self.model_context.get(&self.model).unwrap_or(&128_000);
                    let ctx_k = if ctx_len >= 1000 {
                        format!("{}k", ctx_len / 1000)
                    } else {
                        ctx_len.to_string()
                    };
                    let used = if tokens >= 1000 {
                        let k = tokens as f32 / 1000.0;
                        let s = format!("{k:.1}k");
                        s.replace(".0k", "k")
                    } else {
                        tokens.to_string()
                    };
                    let pct = ((tokens as f32 / ctx_len as f32) * 100.0) as usize;
                    let pct = pct.min(100);
                    let bar = format!("{}{}", "▓".repeat(pct / 10), "░".repeat(10 - pct / 10));
                    format!("| {used}/{ctx_k} {pct}% {bar} ")
                },
                Style::default().fg(Color::DarkGray),
            ));
            if let Some(q) = &self.chat_search {
                v.push(Span::styled(
                    format!(" [{}] ", q),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            v
        }))
        .style(Theme::header_bg());
        frame.render_widget(header, area);
    }

    pub(crate) fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let model_short = self.model.split('/').next_back().unwrap_or(&self.model);
        let title = if self.busy {
            format!(
                " Input — {} [{}] (busy — Enter queues • $… ⚡ current turn • Esc cancels) ",
                model_short,
                self.mode.to_uppercase()
            )
        } else if let Some(img) = &self.pending_image {
            format!(
                " Input — {} [{}] • [image {} pending] • Enter send • Esc clear ",
                model_short,
                self.mode.to_uppercase(),
                img.label
            )
        } else {
            format!(
                " Input — {} [{}] • Enter send • Tab → {} • ↑/↓ history ",
                model_short,
                self.mode.to_uppercase(),
                if self.mode == "plan" { "BUILD" } else { "PLAN" }
            )
        };
        let display_text = if self.input.text.is_empty() && !self.busy {
            "Type a prompt or /help for commands…"
        } else if self.input.text.is_empty() {
            "Type ahead — Enter queues, Esc cancels the turn…"
        } else {
            self.input.text.as_str()
        };
        let style = if self.input.text.is_empty() && !self.busy {
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC)
        } else if self.busy {
            Theme::input_border_busy()
        } else {
            Style::default()
        };
        let input_line = if self.input.text.is_empty() {
            Line::from(Span::styled(display_text, style))
        } else if let Some((lo, hi)) = self.input.selected_range() {
            let before = &self.input.text[..lo];
            let mid = &self.input.text[lo..hi];
            let after = &self.input.text[hi..];
            let mut spans = vec![Span::styled(before.to_string(), style)];
            spans.push(Span::styled(mid.to_string(), Theme::selection()));
            spans.push(Span::styled(after.to_string(), style));
            Line::from(spans)
        } else {
            Line::from(Span::styled(display_text, style))
        };
        let input = Paragraph::new(input_line)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .style(Theme::panel_bg())
                    .border_style(if self.busy {
                        Theme::input_border_busy()
                    } else {
                        Theme::input_border()
                    }),
            )
            .wrap(Wrap { trim: false })
            .style(Theme::panel_bg().patch(style));
        frame.render_widget(input, area);

        let cursor_x = area.x + 1 + self.input.cursor as u16;
        let cursor_y = area.y + 1;
        let max_x = area.x.saturating_add(area.width.saturating_sub(2));
        frame.set_cursor_position((cursor_x.min(max_x), cursor_y));
    }

    pub(crate) fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".into());
        let rel = rel_path(&cwd);

        let tasks = vioraharness_core::tools::tasks::list_tasks();
        let running = tasks
            .iter()
            .filter(|t| t.status == vioraharness_core::tools::tasks::BgStatus::Running)
            .count();
        let total = tasks.len();
        let queued = self.queued_prompts.len();
        let instant = self
            .instant_injector
            .as_ref()
            .map(|inj| inj.len())
            .unwrap_or(0);
        let mut suffix: Vec<Span> = Vec::new();
        if total > 0 {
            suffix.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            if running > 0 {
                suffix.push(Span::styled(
                    format!("◐ {running} running "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            suffix.push(Span::styled(
                if total == 1 {
                    "1 task".to_string()
                } else {
                    format!("{total} tasks")
                },
                Style::default().fg(if running > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                }),
            ));
            suffix.push(Span::styled(
                " [/tasks]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        if queued > 0 {
            suffix.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            suffix.push(Span::styled(
                if queued == 1 {
                    "▸1 queued".to_string()
                } else {
                    format!("▸{queued} queued")
                },
                Style::default().fg(Color::Cyan),
            ));
        }
        if instant > 0 {
            // `»` (not ⚡): footer width math counts chars, and ⚡ renders
            // double-width on some terminals.
            suffix.push(Span::styled("  •  ", Style::default().fg(Color::DarkGray)));
            suffix.push(Span::styled(
                if instant == 1 {
                    "»1 instant".to_string()
                } else {
                    format!("»{instant} instant")
                },
                Style::default().fg(Color::Cyan),
            ));
        }
        let suffix_len: usize = suffix.iter().map(|s| s.content.chars().count()).sum();
        let max_cwd = (area.width as usize).saturating_sub(suffix_len + 5).max(8);
        let display = if rel.chars().count() > max_cwd {
            format!(
                "…{}",
                rel.chars()
                    .skip(rel.chars().count() - max_cwd)
                    .collect::<String>()
            )
        } else {
            rel
        };

        let mut spans = vec![
            Span::styled(" ", Style::default()),
            Span::styled(display, Style::default().fg(Color::DarkGray)),
        ];
        spans.extend(suffix);
        let footer = Paragraph::new(Line::from(spans))
            .style(Theme::footer())
            .alignment(Alignment::Left);
        frame.render_widget(footer, area);
    }

    pub(crate) fn draw_completions(&self, frame: &mut Frame, input_area: Rect) {
        let comps = self.input.slash_completions();
        if comps.is_empty() {
            return;
        }
        let height = (comps.len() as u16 + 2).min(8);
        let popup_area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width: 40.min(input_area.width),
            height,
        };
        frame.render_widget(Clear, popup_area);
        let items: Vec<ListItem> = comps
            .iter()
            .map(|(c, d)| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        *c,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(*d, Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" completions ")
                .border_style(Theme::popup_border()),
        );
        frame.render_widget(list, popup_area);
    }
}
