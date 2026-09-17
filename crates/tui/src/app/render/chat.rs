// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use super::super::App;
use super::super::*;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

impl App {
    pub(crate) fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        // Subagent view owns no input box at all: collapse its row so chat
        // gains the space. Only the main agent controls input.
        let no_input = self.viewing_subagent();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(6),
                Constraint::Length(if no_input { 0 } else { 3 }),
                Constraint::Length(1),
            ])
            .split(area);

        self.chat_area = chunks[1];
        self.input_area = if no_input {
            Rect::new(0, 0, 0, 0)
        } else {
            chunks[2]
        };
        self.draw_header(frame, chunks[0]);
        self.draw_chat(frame, chunks[1]);
        if !no_input {
            self.draw_input(frame, chunks[2]);
        }
        self.draw_footer(frame, chunks[3]);

        if self.popup != Popup::None {
            self.draw_popup(frame, area);
        } else if !no_input && self.input.text.starts_with('/') {
            self.draw_completions(frame, chunks[2]);
        }
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

            let raw = if m.role == "system"
                && (m.content.starts_with("[background subagent finished]")
                    || m.content.starts_with("[background task finished]"))
            {
                // Internal wake prompts carry the full result/log for the
                // model. Collapse at render time too: some load paths
                // (sessions-dialog resume/fork) build messages straight
                // from the store and bypass the reload collapse — without
                // this a close+reopen shows the whole wall again.
                // Idempotent with the reload collapse (already-collapsed
                // rows reduce to their own first line).
                Self::collapse_wake_for_display(&m.role, &m.content)
            } else if m.content.chars().count() > 20000 {
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
                    // Tool-only assistant turn: skip the empty `●` line,
                    // tool cards below carry the content.
                    if !(raw.trim().is_empty() && !m.items.is_empty()) {
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
                    }
                } else {
                    let table_max_width = (area.width as usize).saturating_sub(4).max(20);
                    let code_max = code_inner_max(area.width as usize);
                    let fence_widths = fence_block_widths(&raw_lines, code_max);
                    let mut skip_until = 0;
                    let mut code_inner = FENCE_MIN_INNER;
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
                                code_inner =
                                    fence_widths.get(&li).copied().unwrap_or(FENCE_MIN_INNER);
                                let mut spans =
                                    vec![Span::styled(if li == 0 { prefix } else { "  " }, style)];
                                spans.extend(fence_open_spans(&lang, code_inner));
                                // No timestamp: structured rows are full-bleed
                                // by design — a 7-cell tail can never fit and
                                // would dangle onto its own row (dirty view).
                                all_lines.push(Line::from(spans));
                            } else {
                                let mut spans = vec![Span::styled("  ", style)];
                                spans.extend(fence_close_spans(code_inner));

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
                                    // No timestamp (see fence open above).
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
                                    // No timestamp (see fence open above).
                                    all_lines.push(Line::from(spans));
                                }
                                skip_until = li + consumed;
                                continue;
                            }
                        }
                        if in_code_block {
                            for (wi, piece) in
                                wrap_code_line(line_str, code_inner).iter().enumerate()
                            {
                                let mut line_spans = Vec::new();
                                line_spans.push(Span::styled(
                                    if li == 0 && wi == 0 { prefix } else { "  " },
                                    style,
                                ));
                                line_spans.extend(code_body_spans(piece, code_inner));

                                // No timestamp (see fence open above).
                                all_lines.push(Line::from(line_spans));
                            }
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
                        spans.extend(fence_close_spans(code_inner));
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
                // Live tool-call holder (content cleared at emit time):
                // skip the empty `·` line, cards below are the content.
                if display.trim().is_empty() && !m.items.is_empty() {
                    // Fall through to tool-card rendering below.
                } else {
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
                    } else if m.role == "user" {
                        Span::styled(display.clone(), Theme::user_message())
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
                        // Unique card title: `● name · summary · #shortid`
                        // (quiet/empty summary collapses to `● name · #shortid`).
                        let short = short_tool_id(id);
                        let tail = if preview.is_empty() {
                            format!(" · #{short}{elapsed_suffix}")
                        } else {
                            format!(" · {preview} · #{short}{elapsed_suffix}")
                        };
                        let preview_style = if *status == ToolStatus::Error {
                            crate::theme::Theme::error_message()
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        let tline = Line::from(vec![
                            Span::raw("    "),
                            Span::styled(
                                format!("{icon} {name}"),
                                Style::default().fg(col).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(tail, preview_style),
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

                        // Call line already carries `· #id` — result keeps
                        // the name only to avoid showing the id twice.
                        let label: String = if tname.is_empty() {
                            format!("#{}", short_tool_id(id))
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
                let stream_code_max = code_inner_max(area.width as usize);
                let stream_fence_widths = fence_block_widths(&s_lines, stream_code_max);
                let mut stream_in_code = false;
                let mut stream_code_inner = FENCE_MIN_INNER;
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
                            stream_code_inner = stream_fence_widths
                                .get(&idx)
                                .copied()
                                .unwrap_or(FENCE_MIN_INNER);
                            spans.extend(fence_open_spans(&lang, stream_code_inner));
                        } else {
                            spans.extend(fence_close_spans(stream_code_inner));
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
                    if stream_in_code {
                        let pieces = wrap_code_line(seg, stream_code_inner);
                        let last = pieces.len().saturating_sub(1);
                        for (pi, piece) in pieces.iter().enumerate() {
                            let mut spans = vec![Span::styled(
                                if idx == 0 && pi == 0 { "● " } else { "  " },
                                Theme::assistant_prefix(),
                            )];
                            spans.extend(code_body_spans(piece, stream_code_inner));
                            if idx == s_lines.len() - 1 && pi == last {
                                spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                            }
                            all_lines.push(Line::from(spans));
                        }
                        continue;
                    }
                    let mut spans = vec![Span::styled(
                        if idx == 0 { "● " } else { "  " },
                        Theme::assistant_prefix(),
                    )];
                    spans.extend(markdown_inline_spans(seg));
                    if idx == s_lines.len() - 1 {
                        spans.push(Span::styled(" ▌", Style::default().fg(Color::Yellow)));
                    }
                    all_lines.push(Line::from(spans));
                }

                if stream_in_code {
                    let mut spans = vec![Span::styled("  ", Theme::assistant_prefix())];
                    spans.extend(fence_close_spans(stream_code_inner));
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
                        // Clipboard backends (arboard/X11, xclip…) can
                        // stall for seconds — never park a frame on them.
                        // Copy on a helper thread; poll_clipboard_results
                        // reports the outcome on a later frame.
                        let (tx, rx) = std::sync::mpsc::channel();
                        self.copy_rx = Some(rx);
                        std::thread::spawn(move || {
                            let status = match clipboard_copy_text(&text) {
                                Ok((n, via)) => format!(
                                    "copied {n} chars via {via} — selection kept (Esc clears)"
                                ),
                                Err(e) => format!("copy failed ({e}) — selection kept"),
                            };
                            let _ = tx.send(status);
                        });
                        self.status = "copying…".into();
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
}
