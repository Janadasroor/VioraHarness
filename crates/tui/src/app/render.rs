use super::popups::rewind_checkpoints;
use super::*;
use crate::theme::Theme;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

impl App {
    /// Bottom hint bar with highlighted key chips:
    /// `[↑/↓] Navigate   [enter] Select   [esc] Cancel`.
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

    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(6),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        self.chat_area = chunks[1];
        self.input_area = chunks[2];
        self.draw_header(frame, chunks[0]);
        self.draw_chat(frame, chunks[1]);
        self.draw_input(frame, chunks[2]);
        self.draw_footer(frame, chunks[3]);

        if self.popup != Popup::None {
            self.draw_popup(frame, area);
        } else if self.input.text.starts_with('/') {
            self.draw_completions(frame, chunks[2]);
        }
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
            v.push(Span::raw(format!(" | {} msgs ", self.messages.len())));
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

    pub(crate) fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
        let mut all_lines: Vec<Line> = Vec::new();
        for (abs_idx, m) in self.messages.iter().enumerate() {
            let (prefix, style) = match m.role.as_str() {
                "user" => ("> ", Theme::user_prefix()),
                "assistant" => ("● ", Theme::assistant_prefix()),
                "system" => ("· ", Theme::system_prefix()),
                _ => ("  ", Style::default()),
            };

            let raw = if m.content.chars().count() > 20000 {
                let mut cut = truncate_chars(&m.content, 20000);

                if cut.lines().filter(|l| l.trim().starts_with("```")).count() % 2 == 1 {
                    cut.push_str("\n```");
                }
                format!(
                    "{cut}… ({} chars total, truncated at 20k — scroll or /export)",
                    m.content.chars().count()
                )
            } else {
                m.content.clone()
            };

            let (raw, collapsed_total) = if m.role == "user" {
                collapse_long_content(&raw, self.expanded_messages.contains(&(abs_idx, None)))
            } else {
                (raw, None)
            };
            let time = Span::styled(
                format!(" {} ", m.timestamp),
                Style::default().fg(Color::DarkGray),
            );
            if m.role == "assistant" {
                let mut in_code_block = false;
                let raw_lines: Vec<&str> = raw.lines().collect();

                let joined = join_math_lines(&raw_lines);
                let raw_lines: Vec<&str> = joined.iter().map(|s| s.as_str()).collect();

                if raw_lines.is_empty() {
                    let mut spans = markdown_inline_spans(&raw);
                    if let Some(q) = &self.chat_search {
                        if !q.is_empty() && raw.to_lowercase().contains(&q.to_lowercase()) {
                            spans = vec![Span::styled(
                                raw.clone(),
                                Style::default()
                                    .fg(Color::Yellow)
                                    .bg(Color::Rgb(50, 50, 30))
                                    .add_modifier(Modifier::BOLD),
                            )];
                        }
                    }
                    let mut line_spans = Vec::new();
                    line_spans.push(Span::styled(prefix, style));
                    line_spans.extend(spans);
                    line_spans.push(time);
                    if self.busy && !self.streaming_buf.is_empty() {
                        line_spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    }
                    all_lines.push(Line::from(line_spans));
                } else {
                    let table_max_width = (area.width as usize).saturating_sub(4).max(20);
                    let mut skip_until = 0;
                    for (li, line_str) in raw_lines.iter().enumerate() {
                        if li < skip_until {
                            continue;
                        }
                        let trimmed = line_str.trim();

                        if trimmed.starts_with("```") {
                            let lang = code_lang_from_fence(trimmed);
                            let entering = !in_code_block;
                            in_code_block = !in_code_block;
                            if entering {
                                let mut spans =
                                    vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                                spans.extend(fence_open_spans(&lang));
                                if li == 0 {
                                    spans.push(time.clone());
                                }
                                all_lines.push(Line::from(spans));
                            } else {
                                let mut spans = vec![Span::styled("  ", style)];
                                spans.extend(fence_close_spans());

                                all_lines.push(Line::from(spans));
                            }
                            continue;
                        }

                        if !in_code_block
                            && (trimmed.starts_with("$$") || trimmed.starts_with("\\["))
                        {
                            if let Some((mrows, consumed)) =
                                render_math_block(&raw_lines[li..], table_max_width)
                            {
                                for (mi, mrow) in mrows.into_iter().enumerate() {
                                    let mut spans = vec![Span::styled(
                                        if li == 0 && mi == 0 { prefix } else { "  " },
                                        style,
                                    )];
                                    spans.extend(mrow);
                                    if li == 0 && mi == 0 {
                                        spans.push(time.clone());
                                    }
                                    all_lines.push(Line::from(spans));
                                }
                                skip_until = li + consumed;
                                continue;
                            }
                        }

                        if !in_code_block && trimmed.contains('|') {
                            if let Some((tbl, consumed)) =
                                render_table_block(&raw_lines[li..], table_max_width)
                            {
                                for (ti, tline) in tbl.into_iter().enumerate() {
                                    let mut spans = vec![Span::styled(
                                        if li == 0 && ti == 0 { prefix } else { "  " },
                                        style,
                                    )];
                                    spans.extend(tline);
                                    if li == 0 && ti == 0 {
                                        spans.push(time.clone());
                                    }
                                    all_lines.push(Line::from(spans));
                                }
                                skip_until = li + consumed;
                                continue;
                            }
                        }
                        if in_code_block {
                            let mut line_spans = Vec::new();
                            line_spans
                                .push(Span::styled(if li == 0 { prefix } else { "  " }, style));
                            line_spans.push(Span::styled("│ ", crate::theme::Theme::code_block()));
                            line_spans.extend(highlighted_code_spans(line_str));

                            if li == 0 {
                                line_spans.push(time.clone());
                            }
                            all_lines.push(Line::from(line_spans));
                            continue;
                        }
                        if trimmed.is_empty() {
                            let head =
                                vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                            let tail = if li == 0 { vec![time.clone()] } else { vec![] };
                            all_lines.push(chat_line(head, Vec::new(), tail));
                            continue;
                        }

                        if trimmed.starts_with('#') {
                            let level = trimmed.chars().take_while(|&c| c == '#').count();
                            let header_text = trimmed[level..].trim_start();
                            let hstyle = markdown_header_style(level.min(3));
                            let text_width = (area.width as usize).saturating_sub(2).max(20);
                            let gutter: Vec<Span> =
                                vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                            let mut tail: Vec<Span> = Vec::new();
                            if li == 0 {
                                tail.push(time.clone());
                            }

                            if li == 0 && self.busy && !self.streaming_buf.is_empty() {
                                tail.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                            }

                            if let Some(q) = &self.chat_search {
                                if !q.is_empty()
                                    && line_str.to_lowercase().contains(&q.to_lowercase())
                                {
                                    let hl = Style::default()
                                        .fg(Color::Yellow)
                                        .bg(Color::Rgb(50, 50, 30))
                                        .add_modifier(Modifier::BOLD);
                                    let mut spans = gutter;
                                    spans.push(Span::styled((*line_str).to_string(), hl));
                                    spans.extend(tail);
                                    all_lines.push(Line::from(spans));
                                    continue;
                                }
                            }

                            let patch = |s: Span<'static>| Span {
                                content: s.content,
                                style: hstyle.patch(s.style),
                            };
                            let segs: Vec<Span> = match split_list_marker(header_text) {
                                Some((marker, rest)) => {
                                    let mut v = vec![Span::styled(
                                        format!("{marker} "),
                                        hstyle.patch(Theme::title_marker()),
                                    )];
                                    v.extend(markdown_inline_spans(&rest).into_iter().map(patch));
                                    v
                                }
                                None => markdown_inline_spans(header_text)
                                    .into_iter()
                                    .map(patch)
                                    .collect(),
                            };
                            all_lines.extend(wrap_spans(
                                segs,
                                gutter,
                                vec![Span::raw("  ")],
                                tail,
                                text_width,
                            ));
                            continue;
                        }

                        let text_width = (area.width as usize).saturating_sub(2).max(20);
                        let gutter: Vec<Span> =
                            vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                        let mut tail: Vec<Span> = Vec::new();
                        if li == 0 {
                            tail.push(time.clone());
                        }
                        if li == 0 && self.busy && !self.streaming_buf.is_empty() {
                            tail.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                        }
                        let content = *line_str;

                        if let Some(q) = &self.chat_search {
                            if !q.is_empty() && content.to_lowercase().contains(&q.to_lowercase()) {
                                let mut spans = gutter;
                                spans.push(Span::styled(
                                    content.to_string(),
                                    Style::default()
                                        .fg(Color::Yellow)
                                        .bg(Color::Rgb(50, 50, 30))
                                        .add_modifier(Modifier::BOLD),
                                ));
                                spans.extend(tail);
                                all_lines.push(Line::from(spans));
                                continue;
                            }
                        }

                        if let Some((marker, rest)) = split_list_marker(trimmed) {
                            let nest = line_str
                                .chars()
                                .take_while(|c| *c == ' ' || *c == '\t')
                                .map(|c| if c == '\t' { 4 } else { 1 })
                                .sum::<usize>();
                            let title = is_section_title(&rest);
                            let marker_style = if title {
                                Theme::title_marker()
                            } else {
                                Theme::list_marker()
                            };
                            let mut segs = vec![Span::styled(format!("{marker} "), marker_style)];
                            segs.extend(markdown_inline_spans(&rest));

                            let hang = 2 + nest + marker.width() + 1;
                            let cont: Vec<Span> =
                                vec![Span::raw(" ".repeat(hang.min(text_width - 1)))];

                            let mut head = gutter;
                            if nest > 0 {
                                head.push(Span::raw(" ".repeat(nest.min(text_width - 1))));
                            }
                            all_lines.extend(wrap_spans(segs, head, cont, tail, text_width));
                            continue;
                        }
                        let inner_spans = markdown_inline_spans(content);
                        all_lines.extend(wrap_spans(
                            inner_spans,
                            gutter,
                            vec![Span::raw("  ")],
                            tail,
                            text_width,
                        ));
                    }

                    if in_code_block {
                        let mut spans = vec![Span::styled("  ", style)];
                        spans.extend(fence_close_spans());
                        all_lines.push(Line::from(spans));
                    }
                }
            } else {
                let mut display = raw.lines().next().unwrap_or("").to_string();
                let rest: String = raw.lines().skip(1).collect::<Vec<_>>().join(" ");
                if !rest.is_empty() {
                    if display.is_empty() {
                        display = rest;
                    } else {
                        display = format!("{display} {rest}");
                    }
                }
                let is_error_msg = m.role == "system"
                    && (display.to_lowercase().contains("error")
                        || display.contains("✖")
                        || display.contains("denied")
                        || display.contains("blocked")
                        || display.contains("FILE NOT CREATED")
                        || display.contains("failed"));
                let is_code = display.trim_start().starts_with("```") || raw.contains("```");
                let mut content_span = if is_code {
                    Span::styled(
                        display.clone(),
                        Style::default()
                            .bg(Color::Rgb(49, 50, 68))
                            .fg(Color::Rgb(205, 214, 244)),
                    )
                } else if is_error_msg {
                    Span::styled(display.clone(), crate::theme::Theme::error_message())
                } else {
                    Span::raw(display.clone())
                };
                if let Some(q) = &self.chat_search {
                    if !q.is_empty() && display.to_lowercase().contains(&q.to_lowercase()) {
                        content_span = Span::styled(
                            display.clone(),
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::Rgb(50, 50, 30))
                                .add_modifier(Modifier::BOLD),
                        );
                    }
                }
                let effective_prefix = if is_error_msg { "✖ " } else { prefix };
                let effective_style = if is_error_msg {
                    crate::theme::Theme::error_prefix()
                } else {
                    style
                };
                let line = Line::from(vec![
                    Span::styled(effective_prefix, effective_style),
                    content_span,
                    time,
                ]);
                all_lines.push(line);
            }

            let tool_names: std::collections::HashMap<&str, &str> = m
                .items
                .iter()
                .filter_map(|it| {
                    if let Content::ToolCall { id, name, .. } = it {
                        Some((id.as_str(), name.as_str()))
                    } else {
                        None
                    }
                })
                .collect();
            for (ii, it) in m.items.iter().enumerate() {
                match it {
                    Content::ToolCall {
                        id,
                        name,
                        args,
                        status,
                    } => {
                        let verbosity = self.tool_verbosity(name);
                        if verbosity == ToolVerbosity::Hidden {
                            continue;
                        }
                        let icon = match status {
                            ToolStatus::Pending => "○",
                            ToolStatus::Running => "◐",
                            ToolStatus::Done => "●",
                            ToolStatus::Error => "✖",
                        };
                        let col = match status {
                            ToolStatus::Done => Color::Green,
                            ToolStatus::Error => Color::Red,
                            ToolStatus::Running => Color::Yellow,
                            _ => Color::DarkGray,
                        };
                        let preview = if verbosity == ToolVerbosity::Quiet {
                            String::new()
                        } else {
                            pretty_tool_args_wide(name, args, verbosity == ToolVerbosity::Full)
                        };

                        let elapsed_suffix = if *status == ToolStatus::Running {
                            if let Some((rid, _, _, start)) = &self.running_tool {
                                let is_running = rid == id;
                                if is_running || *name == "bash" {
                                    let secs = start.elapsed().as_secs();
                                    if secs > 0 {
                                        format!(" ({}s)", secs)
                                    } else {
                                        " (running)".to_string()
                                    }
                                } else {
                                    String::new()
                                }
                            } else if *name == "bash" {
                                " (running)".to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        };
                        let preview_with_elapsed = if elapsed_suffix.is_empty() {
                            preview
                        } else {
                            format!("{preview}{elapsed_suffix}")
                        };
                        let preview_style = if *status == ToolStatus::Error {
                            crate::theme::Theme::error_message()
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        let tline = Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                format!("{icon} {name} "),
                                Style::default().fg(col).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(preview_with_elapsed, preview_style),
                        ]);
                        all_lines.push(tline);
                    }
                    Content::ToolResult { id, content, ok } => {
                        let icon = if *ok { "✔" } else { "✖" };
                        let col = if *ok { Color::Green } else { Color::Red };

                        let tname = tool_names.get(id.as_str()).copied().unwrap_or("");
                        let Some(preview) = self.tool_result_preview(&m.items, id, content, *ok)
                        else {
                            continue;
                        };
                        let preview_style = if *ok {
                            Style::default().fg(Color::White)
                        } else {
                            crate::theme::Theme::error_message()
                        };

                        let label: String = if tname.is_empty() {
                            id.chars().take(12).collect()
                        } else {
                            tname.to_string()
                        };
                        let head = format!("{icon} {label} ");
                        let expanded = self.expanded_messages.contains(&(abs_idx, Some(ii)));
                        let rows: Vec<&str> = preview.lines().collect();
                        let show_all = expanded || rows.len() <= CHAT_COLLAPSE_LINES;
                        let shown = if show_all {
                            rows.len()
                        } else {
                            CHAT_COLLAPSE_HEAD
                        };
                        for (ri, row) in rows.iter().take(shown).enumerate() {
                            if ri == 0 {
                                all_lines.push(Line::from(vec![
                                    Span::raw("      "),
                                    Span::styled(
                                        head.clone(),
                                        Style::default().fg(col).add_modifier(Modifier::BOLD),
                                    ),
                                    Span::styled((*row).to_string(), preview_style),
                                ]));
                            } else {
                                all_lines.push(Line::from(vec![
                                    Span::raw(format!("      {:w$}", "", w = head.chars().count())),
                                    Span::styled((*row).to_string(), preview_style),
                                ]));
                            }
                        }
                        if !show_all {
                            all_lines.push(Line::from(vec![
                                Span::raw(format!("      {:w$}", "", w = head.chars().count())),
                                Span::styled(
                                    format!(
                                        "… (+{} more lines — Ctrl+X to expand)",
                                        rows.len() - shown
                                    ),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ]));
                        }

                        let diff_verbosity = {
                            let v = self.tool_verbosity(tname);
                            if !ok && v != ToolVerbosity::Full {
                                ToolVerbosity::Compact
                            } else {
                                v
                            }
                        };
                        if (tname == "write" || tname == "edit")
                            && *ok
                            && (diff_verbosity == ToolVerbosity::Compact
                                || diff_verbosity == ToolVerbosity::Full)
                        {
                            if let Some(d) = result_diff_text(content) {
                                let (changes, rest) = diff_card_lines(&d, 4);
                                for (is_add, text) in changes {
                                    all_lines.push(Line::from(vec![
                                        Span::raw("      "),
                                        Span::styled(
                                            format!("{} {text}", if is_add { "+" } else { "-" }),
                                            Style::default().fg(if is_add {
                                                Color::Green
                                            } else {
                                                Color::Red
                                            }),
                                        ),
                                    ]));
                                }
                                if rest > 0 {
                                    all_lines.push(Line::from(Span::styled(
                                        format!("      … (+{rest} more — V or /diff for full)"),
                                        Style::default()
                                            .fg(Color::DarkGray)
                                            .add_modifier(Modifier::ITALIC),
                                    )));
                                }
                            }
                        }
                    }
                    Content::Reasoning(r) => {
                        let rline = Line::from(vec![
                            Span::raw("    "),
                            Span::styled(format!("… {r}"), Theme::reasoning_prefix()),
                        ]);
                        all_lines.push(rline);
                    }
                    _ => {}
                }
            }

            if self.show_thinking && m.role == "assistant" {
                if let Some(reasoning) = &m.reasoning {
                    if !reasoning.trim().is_empty() {
                        let is_expanded =
                            self.expanded_reasoning.contains(&abs_idx) || self.thinking_expanded;
                        let collapsed = !is_expanded;

                        let glyph = if collapsed { "✦" } else { "✧" };
                        let hint = if collapsed {
                            "[Ctrl+O/Ctrl+G to expand]"
                        } else {
                            "[Ctrl+O/Ctrl+G to collapse]"
                        };
                        let tline = Line::from(vec![
                            Span::styled(
                                format!(" {glyph} {}… ", self.thinking_title),
                                Theme::thinking_title_style(),
                            ),
                            Span::styled(
                                format!("· {} chars ", reasoning.len()),
                                Theme::thinking_label_style(),
                            ),
                            Span::styled(
                                hint,
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]);
                        all_lines.push(tline);
                        if !collapsed {
                            for line in reasoning.lines().take(12) {
                                all_lines.push(Line::from(vec![
                                    Span::styled(" │ ", Theme::thinking_icon_style()),
                                    Span::styled(line.to_string(), Theme::reasoning_prefix()),
                                ]));
                            }
                            if reasoning.lines().count() > 12 {
                                all_lines.push(Line::from(Span::styled(
                                    format!("   … ({} lines total)", reasoning.lines().count()),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                )));
                            }
                        }
                    }
                }
            }

            if let Some(total) = collapsed_total {
                all_lines.push(Line::from(Span::styled(
                    format!(
                        "  … (+{} lines — Ctrl+X to expand)",
                        total.saturating_sub(CHAT_COLLAPSE_HEAD)
                    ),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }

        if self.show_thinking {
            let has_thinking = !self.thinking_buf.is_empty() || self.busy;
            if has_thinking {
                let title = &self.thinking_title;
                let collapsed = !self.thinking_expanded;
                let glyph = if collapsed { "✦" } else { "✧" };
                let count = if self.thinking_buf.is_empty() {
                    "live".to_string()
                } else {
                    format!("{} chars", self.thinking_buf.len())
                };
                let tline = Line::from(vec![
                    Span::styled(format!(" {glyph} {title}… "), Theme::thinking_title_style()),
                    Span::styled(format!("· {count} "), Theme::thinking_label_style()),
                    Span::styled(
                        if collapsed {
                            "[Ctrl+O/Ctrl+G to expand]"
                        } else {
                            "[Ctrl+O/Ctrl+G to collapse]"
                        },
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]);
                all_lines.push(tline);
                if !collapsed {
                    let reasoning_preview = if self.thinking_buf.is_empty() {
                        if self.busy {
                            "Reasoning…"
                        } else {
                            "(no reasoning captured — model may not support thinking)"
                        }
                    } else {
                        &self.thinking_buf
                    };
                    for line in reasoning_preview.lines().take(6) {
                        let rline = Line::from(vec![
                            Span::styled(" │ ", Theme::thinking_icon_style()),
                            Span::styled(line.to_string(), Theme::reasoning_prefix()),
                        ]);
                        all_lines.push(rline);
                    }
                    if self.busy && self.thinking_buf.len() > 200 {
                        all_lines.push(Line::from(Span::styled(
                            "   … streaming…",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
            }
        }

        if self.busy
            && (self.messages.is_empty()
                || self
                    .messages
                    .last()
                    .map(|m| m.role != "assistant")
                    .unwrap_or(true))
            && !self.streaming_buf.is_empty()
            && !self.show_thinking
        {
            let sraw = self.streaming_buf.clone();
            let s_lines: Vec<&str> = sraw.lines().collect();
            let s_joined = join_math_lines(&s_lines);
            let s_lines: Vec<&str> = s_joined.iter().map(|s| s.as_str()).collect();
            if s_lines.is_empty() {
                let mut spans = vec![Span::styled("● ", Theme::assistant_prefix())];
                spans.extend(markdown_inline_spans(&sraw));
                spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                all_lines.push(Line::from(spans));
            } else {
                let stream_table_w = (area.width as usize).saturating_sub(4).max(20);
                let mut stream_in_code = false;
                let mut stream_skip_until = 0;
                for (idx, seg) in s_lines.iter().enumerate() {
                    if idx < stream_skip_until {
                        continue;
                    }
                    let trimmed = seg.trim();
                    if trimmed.starts_with("```") {
                        let lang = code_lang_from_fence(trimmed);
                        let entering = !stream_in_code;
                        stream_in_code = !stream_in_code;
                        let mut spans = vec![Span::styled(
                            if idx == 0 { "● " } else { "  " },
                            Theme::assistant_prefix(),
                        )];
                        if entering {
                            spans.extend(fence_open_spans(&lang));
                        } else {
                            spans.extend(fence_close_spans());
                        }
                        if idx == s_lines.len() - 1 {
                            spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                        }
                        all_lines.push(Line::from(spans));
                        continue;
                    }

                    if !stream_in_code && trimmed.contains('|') {
                        if let Some((tbl, consumed)) =
                            render_table_block(&s_lines[idx..], stream_table_w)
                        {
                            for (ti, tline) in tbl.into_iter().enumerate() {
                                let mut spans = vec![Span::styled(
                                    if idx == 0 && ti == 0 { "● " } else { "  " },
                                    Theme::assistant_prefix(),
                                )];
                                spans.extend(tline);
                                if idx + ti + 1 >= s_lines.len() {
                                    spans.push(Span::styled(
                                        " ▌",
                                        Style::default().fg(Color::Yellow),
                                    ));
                                }
                                all_lines.push(Line::from(spans));
                            }
                            stream_skip_until = idx + consumed;
                            continue;
                        }
                    }

                    if !stream_in_code && (trimmed.starts_with("$$") || trimmed.starts_with("\\["))
                    {
                        if let Some((mrows, consumed)) =
                            render_math_block(&s_lines[idx..], stream_table_w)
                        {
                            for (mi, mrow) in mrows.into_iter().enumerate() {
                                let mut spans = vec![Span::styled(
                                    if idx == 0 && mi == 0 { "● " } else { "  " },
                                    Theme::assistant_prefix(),
                                )];
                                spans.extend(mrow);
                                if idx + mi + 1 >= s_lines.len() {
                                    spans.push(Span::styled(
                                        " ▌",
                                        Style::default().fg(Color::Yellow),
                                    ));
                                }
                                all_lines.push(Line::from(spans));
                            }
                            stream_skip_until = idx + consumed;
                            continue;
                        }
                    }
                    let mut spans = vec![Span::styled(
                        if idx == 0 { "● " } else { "  " },
                        Theme::assistant_prefix(),
                    )];
                    if stream_in_code {
                        spans.push(Span::styled("│ ", crate::theme::Theme::code_block()));
                        spans.extend(highlighted_code_spans(seg));
                    } else {
                        spans.extend(markdown_inline_spans(seg));
                    }
                    if idx == s_lines.len() - 1 {
                        spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    }
                    all_lines.push(Line::from(spans));
                }

                if stream_in_code {
                    let mut spans = vec![Span::styled("  ", Theme::assistant_prefix())];
                    spans.extend(fence_close_spans());
                    spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    all_lines.push(Line::from(spans));
                }
            }
        }

        if self.copy_pending {
            self.copy_pending = false;
            let stale = self
                .selection
                .as_ref()
                .is_some_and(|s| s.session != self.session_id);
            if stale {
                self.selection = None;
            }
            if let Some(sel) = self.selection.clone() {
                let (top, bottom) = sel.ordered();
                if top.line < all_lines.len() {
                    let text = extract_selection_text(&all_lines, top, bottom);
                    if text.trim().is_empty() {
                        self.selection = None;
                        self.status = "nothing to copy".into();
                    } else {
                        match clipboard_copy_text(&text) {
                            Ok((n, via)) => {
                                self.status = format!(
                                    "copied {n} chars via {via} — selection kept (Esc clears)"
                                );
                            }
                            Err(e) => {
                                self.status = format!("copy failed ({e}) — selection kept");
                            }
                        }
                    }
                } else {
                    self.selection = None;
                }
            }
        }

        let stale = self
            .selection
            .as_ref()
            .is_some_and(|s| s.session != self.session_id || self.messages.len() < s.msg_len);
        if stale {
            self.selection = None;
        } else if let Some(sel) = self.selection.as_mut() {
            sel.msg_len = self.messages.len();
        }
        let visible = (area.height as usize).saturating_sub(2).max(5);
        let total = all_lines.len();
        if self.scroll > 0 {
            if total > self.chat_total_lines {
                self.scroll += total - self.chat_total_lines;
            }

            let max_scroll = total.saturating_sub(visible);
            if self.scroll > max_scroll {
                self.scroll = max_scroll;
            }
        }
        self.chat_total_lines = total;
        let start = total.saturating_sub(visible + self.scroll);
        let end = (start + visible).min(total);

        self.view_start = start;
        self.view_total = total;
        let sel_range = self
            .selection
            .as_ref()
            .filter(|s| s.session == self.session_id)
            .map(|s| s.ordered());

        let inner_w = self
            .chat_inner()
            .map(|r| r.width as usize)
            .unwrap_or(80)
            .max(8);

        let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
        self.vis_rows.clear();
        for (i, line) in all_lines[start..end].iter().enumerate() {
            let li = start + i;
            let mut base: Vec<Span<'static>> = line.spans.clone();
            if let Some((top, bottom)) = sel_range {
                if li >= top.line && li <= bottom.line {
                    let s = if li == top.line { top.col } else { 0 };
                    let e = if li == bottom.line {
                        bottom.col
                    } else {
                        usize::MAX
                    };
                    if s < e {
                        base = highlight_range(&line.spans, s, e, Theme::text_selection());
                    }
                }
            }
            for (row_spans, col_start) in wrap_line_preserving(&Line::from(base), inner_w) {
                self.vis_rows.push((li, col_start));
                lines.push(Line::from(row_spans));
            }
        }

        let title = format!(
            " Chat {} msgs {} ",
            self.messages.len(),
            if self.scroll > 0 {
                format!("↑{} ", self.scroll)
            } else {
                "".to_string()
            }
        );
        let chat = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Theme::card_border())
                    .style(Theme::chat_bg()),
            )
            .wrap(Wrap { trim: false })
            .style(Theme::chat_bg());
        frame.render_widget(chat, area);
    }

    pub(crate) fn draw_input(&self, frame: &mut Frame, area: Rect) {
        let model_short = self.model.split('/').next_back().unwrap_or(&self.model);
        let title = if self.busy {
            format!(
                " Input — {} [{}] (busy — type ahead, Enter queues • Esc cancels turn) ",
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
        let input = Paragraph::new(display_text)
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
                Line::from("  Ctrl-C     copy selection, else quit"),
                Line::from("  Ctrl-Alt-C copy selection (never quits)"),
                Line::from("  Mouse      drag select+auto-copy (edge auto-scroll) • wheel scroll • middle-click pastes • Shift+drag native select"),
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
                    .and_then(|store| store.list_sessions_filtered(None, None, true, 50, 0));
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
                            Span::raw(format!("  ({} shown{})", total, if filter.is_empty() { "" } else { " filtered" })),
                        ]));
                        lines.push(Line::from(Span::styled(
                            format!(" Chats — {} saved • {} msgs current • {}  • Enter resume • n new • f fork • r rename • a archive • d delete • Esc close ", total, self.messages.len(), &self.session_id[..8.min(self.session_id.len())]),
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
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!(" ARMED at #{seq} — Enter again to rewind, Esc cancels "),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testkit::*;
    fn long_tool_app() -> (App, String) {
        let mut app = test_app();
        let body = serde_json::json!({
            "ok": false,
            "error": (0..20).map(|i| format!("e{i:02}")).collect::<Vec<_>>().join("\n"),
        })
        .to_string();
        let mut m = Msg::new("assistant", "running");
        m.items.push(Content::ToolCall {
            id: "call_bash_1".into(),
            name: "bash".into(),
            args: "{\"command\": \"noisy-fail\"}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_bash_1".into(),
            content: body.clone(),
            ok: false,
        });
        app.messages.push(m);
        (app, body)
    }

    #[test]
    fn long_tool_output_collapses_with_hint() {
        let (mut app, _) = long_tool_app();
        let text = render_text(&mut app, 100, 40);
        assert!(text.contains("e00"), "head shown");
        assert!(!text.contains("e19"), "tail hidden");
        assert!(text.contains("Ctrl+X to expand"), "hint shown");
        assert!(text.contains("+12 more lines"), "remaining count");
    }

    #[test]
    fn ctrl_x_expands_most_recent_tool_output() {
        let (mut app, _) = long_tool_app();
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("e19"), "expanded reveals all");
        assert!(!text.contains("Ctrl+X to expand"), "hint gone when open");
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(!text.contains("e19"), "toggles back to collapsed");
    }

    #[test]
    fn read_tool_card_shows_only_line_counts() {
        fn read_preview(content: serde_json::Value) -> String {
            let app = test_app();
            let items = vec![Content::ToolCall {
                id: "call_read_9".into(),
                name: "read".into(),
                args: "{}".into(),
                status: ToolStatus::Done,
            }];
            app.tool_result_preview(&items, "call_read_9", &content.to_string(), true)
                .expect("read always previews")
        }
        let full = serde_json::json!({
            "ok": true, "path": "deck.cir",
            "content": "R1 1 0 1k\nC1 1 0 1u\n", "total_lines": 2,
        });
        assert_eq!(read_preview(full), "deck.cir (2 lines)");
        let paged = serde_json::json!({
            "ok": true, "path": "deck.cir",
            "content": "R1 1 0 1k\nC1 1 0 1u\nR2 2 0 2k\n",
            "offset": 10, "limit": 20, "total_lines": 100,
        });
        assert_eq!(read_preview(paged), "deck.cir (lines 11–13 of 100)");
        let bare = serde_json::json!({
            "ok": true, "path": "deck.cir", "content": "R1 1 0 1k\n",
        });
        let preview = read_preview(bare);
        assert!(!preview.contains("R1"), "no content leak: {preview}");
        let empty = serde_json::json!({
            "ok": true, "path": "empty.txt", "content": "", "total_lines": 0,
        });
        assert_eq!(read_preview(empty), "empty.txt (0 lines)");
        // Chat render carries no read content either.
        let mut app = test_app();
        let mut m = Msg::new("assistant", "reading");
        m.items.push(Content::ToolCall {
            id: "call_read_9".into(),
            name: "read".into(),
            args: "{}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_read_9".into(),
            content: full_content(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("R1 1 0"), "chat shows no read content");
        assert!(text.contains("50 lines"), "chat shows the count");
    }

    fn full_content() -> String {
        let body = (0..50)
            .map(|i| format!("R{i} 1 0 1k"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::json!({
            "ok": true, "path": "deck.cir", "content": body, "total_lines": 50,
        })
        .to_string()
    }

    #[test]
    fn ctrl_x_prefers_tool_output_over_earlier_user_text() {
        let (mut app, _) = long_tool_app();
        let long = (0..20)
            .map(|i| format!("u{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut m0 = Msg::new("user", long);
        m0.timestamp = "04:12".into();
        app.messages.insert(0, m0);
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 80);
        assert!(text.contains("e19"), "later tool output wins");
        assert!(!text.contains("u19"), "earlier user text stays collapsed");
    }
    #[tokio::test]
    async fn chat_pageup_scrolls_view() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let before = render_text(&mut app, 80, 24);
        assert!(before.contains("line 059"), "tail visible at bottom");
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 10);
        let after = render_text(&mut app, 80, 24);
        assert!(!after.contains("line 059"), "pageup moved tail out of view");
        assert!(after.contains("line 049"), "pageup shows earlier lines");

        app.handle_key(KeyCode::PageDown).await.unwrap();
        assert_eq!(app.scroll, 0);
        let back = render_text(&mut app, 80, 24);
        assert!(back.contains("line 059"), "pagedown returns to tail");
    }

    #[tokio::test]
    async fn chat_holds_position_on_new_content() {
        let mut app = test_app();
        push_lines(&mut app, 60, 0);

        let _ = render_text(&mut app, 80, 24);
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        app.handle_key(KeyCode::PageUp).await.unwrap();
        assert_eq!(app.scroll, 30);
        let first = render_text(&mut app, 80, 24);

        let first_row = first.lines().nth(2).unwrap_or("").to_string();

        push_lines(&mut app, 5, 60);
        let second = render_text(&mut app, 80, 24);
        let second_row = second.lines().nth(2).unwrap_or("").to_string();
        assert_eq!(
            first_row, second_row,
            "scrolled-up view holds position on new content"
        );
        assert_eq!(app.scroll, 35, "scroll compensates for new lines");
    }

    #[test]
    fn picker_cursor_stays_visible_on_short_terminal() {
        let mut app = test_app();
        app.available_models = (0..50).map(|i| format!("openai/model-{i:02}")).collect();
        app.popup = Popup::ModelPicker;
        app.model_cursor = 40;

        let text = render_text(&mut app, 80, 20);
        assert!(
            text.contains("openai/model-40"),
            "selected model visible with height-aware viewport"
        );
        assert!(text.contains("▶"), "selection marker visible");
    }

    #[test]
    fn table_borders_align_on_screen() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich | Dart is less common than JS/Swift/Kotlin |\n\n```bash\necho hi\n```",
        ));
        let text = render_text(&mut app, 120, 40);
        let rows: Vec<String> = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with('┌') || t.starts_with('│') || t.starts_with('├') || t.starts_with('└')
            })
            .map(|l| l.trim_end().to_string())
            .collect();
        assert!(
            rows.len() >= 8,
            "table + fence rows present: {}",
            rows.len()
        );

        let nogutter = |l: &&String| l.chars().skip(2).collect::<String>();
        let table: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).starts_with(['┌', '│', '├', '└']) && !l.contains("echo"))
            .collect();
        let widths: Vec<usize> = table.iter().map(|l| nogutter(l).width()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "on-screen table rows align: {widths:?}"
        );

        let fence: Vec<&String> = rows
            .iter()
            .filter(|l| nogutter(l).contains('┌') || nogutter(l).starts_with('└'))
            .collect();
        assert_eq!(fence.len(), 2);
        let fence_w: Vec<usize> = fence.iter().map(|l| nogutter(l).width()).collect();
        assert_eq!(fence_w[0], fence_w[1], "fence top/bottom align");
    }

    #[test]
    fn headers_parse_inline_markdown() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "### 2. **Declarative UI**\nBody with **bold** and `code`.\n## Plain Header",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(!text.contains("**"), "no raw markers leak:\n{text}");
        assert!(text.contains("Declarative UI"));
        assert!(text.contains("Plain Header"));
    }

    #[test]
    fn unclosed_fence_gets_closing_rule() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "```bash\necho oops, never closed\n- a bullet after",
        ));
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains('└'), "auto-closed fence renders bottom rule");
    }

    #[test]
    fn bullet_wraps_with_hanging_indent() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "- Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));

        let text = render_text(&mut app, 40, 20);
        let rows: Vec<&str> = text.lines().collect();

        let first = rows.iter().find(|l| l.contains("Alpha")).unwrap();
        assert!(first.contains('-'), "marker on first row: {first:?}");
        let strip_border = |l: &&str| {
            l.trim_start()
                .strip_prefix('│')
                .unwrap_or(l.trim_start())
                .to_string()
        };
        let cont = rows
            .iter()
            .skip_while(|l| !l.contains("Alpha"))
            .nth(1)
            .unwrap();
        let inner = strip_border(cont);
        assert!(
            inner.trim_start().starts_with(|c: char| c.is_alphabetic()),
            "continuation has no marker: {cont:?}"
        );
        assert!(
            inner.starts_with("    "),
            "continuation hangs under item text: {cont:?}"
        );
    }

    #[test]
    fn blank_lines_render_singly() {
        let mut app = test_app();
        app.messages.push(Msg::new("assistant", "aaa\n\nbbb"));
        let text = render_text(&mut app, 100, 14);
        let chat: Vec<&str> = text.lines().skip(1).take(7).collect();
        let joined = chat.join("\n");
        let aaa_at = chat.iter().position(|l| l.contains("aaa")).unwrap();
        let bbb_at = chat.iter().position(|l| l.contains("bbb")).unwrap();
        assert_eq!(
            bbb_at,
            aaa_at + 2,
            "exactly one blank row between paragraphs:\n{joined}"
        );
    }

    #[test]
    fn full_render_stays_fast_on_large_chats() {
        let mut app = test_app();
        let body = "## Section\n\n### 2. **Declarative UI**\nBody with **bold**, *italic* and `code` spans across several words.\n\n- Alpha bullet with enough words to wrap across rows on narrow screens\n- Beta bullet\n\n| A | B |\n|---|---|\n| one | two |\n| three | four |\n\n```rust\nfn main() {\n    // comment\n    let s = \"hi\";\n    println!(\"{s}\");\n}\n```\n";
        for i in 0..60 {
            app.messages
                .push(Msg::new("user", format!("prompt number {i}")));
            app.messages.push(Msg::new("assistant", body));
        }
        let t0 = std::time::Instant::now();
        let text = render_text(&mut app, 120, 40);
        let ms = t0.elapsed().as_millis();
        println!("full render of {} msgs took {ms}ms", app.messages.len());
        assert!(text.contains("Declarative UI"));
        assert!(
            ms < 3000,
            "render stall: {ms}ms for a large chat (theme cache? memo?)"
        );
    }

    #[test]
    fn visual_dump_table_and_code() {
        let mut app = test_app();
        app.messages.push(Msg::new(
            "assistant",
            "## Core Concepts\n\n### 2. **Declarative UI**\nYou describe **what** the UI should look like for a given state, not **how** to transition between states. Flutter handles the rendering pipeline.\n\n### 4. **Rendering Engine (Skia)**\nFlutter draws everything using **Skia**. This means:\n- **Consistent** look and feel across platforms with pixel-perfect control over every pixel on the screen.\n- No bridge overhead unlike React Native and other cross-platform frameworks.\n\n| ✅ Pros | ⚠️ Cons |\n|---|---|\n| Single codebase for all platforms | Larger app size than native |\n| Rich, customizable widgets | Dart is less common than JS/Swift/Kotlin |\n\n## Quick Start\n\n```bash\ngit clone https://github.com/flutter/flutter.git\nexport PATH=\"$PATH:pwd/flutter/bin\"\nflutter create my_app\n```",
        ));
        let text = render_text(&mut app, 100, 48);
        println!("=== VISUAL DUMP START ===\n{text}\n=== VISUAL DUMP END ===");
        assert!(text.contains('┌') && text.contains('┐'));
        assert!(text.contains('└') && text.contains('┘'));
        assert!(!text.contains("|---|"), "no raw delimiter row leaks");
        assert!(!text.contains("**"), "no raw bold markers leak");
    }

    #[test]
    fn thinking_header_renders_distinct_marker() {
        let mut app = test_app();
        app.show_thinking = true;
        let mut m = Msg::new("assistant", "hello");
        m.reasoning = Some("private chain of thought".into());
        app.messages.push(m);
        let text = render_text(&mut app, 80, 30);
        assert!(
            text.contains("✦"),
            "collapsed thinking header carries the distinct marker"
        );
        assert!(text.contains("Thinking"), "thinking title renders");
        assert!(
            !text.contains("(Reasoning)"),
            "redundant parenthetical label is gone from chat lines"
        );
    }

    #[test]
    fn tool_cards_respect_verbosity() {
        fn card_app(level: &str) -> App {
            let mut app = test_app();
            app.tool_display.insert("bash".into(), level.into());
            let mut m = Msg::new("assistant", "working");
            m.items.push(Content::ToolCall {
                id: "call_hidden_test_1".into(),
                name: "bash".into(),
                args: "{\"command\": \"ls -la myproject\"}".into(),
                status: ToolStatus::Done,
            });
            m.items.push(Content::ToolResult {
                id: "call_hidden_test_1".into(),
                content: "{\"ok\":true,\"code\":0,\"stdout\":\"total 8\"}".into(),
                ok: true,
            });
            app.messages.push(m);
            app
        }

        let hidden = render_text(&mut card_app("hidden"), 100, 30);
        assert!(
            !hidden.contains("bash"),
            "hidden tool leaves no card: {hidden}"
        );

        let quiet = render_text(&mut card_app("quiet"), 100, 30);
        assert!(quiet.contains("bash"), "quiet keeps the name");
        assert!(
            !quiet.contains("ls -la"),
            "quiet drops the preview: {quiet}"
        );
        assert!(
            !quiet.contains("total 8"),
            "quiet drops the output: {quiet}"
        );

        let compact = render_text(&mut card_app("compact"), 100, 30);
        assert!(compact.contains("ls -la"), "compact shows the summary");
    }

    #[test]
    fn tool_result_line_shows_tool_name_not_call_id() {
        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            name: "write".into(),
            args: "{}".into(),
            status: ToolStatus::Done,
        });
        m.items.push(Content::ToolResult {
            id: "call_01a07444c2e1757388bf0c7479e682e2".into(),
            content: "{\"ok\":true,\"path\":\"./index.html\",\"old_lines\":0,\"new_lines\":136,\"bytes\":5082}".into(),
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("✔ write"), "result line uses tool name");
        assert!(
            !text.contains("call_01a07"),
            "raw provider id hidden from result line"
        );
        assert!(
            text.contains("136 lines"),
            "write summary renders on the card"
        );
    }

    #[test]
    fn edit_card_shows_inline_mini_diff() {
        let diff = "--- a/AGENTS.md\n+++ b/AGENTS.md\n@@ -43 +43 @@\n-old line here\n+new line here\n context\n+second add\n+third add\n+fourth add\n+fifth add\n";
        let (shown, rest) = diff_card_lines(diff, 4);
        assert_eq!(shown.len(), 4, "caps shown lines");
        assert_eq!(rest, 2, "counts the remainder");
        assert_eq!(shown[0], (false, "old line here".to_string()));
        assert_eq!(shown[1], (true, "new line here".to_string()));

        let mut app = test_app();
        let mut m = Msg::new("assistant", "done");
        m.items.push(Content::ToolCall {
            id: "call_edit_diff_1".into(),
            name: "edit".into(),
            args: "{\"path\": \"AGENTS.md\"}".into(),
            status: ToolStatus::Done,
        });
        let res = serde_json::json!({
            "ok": true, "path": "AGENTS.md",
            "old_lines": 69, "new_lines": 70, "bytes": 3000,
            "diff_preview": diff,
        })
        .to_string();
        m.items.push(Content::ToolResult {
            id: "call_edit_diff_1".into(),
            content: res,
            ok: true,
        });
        app.messages.push(m);
        let text = render_text(&mut app, 100, 30);
        assert!(text.contains("+ new line here"), "added line on card");
        assert!(text.contains("- old line here"), "removed line on card");
        assert!(text.contains("+2 more"), "remainder hint on card");
    }

    #[test]
    fn long_chat_messages_collapse_with_hint() {
        let short = "hello";
        let (t, h) = collapse_long_content(short, false);
        assert_eq!(t, short);
        assert!(h.is_none());
        let long: String = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (t, h) = collapse_long_content(&long, false);
        assert_eq!(h, Some(20));
        assert_eq!(t.lines().count(), CHAT_COLLAPSE_HEAD);
        assert!(t.contains("line 0") && !t.contains("line 19"));
        let (t, h) = collapse_long_content(&long, true);
        assert!(h.is_none() && t == long, "expanded shows all");

        let code = "```rust\n".to_string()
            + &(0..20)
                .map(|i| format!("l{i}"))
                .collect::<Vec<_>>()
                .join("\n");
        let (t, _) = collapse_long_content(&code, false);
        assert_eq!(
            t.lines().filter(|l| l.trim().starts_with("```")).count() % 2,
            0,
            "fence balanced"
        );

        let mut app = test_app();
        let mut m = Msg::new("user", long.clone());
        m.timestamp = "04:10".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 40);
        assert!(text.contains("line 0"), "head shown");
        assert!(!text.contains("line 19"), "tail hidden");
        assert!(text.contains("Ctrl+X to expand"), "hint shown");
        app.toggle_recent_long_message();
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "expanded reveals all");
        assert!(!text.contains("Ctrl+X to expand"), "hint gone when open");

        let mut app = test_app();
        let mut m = Msg::new("assistant", long.clone());
        m.timestamp = "04:11".into();
        app.messages.push(m);
        let text = render_text(&mut app, 100, 60);
        assert!(text.contains("line 19"), "model answer fully visible");
        assert!(!text.contains("Ctrl+X to expand"), "no hint on responses");
    }

    #[test]
    fn wrapped_continuation_maps_via_drawn_frame() {
        let mut app = test_app();
        app.show_thinking = false;
        app.messages.push(Msg::new(
            "assistant",
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
        ));
        let _ = render_text(&mut app, 40, 16);
        assert!(app.vis_rows.len() >= 2, "wraps: {:?}", app.vis_rows);

        let body: Vec<(usize, usize)> = app.vis_rows.clone();
        assert!(body.windows(2).all(|w| w[0].0 == w[1].0 || w[1].1 == 0));
        let inner = app.chat_inner().unwrap();
        let pos = app.screen_to_content(inner.x, inner.y + 1).expect("maps");
        assert_eq!(pos.line, body[1].0);
        assert_eq!(pos.col, body[1].1);
    }

    #[test]
    fn selection_paints_full_range_including_gutter() {
        let sel_bg = Theme::text_selection().bg;
        let gutter = Span::styled("  ", Style::default());
        let rest = highlighted_code_spans("  Widget build(BuildContext context) {");
        let mut spans = vec![gutter];
        spans.extend(rest);
        spans.insert(1, Span::styled("│ ", Theme::code_block()));
        let out = highlight_range(&spans, 0, usize::MAX, Theme::text_selection());
        assert!(!out.is_empty());
        for s in &out {
            assert_eq!(s.style.bg, sel_bg, "all cols selected: {:?}", s.content);
        }

        let out = highlight_range(&spans, 4, 8, Theme::text_selection());
        let painted: String = out
            .iter()
            .filter(|s| s.style.bg == sel_bg)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(painted.chars().count(), 4);
    }

    #[test]
    fn hint_bar_chips_keys_and_labels() {
        let line = App::hint_bar(&[("↑/↓", "Navigate"), ("enter", "Select"), ("esc", "Cancel")]);
        let texts: Vec<String> = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert!(texts.iter().any(|t| t.contains("↑/↓")), "key chip");
        assert!(texts.iter().any(|t| t.contains("Navigate")), "label");
        let chip = line
            .spans
            .iter()
            .find(|s| s.content.contains("enter"))
            .expect("enter chip");
        assert_eq!(chip.style.fg, Some(Color::Black));
        assert_eq!(chip.style.bg, Some(Color::Cyan));
        assert!(chip.style.add_modifier.contains(Modifier::BOLD));
        let label = line
            .spans
            .iter()
            .find(|s| s.content.contains("Cancel"))
            .expect("cancel label");
        assert_eq!(label.style.fg, Some(Color::DarkGray));
    }
}
