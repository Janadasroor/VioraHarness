use super::latex::{
    render_display_math, render_inline_math, translate_bare_latex_chunk, translate_latex,
};
use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

pub(crate) fn markdown_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let s = text;
    let mut i = 0;
    let bytes = s.as_bytes();
    let len = s.len();

    let push_plain = |out: &mut Vec<Span<'static>>, chunk: &str| {
        if !chunk.is_empty() {
            out.push(Span::raw(translate_bare_latex_chunk(chunk)));
        }
    };
    while i < len {
        if bytes[i] == b'`' {
            if let Some(end) = s[i + 1..].find('`') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        crate::theme::Theme::inline_code(),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        if i + 1 < len && bytes[i] == b'$' && bytes[i + 1] == b'$' {
            if let Some(end) = s[i + 2..].find("$$") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        render_inline_math(inner),
                        crate::theme::Theme::math(),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'$' {
            let escaped = i > 0 && bytes[i - 1] == b'\\';
            let after = s[i + 1..].chars().next();
            if !escaped && matches!(after, Some(c) if !c.is_whitespace() && c != '$') {
                let rest = &s[i + 1..];
                let rb = rest.as_bytes();
                let mut j = 0;
                let mut found = None;
                while j < rb.len() {
                    if rb[j] == b'\n' {
                        break;
                    }
                    if rb[j] == b'$'
                        && j > 0
                        && !(rb[j - 1] as char).is_whitespace()
                        && rb[j - 1] != b'\\'
                    {
                        found = Some(j);
                        break;
                    }
                    j += 1;
                }
                if let Some(j) = found {
                    let inner = &rest[..j];
                    if !inner.is_empty() && !inner.contains('$') {
                        out.push(Span::styled(
                            render_inline_math(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 1 + j + 1;
                        continue;
                    }
                }
            }
        }
        if bytes[i] == b'\\' {
            if i + 1 < len && bytes[i + 1] == b'\\' {
                push_plain(&mut out, &s[i..i + 2]);
                i += 2;
                continue;
            }
            if i + 1 < len && bytes[i + 1] == b'(' {
                if let Some(end) = s[i + 2..].find("\\)") {
                    let inner = &s[i + 2..i + 2 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        out.push(Span::styled(
                            render_inline_math(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 2 + end + 2;
                        continue;
                    }
                }
            }

            if i + 1 < len && bytes[i + 1] == b'[' {
                if let Some(end) = s[i + 2..].find("\\]") {
                    let inner = &s[i + 2..i + 2 + end];
                    if !inner.is_empty() && !inner.contains('\n') {
                        out.push(Span::styled(
                            render_inline_math(inner),
                            crate::theme::Theme::math(),
                        ));
                        i += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        if i + 1 < len && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end) = s[i + 2..].find("**") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if i + 1 < len && bytes[i] == b'_' && bytes[i + 1] == b'_' {
            if let Some(end) = s[i + 2..].find("__") {
                let inner = &s[i + 2..i + 2 + end];
                if !inner.is_empty() && !inner.contains('\n') {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    i += 2 + end + 2;
                    continue;
                }
            }
        }
        if bytes[i] == b'*' {
            if let Some(end) = s[i + 1..].find('*') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains("**") {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }
        if bytes[i] == b'_' {
            if let Some(end) = s[i + 1..].find('_') {
                let inner = &s[i + 1..i + 1 + end];
                if !inner.is_empty() && !inner.contains('\n') && !inner.contains("__") {
                    out.push(Span::styled(
                        inner.to_string(),
                        Style::default().add_modifier(Modifier::ITALIC),
                    ));
                    i += 1 + end + 1;
                    continue;
                }
            }
        }

        let next_markers = ["**", "__", "`", "*", "_", "$$", "$", "\\(", "\\["]
            .iter()
            .filter_map(|m| s[i..].find(m).map(|p| p + i))
            .min()
            .unwrap_or(len);
        let end = if next_markers == i {
            i + 1
        } else {
            next_markers
        };

        if end == i {
            push_plain(&mut out, &s[i..i + 1]);
            i += 1;
        } else {
            push_plain(&mut out, &s[i..end]);
            i = end;
        }
    }
    if out.is_empty() {
        out.push(Span::raw(text.to_string()));
    }
    merge_adjacent_code_spans(out)
}

/// Join `` `a` = `b` `` runs (inline-code spans separated only by plain
/// operator/punctuation gaps — no words, no styled spans) into a single badge
/// so formula-heavy lines don't render as striped pills. Text is preserved
/// exactly; only span boundaries change.
fn merge_adjacent_code_spans(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let code = Theme::inline_code();
    // End index (of the next code span) for a mergeable gap run starting at
    // `from`: one or more unstyled spans with no word characters in between.
    let gap_end = |spans: &[Span], mut k: usize| -> Option<usize> {
        let mut saw = false;
        while k < spans.len() && spans[k].style == Style::default() {
            saw = true;
            if spans[k].content.chars().any(|c| c.is_alphanumeric()) {
                return None;
            }
            k += 1;
        }
        (saw && k < spans.len() && spans[k].style == code).then_some(k)
    };
    let mut merged: Vec<Span<'static>> = Vec::with_capacity(spans.len());
    let mut i = 0;
    while i < spans.len() {
        if spans[i].style != code {
            merged.push(Span {
                content: spans[i].content.clone(),
                style: spans[i].style,
            });
            i += 1;
            continue;
        }
        let mut text = spans[i].content.to_string();
        let mut j = i + 1;
        while let Some(k) = gap_end(&spans, j) {
            for s in &spans[j..=k] {
                text.push_str(&s.content);
            }
            j = k + 1;
        }
        if j == i + 1 {
            merged.push(Span {
                content: spans[i].content.clone(),
                style: spans[i].style,
            });
        } else {
            merged.push(Span::styled(text, code));
        }
        i = j;
    }
    merged
}

pub(crate) fn markdown_header_style(level: usize) -> Style {
    match level {
        1 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        2 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    }
}

pub(crate) fn code_lang_from_fence(line: &str) -> String {
    let t = line.trim();
    if t.starts_with("```") {
        let lang = t.trim_start_matches("```").trim().to_lowercase();

        lang.split_whitespace().next().unwrap_or("").to_string()
    } else {
        String::new()
    }
}

pub(crate) const FENCE_MIN_INNER: usize = 12;

/// Inner content width available for a code box: 2 (chat gutter) + 2 (`│ `)
/// + inner + 2 (` │`) + 1 margin must fit `area_width`.
pub(crate) fn code_inner_max(area_width: usize) -> usize {
    area_width.saturating_sub(7).max(FENCE_MIN_INNER)
}

pub(crate) fn expand_code_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

/// Map each fence-open line index to its box inner width: the widest body
/// line, clamped to `FENCE_MIN_INNER..=max_inner`. Unclosed fences run to
/// the end of `lines`.
pub(crate) fn fence_block_widths(
    lines: &[&str],
    max_inner: usize,
) -> std::collections::HashMap<usize, usize> {
    let mut out = std::collections::HashMap::new();
    let mut open: Option<usize> = None;
    let mut widest = 0;
    // `code_inner_max` guarantees max_inner >= MIN; stay robust anyway.
    let hi = max_inner.max(FENCE_MIN_INNER);
    for (idx, line) in lines.iter().enumerate() {
        if line.trim().starts_with("```") {
            if let Some(o) = open.take() {
                out.insert(o, widest.clamp(FENCE_MIN_INNER, hi));
                widest = 0;
            } else {
                open = Some(idx);
            }
            continue;
        }
        if open.is_some() {
            widest = widest.max(expand_code_tabs(line).width());
        }
    }
    if let Some(o) = open {
        out.insert(o, widest.clamp(FENCE_MIN_INNER, hi));
    }
    out
}

/// Hard-break a code body line to `inner`-wide rows (tabs expanded first).
/// Always returns at least one row.
pub(crate) fn wrap_code_line(line: &str, inner: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    let inner = inner.max(1);
    let expanded = expand_code_tabs(line);
    if expanded.width() <= inner {
        return vec![expanded];
    }
    let mut rows = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for c in expanded.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if cur_w + cw > inner && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(c);
        cur_w += cw;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }
    rows
}

pub(crate) fn fence_open_spans(lang: &str, inner: usize) -> Vec<Span<'static>> {
    let label = if lang.is_empty() {
        "code".to_string()
    } else {
        lang.to_string()
    };
    let inner = inner.max(FENCE_MIN_INNER);
    // `┌─ {label} {─…}┐` totals inner + 4.
    let dashes = (inner + 4).saturating_sub(4 + label.len() + 1);
    let mut spans = vec![Span::styled("┌─ ", Theme::code_block())];
    spans.push(Span::styled(label, Theme::code_lang_label()));
    spans.push(Span::styled(
        format!(" {}┐", "─".repeat(dashes)),
        Theme::code_block(),
    ));
    spans
}

pub(crate) fn fence_close_spans(inner: usize) -> Vec<Span<'static>> {
    let inner = inner.max(FENCE_MIN_INNER);
    vec![Span::styled(
        format!("└{}┘", "─".repeat(inner + 2)),
        Theme::code_block(),
    )]
}

/// Full closed box row: `│ {content padded to inner} │`, highlight-preserving.
/// `line` must already fit `inner` (see [`wrap_code_line`]).
pub(crate) fn code_body_spans(line: &str, inner: usize) -> Vec<Span<'static>> {
    let inner = inner.max(FENCE_MIN_INNER);
    let mut spans = vec![Span::styled("│ ", Theme::code_block())];
    spans.extend(highlighted_code_spans(line));
    let pad = inner.saturating_sub(line.width());
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Theme::code_block()));
    }
    spans.push(Span::styled(" │", Theme::code_block()));
    spans
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TableAlign {
    Left,
    Center,
    Right,
}

pub(crate) fn split_table_row(line: &str) -> Option<Vec<String>> {
    let mut s = line.trim();
    if !s.contains('|') {
        return None;
    }
    if s.starts_with('|') {
        s = &s[1..];
    }
    if s.ends_with('|') && !(s.ends_with("\\|") && !s.ends_with("\\\\|")) {
        s = &s[..s.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('|') => {
                    cur.push('|');
                    chars.next();
                }
                _ => cur.push(c),
            }
        } else if c == '|' {
            cells.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    Some(cells)
}

pub(crate) fn parse_delim_cell(cell: &str) -> Option<TableAlign> {
    let c = cell.trim();
    if c.is_empty() {
        return None;
    }
    let left = c.starts_with(':');
    let right = c.ends_with(':');
    let inner = c.trim_matches(':');
    if inner.is_empty() || !inner.chars().all(|x| x == '-') {
        return None;
    }
    Some(match (left, right) {
        (true, true) => TableAlign::Center,
        (false, true) => TableAlign::Right,
        _ => TableAlign::Left,
    })
}

pub(crate) fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

pub(crate) fn split_list_marker(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    for m in ["- ", "* ", "• "] {
        if let Some(rest) = t.strip_prefix(m) {
            return Some((m.trim_end().to_string(), rest.to_string()));
        }
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(rest) = t[digits.len()..].strip_prefix(". ") {
            return Some((format!("{digits}."), rest.to_string()));
        }
    }
    None
}

pub(crate) fn is_section_title(rest: &str) -> bool {
    let t = rest.trim_start();
    t.starts_with("**") && t[2..].contains("**")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelPos {
    pub(crate) line: usize,
    pub(crate) col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) anchor: SelPos,
    pub(crate) cursor: SelPos,
    pub(crate) session: String,
    pub(crate) msg_len: usize,
}

impl Selection {
    pub(crate) fn ordered(&self) -> (SelPos, SelPos) {
        if (self.cursor.line, self.cursor.col) < (self.anchor.line, self.anchor.col) {
            (self.cursor, self.anchor)
        } else {
            (self.anchor, self.cursor)
        }
    }

    pub(crate) fn is_caret(&self) -> bool {
        self.anchor == self.cursor
    }
}

pub(crate) fn highlight_range(
    spans: &[Span],
    start_col: usize,
    end_col: usize,
    sel_style: Style,
) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for s in spans {
        let mut before = String::new();
        let mut middle = String::new();
        let mut after = String::new();
        for c in s.content.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            let span = if col + w <= start_col || col >= end_col {
                if col < start_col {
                    &mut before
                } else {
                    &mut after
                }
            } else {
                &mut middle
            };
            span.push(c);
            col += w;
        }
        if !before.is_empty() {
            out.push(Span::styled(before, s.style));
        }
        if !middle.is_empty() {
            out.push(Span::styled(middle, s.style.patch(sel_style)));
        }
        if !after.is_empty() {
            out.push(Span::styled(after, s.style));
        }
    }
    if out.is_empty() {
        out.push(Span::raw(""));
    }
    out
}

pub(crate) fn extract_selection_text(lines: &[Line], top: SelPos, bottom: SelPos) -> String {
    let mut parts = Vec::new();
    if lines.is_empty() {
        return String::new();
    }
    let end = bottom.line.min(lines.len().saturating_sub(1));
    for (li, line) in lines.iter().enumerate().skip(top.line) {
        if li > end {
            break;
        }
        let spans = &line.spans;
        let s = if li == top.line { top.col } else { 0 };
        let e = if li == bottom.line {
            bottom.col
        } else {
            usize::MAX
        };
        if s < e {
            parts.push(extract_range_text(spans, s, e).trim_end().to_string());
        } else {
            parts.push(String::new());
        }
    }
    parts.join("\n")
}

pub(crate) fn extract_range_text(spans: &[Span], start_col: usize, end_col: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut col = 0;
    for s in spans {
        for c in s.content.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            if col + w > start_col && col < end_col {
                out.push(c);
            }
            col += w;
        }
    }
    out
}

pub(crate) fn chat_line(
    head: Vec<Span<'static>>,
    mut body: Vec<Span<'static>>,
    tail: Vec<Span<'static>>,
) -> Line<'static> {
    let mut full = head;
    full.append(&mut body);
    full.extend(tail);
    let text: String = full.iter().map(|s| s.content.as_ref()).collect();
    if text.trim().is_empty() {
        Line::default()
    } else {
        Line::from(full)
    }
}

pub(crate) fn push_cell_char(row: &mut Vec<Span<'static>>, row_w: &mut usize, c: char, st: Style) {
    let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    if let Some(last) = row.last_mut() {
        if last.style == st {
            last.content.to_mut().push(c);
            *row_w += cw;
            return;
        }
    }
    row.push(Span::styled(c.to_string(), st));
    *row_w += cw;
}

pub(crate) fn wrap_spans(
    segs: Vec<Span<'static>>,
    first_head: Vec<Span<'static>>,
    cont_head: Vec<Span<'static>>,
    tail: Vec<Span<'static>>,
    width: usize,
) -> Vec<Line<'static>> {
    let head_w = spans_width(&first_head);
    let cont_w = spans_width(&cont_head);

    let mut cells: Vec<(char, Style)> = Vec::new();
    for s in &segs {
        for c in s.content.chars() {
            cells.push((c, s.style));
        }
    }

    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for cell in cells {
        if cell.0.is_whitespace() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(cell);
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let word_width = |w: &[(char, Style)]| -> usize {
        w.iter()
            .map(|(c, _)| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
            .sum()
    };

    let max_avail = width.saturating_sub(head_w.min(cont_w)).max(8);
    let mut norm: Vec<Vec<(char, Style)>> = Vec::new();
    for w in words {
        if word_width(&w) <= max_avail || w.len() <= 1 {
            norm.push(w);
        } else {
            let mut chunk: Vec<(char, Style)> = Vec::new();
            let mut cw = 0;
            for cell in w {
                let cwid = unicode_width::UnicodeWidthChar::width(cell.0).unwrap_or(0);
                if cw + cwid > max_avail && !chunk.is_empty() {
                    norm.push(std::mem::take(&mut chunk));
                    cw = 0;
                }
                chunk.push(cell);
                cw += cwid;
            }
            if !chunk.is_empty() {
                norm.push(chunk);
            }
        }
    }
    if norm.is_empty() {
        return vec![chat_line(first_head, Vec::new(), tail)];
    }
    let mut rows: Vec<Line> = Vec::new();
    let mut row: Vec<Span> = Vec::new();
    let mut row_w = 0;
    let mut avail = width.saturating_sub(head_w).max(8);
    let mut first_row = true;
    let mut wi = 0;
    while wi < norm.len() {
        let ww = word_width(&norm[wi]);

        let need = ww + usize::from(!row.is_empty());
        if row_w + need <= avail {
            if !row.is_empty() {
                push_cell_char(&mut row, &mut row_w, ' ', Style::default());
            }
            for (c, st) in &norm[wi] {
                push_cell_char(&mut row, &mut row_w, *c, *st);
            }
            wi += 1;
        } else if row_w == 0 {
            for (c, st) in &norm[wi] {
                push_cell_char(&mut row, &mut row_w, *c, *st);
            }
            wi += 1;
        } else {
            let head = if first_row {
                first_head.clone()
            } else {
                cont_head.to_vec()
            };
            rows.push(chat_line(head, std::mem::take(&mut row), Vec::new()));
            first_row = false;
            avail = width.saturating_sub(cont_w).max(8);
            row_w = 0;
        }
    }
    let head = if first_row {
        first_head
    } else {
        cont_head.to_vec()
    };

    rows.push(chat_line(head, row, tail));
    rows
}

pub(crate) fn wrap_line_preserving(
    line: &Line<'static>,
    width: usize,
) -> Vec<(Vec<Span<'static>>, usize)> {
    use unicode_width::UnicodeWidthChar;
    let width = width.max(1);
    let cell_w = |c: char| UnicodeWidthChar::width(c).unwrap_or(0);

    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
        .collect();

    let mut words: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    let mut in_space = cells.first().is_some_and(|(c, _)| *c == ' ' || *c == '\t');
    for cell in cells {
        let ws = cell.0 == ' ' || cell.0 == '\t';
        if ws != in_space && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
            in_space = ws;
        }
        cur.push(cell);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    let word_width = |w: &[(char, Style)]| -> usize {
        w.iter()
            .map(|(c, _)| if *c == '\t' { 4 } else { cell_w(*c) })
            .sum()
    };
    let mut rows: Vec<(Vec<Span<'static>>, usize)> = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut row_w: usize = 0;
    let mut consumed: usize = 0;
    let mut wi = 0;

    let mut flush_row = |row: &mut Vec<Span<'static>>, row_w: &mut usize, consumed: &mut usize| {
        rows.push((std::mem::take(row), *consumed));
        *consumed += *row_w;
        *row_w = 0;
    };
    while wi < words.len() {
        let ww = word_width(&words[wi]);
        if row_w + ww <= width {
            for (c, st) in &words[wi] {
                if *c == '\t' {
                    let pad = 4 - (row_w % 4);
                    for _ in 0..pad {
                        push_cell_char(&mut row, &mut row_w, ' ', *st);
                    }
                } else {
                    push_cell_char(&mut row, &mut row_w, *c, *st);
                }
            }
            wi += 1;
        } else if row_w == 0 {
            let mut placed_any = false;
            let mut ci = 0;
            while ci < words[wi].len() {
                let (c, st) = words[wi][ci];
                let cw = if c == '\t' {
                    4 - (row_w % 4)
                } else {
                    cell_w(c)
                };
                if row_w + cw > width && !row.is_empty() {
                    break;
                }
                if c == '\t' {
                    for _ in 0..cw {
                        push_cell_char(&mut row, &mut row_w, ' ', st);
                    }
                } else {
                    push_cell_char(&mut row, &mut row_w, c, st);
                }
                placed_any = true;
                ci += 1;
            }
            words[wi].drain(..ci);
            if words[wi].is_empty() {
                wi += 1;
            }
            if placed_any {
                flush_row(&mut row, &mut row_w, &mut consumed);
            } else {
                wi += 1;
            }
        } else {
            flush_row(&mut row, &mut row_w, &mut consumed);
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push((row, consumed));
    }
    rows
}

pub(crate) fn truncate_by_width(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

pub(crate) fn join_math_lines(lines: &[&str]) -> Vec<String> {
    pub(crate) fn has_unclosed_bracket(line: &str) -> bool {
        match line.rfind("\\[") {
            Some(pos) => !line[pos + 2..].contains("\\]"),
            None => false,
        }
    }
    pub(crate) fn dollar_unclosed(line: &str) -> bool {
        line.matches("$$").count() % 2 == 1
    }
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        if !in_fence && (has_unclosed_bracket(lines[i]) || dollar_unclosed(lines[i])) {
            let closer = if has_unclosed_bracket(lines[i]) {
                "\\]"
            } else {
                "$$"
            };
            let mut buf = lines[i].to_string();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() && j - i <= 60 {
                buf.push('\n');
                buf.push_str(lines[j]);
                j += 1;
                if lines[j - 1].contains(closer) {
                    closed = true;
                    break;
                }
            }
            if closed {
                out.push(buf);
                i = j;
                continue;
            }

            out.push(lines[i].to_string());
            i += 1;
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out
}

pub(crate) fn render_math_block(
    lines: &[&str],
    max_width: usize,
) -> Option<(Vec<Vec<Span<'static>>>, usize)> {
    use unicode_width::UnicodeWidthStr as _;
    let first = lines.first()?.trim();
    let (close, after_open) = match (first.strip_prefix("$$"), first.strip_prefix("\\[")) {
        (Some(rest), _) => ("$$", rest),
        (_, Some(rest)) => ("\\]", rest),
        _ => return None,
    };
    let (content, consumed) = if let Some(end) = after_open.find(close) {
        (after_open[..end].to_string(), 1usize)
    } else {
        let mut buf = after_open.to_string();
        let mut consumed = 1usize;
        let mut closed = false;
        for line in lines.iter().skip(1) {
            consumed += 1;
            if let Some(pos) = line.find(close) {
                buf.push('\n');
                buf.push_str(&line[..pos]);
                closed = true;
                break;
            }
            buf.push('\n');
            buf.push_str(line);
            if consumed > 60 {
                break;
            }
        }
        if !closed {
            return None;
        }
        (buf, consumed)
    };
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let translated = translate_latex(&content);
    if let Some(art) = render_display_math(&content, max_width) {
        return Some((math_block_rows(art, max_width), consumed));
    }
    for eq in translated.lines() {
        let eq = eq.trim();
        if eq.is_empty() {
            continue;
        }
        let eq = truncate_by_width(eq, max_width);
        let pad = max_width.saturating_sub(eq.width()) / 2;
        rows.push(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(eq, crate::theme::Theme::math()),
        ]);
    }
    if rows.is_empty() {
        return None;
    }
    Some((rows, consumed))
}

fn math_block_rows(art: Vec<String>, max_width: usize) -> Vec<Vec<Span<'static>>> {
    use unicode_width::UnicodeWidthStr as _;
    // Center the block as a whole: per-line centering would destroy the
    // internal alignment of the 2D layout.
    let block = art.iter().map(|l| l.width()).max().unwrap_or(0);
    let pad = max_width.saturating_sub(block) / 2;
    art.into_iter()
        .map(|eq| {
            vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(eq, crate::theme::Theme::math()),
            ]
        })
        .collect()
}

pub(crate) fn render_table_block(
    lines: &[&str],
    max_width: usize,
) -> Option<(Vec<Vec<Span<'static>>>, usize)> {
    let header = split_table_row(lines.first()?)?;
    if header.is_empty() {
        return None;
    }
    let delim_line = lines.get(1)?;
    let delim_cells = split_table_row(delim_line)?;
    let mut aligns: Vec<TableAlign> = delim_cells
        .iter()
        .map(|c| parse_delim_cell(c))
        .collect::<Option<Vec<_>>>()?;
    let mut rows: Vec<Vec<String>> = vec![header];
    let mut consumed = 2;
    for line in lines.iter().skip(2) {
        match split_table_row(line) {
            Some(cells) if !cells.iter().all(|c| c.is_empty()) => {
                rows.push(cells);
                consumed += 1;
            }
            _ => break,
        }
    }

    let ncols = rows.first().map(|r| r.len()).unwrap_or(0);
    if ncols == 0 {
        return None;
    }
    for row in rows.iter_mut().skip(1) {
        if row.len() > ncols {
            let tail = row[ncols - 1..].join("|");
            row.truncate(ncols - 1);
            row.push(tail);
        }
    }
    for row in rows.iter_mut() {
        row.resize(ncols, String::new());
    }
    aligns.resize(ncols, TableAlign::Left);

    let styled: Vec<Vec<Vec<Span>>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| markdown_inline_spans(&c.replace('\n', " ")))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = (0..ncols)
        .map(|c| {
            styled
                .iter()
                .map(|row| spans_width(&row[c]))
                .max()
                .unwrap_or(0)
        })
        .collect();

    for w in widths.iter_mut() {
        *w = (*w).min(48);
    }
    loop {
        let total: usize = widths.iter().sum::<usize>() + ncols * 2 + (ncols + 1);
        if total <= max_width.max(ncols * 2 + ncols + 1 + 4) {
            break;
        }
        let mut widest = None;
        for (i, w) in widths.iter().enumerate() {
            if *w > 4 && widest.map(|(_, mw)| *w > mw).unwrap_or(true) {
                widest = Some((i, *w));
            }
        }
        match widest {
            Some((i, _)) => widths[i] -= 1,
            None => break,
        }
    }

    let border = Theme::table_border();
    let rule_row = |left: &str, mid: &str, right: &str| -> Vec<Span<'static>> {
        let mut spans = vec![Span::styled(left.to_string(), border)];
        for (i, w) in widths.iter().enumerate() {
            spans.push(Span::styled("─".repeat(w + 2), border));
            if i + 1 == widths.len() {
                spans.push(Span::styled(right.to_string(), border));
            } else {
                spans.push(Span::styled(mid.to_string(), border));
            }
        }
        spans
    };
    let data_row = |cells: &[Vec<Span<'static>>], bold: bool| -> Vec<Vec<Span<'static>>> {
        use unicode_width::UnicodeWidthStr;
        let style_add = if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        // Plain text per cell (styled uniformly, as before); wrapped — never clipped.
        let wrapped: Vec<Vec<String>> = cells
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let plain: String = cell.iter().map(|s| s.content.as_ref()).collect();
                wrap_cell_text(&plain, widths[i].max(1))
            })
            .collect();
        let height = wrapped.iter().map(|w| w.len()).max().unwrap_or(1).max(1);
        let mut out_rows = Vec::with_capacity(height);
        for r in 0..height {
            let mut spans = vec![Span::styled("│ ".to_string(), border)];
            for (i, col) in wrapped.iter().enumerate() {
                let target = widths[i];
                let line = col.get(r).map(|s| s.as_str()).unwrap_or("");
                let w = line.width();
                let pad = target.saturating_sub(w);
                let inner =
                    Span::styled(line.to_string(), Style::default().add_modifier(style_add));
                match aligns[i] {
                    TableAlign::Left => {
                        spans.push(inner);
                        if pad > 0 {
                            spans.push(Span::raw(" ".repeat(pad)));
                        }
                    }
                    TableAlign::Right => {
                        if pad > 0 {
                            spans.push(Span::raw(" ".repeat(pad)));
                        }
                        spans.push(inner);
                    }
                    TableAlign::Center => {
                        let left = pad / 2;
                        let right = pad - left;
                        if left > 0 {
                            spans.push(Span::raw(" ".repeat(left)));
                        }
                        spans.push(inner);
                        if right > 0 {
                            spans.push(Span::raw(" ".repeat(right)));
                        }
                    }
                }
                if i + 1 == wrapped.len() {
                    spans.push(Span::styled(" │".to_string(), border));
                } else {
                    spans.push(Span::styled(" │ ".to_string(), border));
                }
            }
            out_rows.push(spans);
        }
        out_rows
    };

    let mut out = Vec::new();
    out.push(rule_row("┌", "┬", "┐"));
    out.extend(data_row(&styled[0], true));
    out.push(rule_row("├", "┼", "┤"));
    for row in styled.iter().skip(1) {
        out.extend(data_row(row, false));
    }
    out.push(rule_row("└", "┴", "┘"));
    Some((out, consumed))
}

/// Word-wrap plain text to `width` columns (wide-char aware). Overlong
/// words hard-break. Always returns at least one line.
pub(crate) fn wrap_cell_text(s: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    let width = width.max(1);
    let push_chunks = |word: &str, out: &mut Vec<String>| {
        let mut cur = String::new();
        let mut cur_w = 0;
        for c in word.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if cur_w + cw > width && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(c);
            cur_w += cw;
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    };
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for word in s.split_whitespace() {
        let ww = word.width();
        if ww > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            push_chunks(word, &mut lines);
            continue;
        }
        if cur_w == 0 {
            cur.push_str(word);
            cur_w = ww;
        } else if cur_w + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + ww;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = ww;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

pub(crate) fn is_code_keyword(w: &str) -> bool {
    matches!(
        w,
        "fn" | "let"
            | "mut"
            | "const"
            | "var"
            | "pub"
            | "struct"
            | "enum"
            | "impl"
            | "trait"
            | "if"
            | "else"
            | "elif"
            | "match"
            | "for"
            | "while"
            | "loop"
            | "return"
            | "break"
            | "continue"
            | "def"
            | "class"
            | "import"
            | "from"
            | "as"
            | "with"
            | "async"
            | "await"
            | "yield"
            | "try"
            | "except"
            | "raise"
            | "function"
            | "export"
            | "extends"
            | "super"
            | "this"
            | "new"
            | "using"
            | "namespace"
            | "template"
            | "typename"
            | "virtual"
            | "override"
            | "public"
            | "private"
            | "protected"
            | "static"
            | "inline"
            | "include"
            | "define"
            | "ifdef"
            | "endif"
            | "int"
            | "float"
            | "double"
            | "char"
            | "bool"
            | "void"
            | "string"
            | "String"
            | "Vec"
            | "Option"
            | "Result"
            | "self"
            | "Self"
            | "crate"
            | "mod"
            | "use"
    )
}

pub(crate) fn highlighted_code_spans(line: &str) -> Vec<Span<'static>> {
    let base = crate::theme::Theme::code_block();

    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("#") || trimmed.starts_with("--") {
        return vec![Span::styled(
            line.to_string(),
            crate::theme::Theme::comment(),
        )];
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut in_str: Option<char> = None;
    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>, in_str: Option<char>| {
        if buf.is_empty() {
            return;
        }
        let w = buf.clone();
        buf.clear();
        if in_str.is_some() {
            spans.push(Span::styled(
                w,
                base.patch(Style::default().fg(Color::Green)),
            ));
        } else if is_code_keyword(&w) {
            spans.push(Span::styled(
                w,
                base.patch(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ));
        } else if w.chars().all(|c| c.is_ascii_digit()) {
            spans.push(Span::styled(
                w,
                base.patch(Style::default().fg(Color::Cyan)),
            ));
        } else {
            spans.push(Span::styled(w, base));
        }
    };
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            buf.push(c);
            if c == q && (i == 0 || chars[i - 1] != '\\') {
                flush(&mut buf, &mut spans, in_str);
                in_str = None;
            }
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' || c == '`' {
            flush(&mut buf, &mut spans, None);
            in_str = Some(c);
            buf.push(c);
            i += 1;
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            buf.push(c);
        } else {
            flush(&mut buf, &mut spans, None);

            spans.push(Span::styled(c.to_string(), base));
        }
        i += 1;
    }
    flush(&mut buf, &mut spans, in_str);
    if spans.is_empty() {
        spans.push(Span::styled(line.to_string(), base));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrap_cell_text_wraps_and_breaks() {
        assert_eq!(wrap_cell_text("", 10), vec!["".to_string()]);
        assert_eq!(wrap_cell_text("hi", 10), vec!["hi".to_string()]);
        assert_eq!(
            wrap_cell_text("alpha beta gamma", 10),
            vec!["alpha beta".to_string(), "gamma".to_string()]
        );
        assert_eq!(
            wrap_cell_text("supercalifragilistic", 5),
            vec![
                "super".to_string(),
                "calif".to_string(),
                "ragil".to_string(),
                "istic".to_string()
            ]
        );
        let wide = wrap_cell_text("日本語 テスト", 4);
        assert!(wide.iter().all(|l| l.width() <= 4), "{wide:?}");
        assert_eq!(wrap_cell_text("a  b", 10), vec!["a b".to_string()]);
    }

    #[test]
    fn table_long_cells_wrap_instead_of_clipping() {
        let lines = vec![
            "| Detail |",
            "|---|",
            "| read truncates at 2000 lines but keeps every byte for the model |",
        ];
        let (rows, consumed) = render_table_block(&lines, 40).expect("table parses");
        assert_eq!(consumed, 3);
        assert!(rows.len() > 4, "wrapped continuation rows exist");
        let text: String = rows
            .iter()
            .flat_map(|r| r.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("read truncates"), "head visible");
        assert!(text.contains("for the model"), "tail visible, not clipped");
        assert!(!text.contains('…'), "no clipping ellipsis");
        let widths: std::collections::HashSet<usize> = rows
            .iter()
            .map(|r| r.iter().map(|s| s.content.width()).sum())
            .collect();
        assert_eq!(widths.len(), 1, "wrapped rows stay aligned");
    }
    #[test]
    fn table_parser_detects_alignments() {
        let lines = vec![
            "| ✅ Pros | ⚠️ Cons |",
            "|---|---|",
            "| Single codebase for all platforms | Larger app size than native |",
            "| Near-native performance (AOT compiled) | Less mature ecosystem for desktop/web |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 120).expect("table parses");
        assert_eq!(consumed, 4);

        assert_eq!(rendered.len(), 6);

        let widths: Vec<usize> = rendered
            .iter()
            .map(|spans| spans.iter().map(|s| s.content.width()).sum())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "all table rows same width: {widths:?}"
        );

        let header_text: String = rendered[1].iter().map(|s| s.content.as_ref()).collect();
        assert!(header_text.contains("Pros") && header_text.contains("Cons"));
        let bold_cells = rendered[1]
            .iter()
            .filter(|s| {
                !s.content.trim().is_empty() && s.style.remove_modifier(Modifier::BOLD) != s.style
            })
            .count();
        assert!(bold_cells > 0, "header cells carry BOLD");
    }

    #[test]
    fn table_merges_surplus_pipes_into_last_column() {
        let lines = vec![
            "| Param | Eq |",
            "|---|---|",
            "| Start-up | |T| > 1 |",
            "| Gain | Av |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 80).expect("table parses");
        assert_eq!(consumed, 4);
        let text: String = rendered
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("|T|") && text.contains("> 1"), "{text:?}");

        let bars: usize = rendered[3]
            .iter()
            .map(|s| s.content.matches('│').count())
            .sum();
        assert_eq!(bars, 3, "{text:?}");
    }

    #[test]
    fn table_alignments_and_escapes() {
        assert_eq!(parse_delim_cell("---"), Some(TableAlign::Left));
        assert_eq!(parse_delim_cell(":---"), Some(TableAlign::Left));
        assert_eq!(parse_delim_cell("---:"), Some(TableAlign::Right));
        assert_eq!(parse_delim_cell(":--:"), Some(TableAlign::Center));
        assert_eq!(parse_delim_cell("abc"), None);
        assert_eq!(parse_delim_cell(":|"), None);

        assert_eq!(
            split_table_row("| a \\| b | c |"),
            Some(vec!["a | b".to_string(), "c".to_string()])
        );

        assert_eq!(
            split_table_row("a | b"),
            Some(vec!["a".to_string(), "b".to_string()])
        );

        let lines = vec!["| a | b |", "| c | d |"];
        assert!(render_table_block(&lines, 120).is_none());

        let lines = vec!["| n |", "|--:|", "| 42 |"];
        let (rendered, consumed) = render_table_block(&lines, 120).expect("table parses");
        assert_eq!(consumed, 3);
        let body: String = rendered[3].iter().map(|s| s.content.as_ref()).collect();
        assert!(body.ends_with("42 │"), "right-aligned: {body:?}");
    }

    #[test]
    fn table_with_math_cell_renders_translated() {
        let lines = vec![
            "| Param | Value |",
            "|---|---|",
            "| Loop gain | \\[ T = Av \\frac{C1}{C1 + C2} \\] |",
            "| Bare | Av \\approx -gm R_C |",
        ];
        let (rendered, consumed) = render_table_block(&lines, 80).expect("table parses");
        assert_eq!(consumed, 4);
        let text: String = rendered
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!text.contains("\\["), "{text:?}");
        assert!(!text.contains("\\approx"), "{text:?}");
        assert!(!text.contains("\\frac"), "{text:?}");
        assert!(text.contains("(C1)/(C1 + C2)"), "{text:?}");
        assert!(text.contains("Av ≈ -gm R_C"), "{text:?}");
    }

    #[test]
    fn list_marker_and_title_detection() {
        assert_eq!(
            split_list_marker("- item"),
            Some(("-".to_string(), "item".to_string()))
        );
        assert_eq!(
            split_list_marker("2. **Declarative UI**"),
            Some(("2.".to_string(), "**Declarative UI**".to_string()))
        );
        assert_eq!(split_list_marker("3.14 is pi"), None);
        assert_eq!(split_list_marker("word - x"), None);
        assert_eq!(
            split_list_marker("- "),
            Some(("-".to_string(), "".to_string()))
        );
        assert!(is_section_title("**Declarative UI**"));
        assert!(!is_section_title("plain item"));
        assert!(!is_section_title("**unclosed"));
    }

    #[test]
    fn user_inductor_snippet_renders_clean() {
        let lines = vec![
            "\\[",
            "  I{L,\\text{peak}} = IL + \\frac{Δ I_L}{2}, \\qquad",
            "  I{L,\\text{valley}} = IL - \\frac{Δ I_L}{2}",
            "\\]",
        ];
        let (rows, consumed) = render_math_block(&lines, 60).expect("block parses");
        assert_eq!(consumed, 4);
        assert_eq!(rows.len(), 8, "two stacked fractions, four rows each");
        let text: String = rows
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(!text.contains('\\'), "{text:?}");
        assert!(text.contains("IL,peak"), "{text:?}");
        assert!(text.contains("IL,valley"), "{text:?}");
        // Capital-L subscript has no unicode form: stacked 2D fallback.
        assert!(text.contains('Δ'), "{text:?}");
        assert!(text.contains('─'), "{text:?}");
    }

    #[test]
    fn wrap_preserving_rows_fit_and_chain() {
        use unicode_width::UnicodeWidthStr;
        let line = Line::from("aaa bbbb cc dddddddd ee");
        let rows = wrap_line_preserving(&line, 10);
        assert!(rows.len() >= 3, "wraps: {rows:?}");
        for (spans, _) in &rows {
            let w: usize = spans.iter().map(|s| s.content.width()).sum();
            assert!(w <= 10, "row fits: {w}");
        }

        let total: usize = "aaa bbbb cc dddddddd ee".width();
        let last = rows.last().unwrap();
        let last_w: usize = last.0.iter().map(|s| s.content.width()).sum();
        assert_eq!(last.1 + last_w, total);

        let joined: String = rows
            .iter()
            .map(|(spans, _)| spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined, "aaa bbbb cc dddddddd ee");
        let tabbed = wrap_line_preserving(&Line::from("ab\tcd"), 20);
        let t: String = tabbed[0].0.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(t, "ab  cd", "tab expands to 4-stop");
    }

    #[test]
    fn wrapped_drag_copy_extracts_right_text() {
        let lines = vec![Line::from("0123456789abcdef"), Line::from("gh")];

        let mut rows: Vec<(usize, usize)> = Vec::new();
        for (li, line) in lines.iter().enumerate() {
            for (_, col) in wrap_line_preserving(line, 10) {
                rows.push((li, col));
            }
        }
        assert_eq!(rows, vec![(0, 0), (0, 10), (1, 0)]);

        let text = extract_selection_text(
            &lines,
            SelPos { line: 0, col: 10 },
            SelPos { line: 0, col: 16 },
        );
        assert_eq!(text, "abcdef");
    }

    #[test]
    fn selection_orders_and_caret() {
        let a = SelPos { line: 5, col: 3 };
        let b = SelPos { line: 2, col: 9 };
        let sel = Selection {
            anchor: a,
            cursor: b,
            session: "s".into(),
            msg_len: 0,
        };
        assert_eq!(sel.ordered(), (b, a));
        assert!(!sel.is_caret());
        let caret = Selection {
            anchor: a,
            cursor: a,
            session: "s".into(),
            msg_len: 0,
        };
        assert!(caret.is_caret());
    }

    #[test]
    fn highlight_splits_spans_and_keeps_styles() {
        let line = Line::from(vec![
            Span::raw("hello "),
            Span::styled(
                "world".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ]);
        let sel_style = Style::default().bg(Color::Yellow);
        let out = highlight_range(&line.spans, 3, 8, sel_style);
        let text: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "hello world");

        let mid: String = out
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(mid, "lo wo");

        assert!(out
            .iter()
            .any(|s| s.content.as_ref() == "wo"
                && s.style.remove_modifier(Modifier::BOLD) != s.style));
    }

    #[test]
    fn highlight_snaps_wide_chars_whole() {
        let line = Line::from(vec![Span::raw("abＡcd")]);
        let out = highlight_range(&line.spans, 3, 4, Style::default().bg(Color::Yellow));
        let mid: String = out
            .iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(mid, "Ａ");
    }

    #[test]
    fn extract_text_joins_partial_rows() {
        let lines = vec![
            Line::from("0123456789"),
            Line::from("abcdefghij"),
            Line::from("0123456789"),
        ];
        let text = extract_selection_text(
            &lines,
            SelPos { line: 0, col: 8 },
            SelPos { line: 2, col: 2 },
        );
        assert_eq!(text, "89\nabcdefghij\n01");
    }

    #[test]
    fn fence_rules_align_and_code_bg_uniform() {
        let open = fence_open_spans("bash", 20);
        let close = fence_close_spans(20);
        let open_w: usize = open.iter().map(|s| s.content.width()).sum();
        let close_w: usize = close.iter().map(|s| s.content.width()).sum();
        assert_eq!(open_w, close_w, "fence top/bottom rules align");
        assert_eq!(open_w, 24, "inner + 4 border cols");

        let body = code_body_spans("echo hi", 20);
        let body_w: usize = body.iter().map(|s| s.content.width()).sum();
        assert_eq!(body_w, open_w, "body row matches rules");
        let text: String = body.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.starts_with("│ ") && text.ends_with(" │"),
            "closed box: {text:?}"
        );

        let code_bg = Theme::code_block().bg;
        for line in [
            "# a comment",
            "export PATH=\"$PATH:/x\"",
            "flutter run",
            "let x = \"s\"; // done",
        ] {
            for s in highlighted_code_spans(line) {
                assert_eq!(s.style.bg, code_bg, "uniform bg for {line:?}");
            }
        }
    }

    #[test]
    fn fence_block_widths_size_to_content_and_clamp() {
        let lines = vec![
            "```",
            "hi",
            "a much longer body line here",
            "```",
            "```rust",
            "```",
        ];
        let widths = fence_block_widths(&lines, 100);
        assert_eq!(widths[&0], "a much longer body line here".width());
        // Empty block still gets a usable minimum.
        assert_eq!(widths[&4], FENCE_MIN_INNER);
        // Clamped to the available maximum; below-minimum max stays robust.
        let widths = fence_block_widths(&lines, 20);
        assert_eq!(widths[&0], 20);
        assert_eq!(fence_block_widths(&lines, 4)[&0], FENCE_MIN_INNER);
        // Unclosed fence runs to the end.
        let open = vec!["```py", "print(12345678901234567890)"];
        assert_eq!(
            fence_block_widths(&open, 100)[&0],
            "print(12345678901234567890)".width()
        );
    }

    #[test]
    fn wrap_code_line_hard_breaks_wide_chars() {
        assert_eq!(wrap_code_line("abc", 10), vec!["abc"]);
        assert_eq!(wrap_code_line("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap_code_line("a\tb", 10), vec!["a    b"]);
        // Wide chars count double.
        assert_eq!(wrap_code_line("ab中d", 4), vec!["ab中", "d"]);
    }

    #[test]
    fn adjacent_inline_code_merges_into_one_badge() {
        let spans = markdown_inline_spans("Switch avg: `Isw_avg` = `IL_avg` * `D` done");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Switch avg: Isw_avg = IL_avg * D done");
        let code: Vec<&Span> = spans
            .iter()
            .filter(|s| s.style == Theme::inline_code())
            .collect();
        assert_eq!(code.len(), 1, "one badge: {spans:?}");
        assert_eq!(code[0].content.as_ref(), "Isw_avg = IL_avg * D");
    }

    #[test]
    fn inline_code_merge_keeps_words_split() {
        let spans = markdown_inline_spans("`a` and `b`");
        let code: Vec<&Span> = spans
            .iter()
            .filter(|s| s.style == Theme::inline_code())
            .collect();
        assert_eq!(code.len(), 2, "real words stay separate: {spans:?}");
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
    fn inline_math_strips_delimiters_and_styles() {
        let spans = markdown_inline_spans("Einstein: $E = mc^2$ ok");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains('$'), "delimiters stripped: {text}");
        assert!(text.contains("E = mc²"), "{text}");
        let math = spans
            .iter()
            .find(|s| s.content.contains("mc²"))
            .expect("math span present");
        assert_eq!(math.style, Theme::math());

        let spans = markdown_inline_spans(r"angle \(\theta = 90^\degree\) done");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("θ = 90^°"), "{text}");
        assert!(!text.contains('\\'), "{text}");

        let spans = markdown_inline_spans("costs $100 and $200 total");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "costs $100 and $200 total", "{text}");

        let spans = markdown_inline_spans("$a*b$");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "a*b", "{text}");
    }

    #[test]
    fn display_math_block_centers_and_consumes() {
        let lines = vec!["$$", r"\frac{1}{2} + \alpha", "$$"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("block parses");
        assert_eq!(consumed, 3);
        assert_eq!(rows.len(), 3, "2D fraction art");
        let body: Vec<&str> = rows.iter().map(|row| row[1].content.as_ref()).collect();
        assert_eq!(body, vec!["1", "─ + α", "2"], "{body:?}");
        // Block (width 5) centered in 40 cols: pad 17 on every row.
        for row in &rows {
            assert_eq!(row[0].content.as_ref(), &" ".repeat(17), "{row:?}");
        }

        let lines = vec!["$$E = mc^2$$"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("single-line");
        assert_eq!(consumed, 1);
        assert_eq!(rows.len(), 1, "single-row art stays one row");
        let text: String = rows[0].iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("E = mc²"), "{text:?}");

        let lines = vec![r"\[x &= 1 \\", r"y &= 2\]"];
        let (rows, consumed) = render_math_block(&lines, 40).expect("bracket form");
        assert_eq!(consumed, 2);
        assert_eq!(rows.len(), 2, "\\\\ splits equation lines");
        let text: String = rows
            .iter()
            .flat_map(|row| row.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("x = 1") && text.contains("y = 2"), "{text:?}");

        assert!(render_math_block(&["$$x + y"], 40).is_none());
        assert!(render_math_block(&["plain"], 40).is_none());
    }

    #[test]
    fn inline_math_ignores_code_spans_and_brackets() {
        let spans = markdown_inline_spans("`$HOME` and $x$");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "$HOME and x", "{text}");
        let code = spans
            .iter()
            .find(|s| s.content.contains("$HOME"))
            .expect("code span");
        assert_eq!(code.style, Theme::inline_code());

        let spans = markdown_inline_spans(r"gain \[|A_v| = g_m R_C\] ok");
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("|A_v| = g_m R_C"), "{text}");
        assert!(!text.contains("\\["), "{text}");
    }

    #[test]
    fn join_math_lines_handles_tables_and_fences() {
        let lines = vec![
            "| Loop gain | T = A_v | \\[",
            "  T = Av \\frac{C1}{C1 + C2}",
            "  \\] |",
            "| next | row | here |",
        ];
        let joined = join_math_lines(&lines);
        assert_eq!(joined.len(), 2, "{joined:?}");
        assert!(
            joined[0].contains("\\[") && joined[0].contains("\\]"),
            "{joined:?}"
        );

        let lines = vec!["```sh", "echo $$", "```", "after"];
        assert_eq!(join_math_lines(&lines).len(), 4);

        let lines = vec!["$$x$$", "plain $100"];
        assert_eq!(join_math_lines(&lines), vec!["$$x$$", "plain $100"]);

        let lines = vec!["text", "$$x + y"];
        assert_eq!(join_math_lines(&lines).len(), 2);
    }
}
