// Copyright 2026 Janada Sroor
// SPDX-License-Identifier: Apache-2.0

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

pub fn diff_lines(diff: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in diff.lines() {
        let style = if line.starts_with("+++") || line.starts_with("---") {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if line.starts_with("@@") {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if line.starts_with('+') {
            Style::default().fg(Color::Green).bg(Color::Rgb(30, 50, 30))
        } else if line.starts_with('-') {
            Style::default().fg(Color::Red).bg(Color::Rgb(50, 30, 30))
        } else if line.starts_with("diff --git") {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        out.push(Line::from(Span::styled(line.to_string(), style)));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled(
            "(no diff — last write was new file or identical)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )));
    }
    out
}

pub fn diff_summary(diff: &str) -> String {
    let added = diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .count();
    let removed = diff
        .lines()
        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
        .count();
    format!("+{added} -{removed}")
}
