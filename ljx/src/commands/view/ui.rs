use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use super::types::Focus;

pub(super) fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(popup[1])[1]
}

pub(super) fn centered_rect_fixed_height(width_percent: u16, min_width: u16, height: u16, area: Rect) -> Rect {
    let width_percent = width_percent.min(100);
    let width = (area.width.saturating_mul(width_percent) / 100).max(min_width).min(area.width);
    let height = height.min(area.height);

    Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    )
}

pub(super) fn pane_block<'a>(title: &'a str, active: bool) -> Block<'a> {
    let title_style = if active { Style::default().fg(Color::Black).bg(Color::LightGreen) } else { Style::default().fg(Color::Gray) };
    let border_style = if active { Style::default().fg(Color::LightGreen) } else { Style::default().fg(Color::Gray) };

    Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(Style::default().fg(Color::White))
}

pub(super) fn status_help_spans(focus: Focus) -> Vec<Span<'static>> {
    match focus {
        Focus::Search => vec![
            status_key("TAB"),
            status_text(" switch  "),
            status_key("ENTER"),
            status_text(" apply  "),
            status_key("ESC"),
            status_text(" clear filter  "),
            status_key("UP/DOWN"),
            status_text(" change mode"),
        ],
        Focus::List => vec![
            status_key("Q"),
            status_text(" quit  "),
            status_key("TAB"),
            status_text(" switch  "),
            status_key("ENTER"),
            status_text(" open  "),
            status_key("S"),
            status_text(" save  "),
            status_key("E"),
            status_text(" export  "),
            status_key("D"),
            status_text(" dedup  "),
            status_key("T"),
            status_text(" tail  "),
            status_key("F"),
            status_text(" field filter  "),
            status_key("I"),
            status_text(" info  "),
            status_key("UP/DOWN"),
            status_text(" navigate"),
        ],
        Focus::Modal
        | Focus::FieldFilter
        | Focus::SavePrompt
        | Focus::SaveError
        | Focus::ExportPrompt
        | Focus::ExportError
        | Focus::DedupPrompt
        | Focus::DedupProgress => Vec::new(),
    }
}

pub(super) fn status_key(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::White).bg(Color::Indexed(30)).add_modifier(Modifier::BOLD))
}

pub(super) fn status_text(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::Black).bg(Color::Indexed(30)))
}

pub(super) fn draw_status_spans(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, width: u16, spans: &[Span<'static>]) {
    let mut cursor_x = x;
    let mut remaining = width;
    for span in spans {
        if remaining == 0 {
            break;
        }
        let next_x = buf.set_stringn(cursor_x, y, span.content.as_ref(), remaining as usize, span.style);
        remaining = remaining.saturating_sub(next_x.0.saturating_sub(cursor_x));
        cursor_x = next_x.0;
    }
}

pub(super) fn render_save_error_message(message: &str) -> Line<'static> {
    const PREFIX: &str = "File ";
    const SUFFIX: &str = " already exist";

    if let Some(filename) = message.strip_prefix(PREFIX).and_then(|rest| rest.strip_suffix(SUFFIX)) {
        return Line::from(vec![
            Span::styled(PREFIX, Style::default().fg(Color::White).bg(Color::Red)),
            Span::styled(filename.to_string(), Style::default().fg(Color::Yellow).bg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled(SUFFIX, Style::default().fg(Color::White).bg(Color::Red)),
        ]);
    }

    Line::from(Span::styled(message.to_string(), Style::default().fg(Color::White).bg(Color::Red)))
}
