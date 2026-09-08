use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(crate) fn hit_rect(area: Rect, mx: u16, my: u16) -> bool {
    area.width > 0
        && area.height > 0
        && mx >= area.x
        && mx < area.x.saturating_add(area.width)
        && my >= area.y
        && my < area.y.saturating_add(area.height)
}

pub(crate) fn popup_list_visible(chrome: usize) -> usize {
    let rows = crossterm::terminal::size()
        .map(|(_, h)| h as usize)
        .unwrap_or(24);
    rows.saturating_mul(60)
        .saturating_div(100)
        .saturating_sub(2)
        .saturating_sub(chrome)
        .max(3)
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
