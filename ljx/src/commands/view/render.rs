use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};

use super::detail::{
    fit_line, fit_modal_body, format_syslog_timestamp, modal_info_line, render_detail_lines, render_modal_footer, render_modal_footer_placeholder,
    render_modal_info_entries, severity_initial, severity_style,
};
use super::text::{fit_to_width, trim_single_line};
use super::types::{ExportField, Focus, ViewApp};
use super::ui::{
    centered_rect, centered_rect_fixed_height, draw_status_spans, pane_block, render_save_error_message, status_help_spans, status_key, status_text,
};

const DEDUP_PROMPT_POPUP_HEIGHT: u16 = 8;
const DEDUP_PROMPT_MIN_WIDTH: u16 = 58;
const DEDUP_PROGRESS_POPUP_HEIGHT: u16 = 6;
const DEDUP_PROGRESS_MIN_WIDTH: u16 = 52;
const EXPORT_PROMPT_POPUP_HEIGHT: u16 = 10;
const EXPORT_FORMAT_HELP: &str = "Format: use ←/→ or SPACE to choose (built-in + plugins)";
const EXPORT_RANGE_HELP: &str = "Range:  a / all  |  c / current / 0  |  N  |  N-N";
const EXPORT_ORDER_HELP: &str = "Uses the current filtered view order.";
const EXPORT_FORMAT_HELP_WIDTH: u16 = 55;
const EXPORT_PROMPT_MIN_WIDTH: u16 = EXPORT_FORMAT_HELP_WIDTH + 2;
const LIST_TIMESTAMP_WIDTH: usize = 15;
const LIST_SEVERITY_WIDTH: usize = 1;

impl ViewApp {
    pub(super) fn render(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(10), Constraint::Length(1)])
            .split(frame.area());

        self.render_search(frame, areas[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(if self.details_visible {
                vec![Constraint::Percentage(64), Constraint::Percentage(36)]
            } else {
                vec![Constraint::Percentage(100), Constraint::Length(0)]
            })
            .split(areas[1]);

        self.render_list(frame, body[0]);
        if self.details_visible {
            self.render_details(frame, body[1]);
        }
        self.render_status(frame, areas[2]);

        match self.focus {
            Focus::Modal => self.render_modal(frame),
            Focus::FieldFilter => self.render_field_filter(frame),
            Focus::SaveError => self.render_save_error(frame),
            Focus::ExportError => self.render_export_error(frame),
            Focus::SavePrompt => self.render_save_prompt(frame),
            Focus::ExportPrompt => self.render_export_prompt(frame),
            Focus::DedupPrompt => self.render_dedup_prompt(frame),
            Focus::DedupProgress => self.render_dedup_progress(frame),
            Focus::Search | Focus::List => {}
        }
    }

    fn render_search(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let title = match self.filter_mode {
            crate::predicate::FilterMode::Strings => "Filter (strings): ",
            crate::predicate::FilterMode::Regex => "Filter (regex): ",
        };
        let bar_style = Style::default().bg(Color::Indexed(30));
        let title_style = Style::default().fg(Color::LightCyan).bg(Color::Indexed(30)).add_modifier(Modifier::BOLD);
        let input_style = Style::default().fg(Color::White).bg(Color::Indexed(30));

        frame.buffer_mut().set_style(area, bar_style);
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(title, title_style), Span::styled(self.query_input.as_str(), input_style)])).style(bar_style),
            area,
        );

        if self.focus == Focus::Search {
            let x = area.x.saturating_add(title.chars().count() as u16).saturating_add(self.query_input.chars().count() as u16);
            let y = area.y;
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), y));
        }
    }

    fn render_list(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if area.height == 0 {
            return;
        }

        let visible_rows = area.height as usize;
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset.saturating_add(visible_rows) && visible_rows > 0 {
            self.list_offset = self.selected + 1 - visible_rows;
        }

        let mut lines = Vec::with_capacity(visible_rows.max(1));
        if self.entries.is_empty() {
            lines.push(Line::from(Span::styled(
                "No matches yet. Type a filter, press Enter, then browse the result set.",
                Style::default().fg(Color::Gray),
            )));
        } else {
            let end = (self.list_offset + visible_rows).min(self.entries.len());
            let row_width = area.width.saturating_sub(1) as usize;
            for index in self.list_offset..end {
                let row_style = if self.tail_mode && self.tail_marker_index == Some(index) {
                    Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD)
                } else if index == self.selected {
                    if self.focus == Focus::Search {
                        Style::default().fg(Color::Gray).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White).bg(Color::Indexed(28)).add_modifier(Modifier::BOLD)
                    }
                } else {
                    Style::default().fg(Color::White)
                };
                let summary = self
                    .summary_for(index)
                    .unwrap_or_else(|_| super::types::ListRowSummary { message: "<failed to render summary>".to_string(), severity: None });
                if self.details_visible {
                    lines.push(Line::from(Span::styled(fit_line(&summary.message, row_width), row_style)));
                } else {
                    lines.push(self.render_list_table_row(index, &summary, row_width, row_style));
                }
            }
        }

        let paragraph = Paragraph::new(Text::from(lines)).scroll((0, 0)).wrap(Wrap { trim: false }).style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, area);

        if !self.entries.is_empty() {
            let mut scrollbar_state = ScrollbarState::new(self.entries.len()).position(self.selected.min(self.entries.len().saturating_sub(1)));
            frame.render_stateful_widget(Scrollbar::new(ScrollbarOrientation::VerticalRight), area, &mut scrollbar_state);
        }
    }

    fn render_list_table_row(&self, index: usize, summary: &super::types::ListRowSummary, row_width: usize, row_style: Style) -> Line<'static> {
        let timestamp = format_syslog_timestamp(self.entries[index].ts_unix_ns);
        let timestamp = fit_to_width(&timestamp, LIST_TIMESTAMP_WIDTH);
        let severity = summary.severity.as_deref().unwrap_or("");
        let severity_style = severity_style(severity).bg(row_style.bg.unwrap_or(Color::Reset));
        let severity = fit_to_width(&severity_initial(severity), LIST_SEVERITY_WIDTH);
        let prefix_width = LIST_TIMESTAMP_WIDTH + 1 + LIST_SEVERITY_WIDTH + 2;
        let message_width = row_width.saturating_sub(prefix_width);

        Line::from(vec![
            Span::styled(timestamp, row_style.fg(Color::LightBlue)),
            Span::styled(" ", row_style),
            Span::styled(severity, severity_style),
            Span::styled("  ", row_style),
            Span::styled(fit_line(&summary.message, message_width), row_style),
        ])
    }

    fn render_details(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = pane_block(" Info ", false);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = if let Some(detail) = &self.selected_detail {
            render_detail_lines(detail, self.hex_payload)
        } else {
            vec![Line::from("No record selected yet.")]
        };

        let paragraph =
            Paragraph::new(Text::from(lines)).scroll((self.detail_scroll, 0)).wrap(Wrap { trim: false }).style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, inner);
    }

    fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let bar_style = Style::default().bg(Color::Indexed(30));
        let buf = frame.buffer_mut();
        buf.set_style(area, bar_style);
        let y = area.y;

        if self.tail_mode {
            draw_status_spans(buf, area.x, y, area.width, &[status_text("Tailing... Press any key to stop")]);
            return;
        }

        match self.focus {
            Focus::Modal => {
                draw_status_spans(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &[
                        status_key("ESC"),
                        status_text(" close   "),
                        status_key("UP/DOWN"),
                        status_text(" scroll   "),
                        status_key("LEFT/RIGHT"),
                        status_text(" prev/next   "),
                        status_key("I"),
                        status_text(" info panel   "),
                        status_key("T"),
                        status_text(" tail"),
                    ],
                );
                return;
            }
            Focus::SavePrompt => {
                draw_status_spans(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &[status_key("ENTER"), status_text(" save   "), status_key("ESC"), status_text(" cancel")],
                );
                return;
            }
            Focus::SaveError | Focus::ExportError => {
                draw_status_spans(buf, area.x, y, area.width, &[status_text("Press any key to return")]);
                return;
            }
            Focus::ExportPrompt => {
                draw_status_spans(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &[
                        status_key("TAB"),
                        status_text(" next field   "),
                        status_key("←/→"),
                        status_text(" format   "),
                        status_key("ENTER"),
                        status_text(" export   "),
                        status_key("ESC"),
                        status_text(" cancel"),
                    ],
                );
                return;
            }
            Focus::DedupPrompt => {
                draw_status_spans(
                    buf,
                    area.x,
                    y,
                    area.width,
                    &[
                        status_key("LEFT/RIGHT"),
                        status_text(" mode   "),
                        status_key("ENTER"),
                        status_text(" start   "),
                        status_key("ESC"),
                        status_text(" cancel"),
                    ],
                );
                return;
            }
            Focus::DedupProgress => {
                draw_status_spans(buf, area.x, y, area.width, &[status_text("Deduplicating…")]);
                return;
            }
            Focus::FieldFilter | Focus::Search | Focus::List => {}
        }

        let left_spans = status_help_spans(self.focus);
        let status = trim_single_line(&self.status, area.width as usize);
        let status_width = status.chars().count().min(area.width as usize) as u16;
        let gap_width = if area.width > status_width { 1 } else { 0 };
        let left_width = area.width.saturating_sub(status_width).saturating_sub(gap_width);
        draw_status_spans(buf, area.x, y, left_width, &left_spans);

        if status_width > 0 {
            let status_x = area.right().saturating_sub(status_width);
            buf.set_stringn(status_x, y, status, status_width as usize, Style::default().fg(Color::LightGreen).bg(Color::Indexed(30)));
        }
    }

    fn render_save_prompt(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(52, 10, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(" Save current content ", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::White).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let label = "Filename: ";
        let input_width = inner.width.saturating_sub(label.chars().count() as u16 + 2);
        let row = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(fit_to_width(&self.save_filename, input_width as usize), Style::default().fg(Color::Black).bg(Color::White)),
            ])),
            row,
        );
        let cursor_x = row
            .x
            .saturating_add(label.chars().count() as u16)
            .saturating_add(self.save_filename_cursor as u16)
            .min(row.x.saturating_add(label.chars().count() as u16 + input_width));
        frame.set_cursor_position((cursor_x, row.y));
    }

    fn render_save_error(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(38, 12, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(" Error ", Style::default().fg(Color::Red).bg(Color::White).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Red))
            .style(Style::default().fg(Color::White).bg(Color::Red));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if let Some(message) = &self.save_message {
            frame.render_widget(
                Paragraph::new(render_save_error_message(message)).style(Style::default().bg(Color::Red)).wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_export_prompt(&self, frame: &mut Frame<'_>) {
        let area = centered_rect_fixed_height(62, EXPORT_PROMPT_MIN_WIDTH, EXPORT_PROMPT_POPUP_HEIGHT, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(
                format!(" Export {} ", self.current_export_format().title()),
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let format_label = "Format:   ";
        let format_width = inner.width.saturating_sub(format_label.chars().count() as u16 + 2);
        let format_row = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        let format_style = if self.export_field == ExportField::Format {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(Color::Indexed(250))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format_label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(fit_to_width(&format!("< {} >", self.current_export_format().label()), format_width as usize), format_style),
            ])),
            format_row,
        );

        let filename_label = "Filename: ";
        let filename_width = inner.width.saturating_sub(filename_label.chars().count() as u16 + 2);
        let filename_row = Rect { x: inner.x, y: inner.y.saturating_add(2), width: inner.width, height: 1 };
        let filename_style = if self.export_field == ExportField::Filename {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::Black).bg(Color::Indexed(250))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(filename_label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(fit_to_width(&self.export_filename, filename_width as usize), filename_style),
            ])),
            filename_row,
        );

        let range_label = "Range:    ";
        let range_width = inner.width.saturating_sub(range_label.chars().count() as u16 + 2);
        let range_row = Rect { x: inner.x, y: inner.y.saturating_add(4), width: inner.width, height: 1 };
        let range_style = if self.export_field == ExportField::Range {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::Black).bg(Color::Indexed(250))
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(range_label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(fit_to_width(&self.export_range, range_width as usize), range_style),
            ])),
            range_row,
        );

        frame.render_widget(
            Paragraph::new(Text::from(vec![Line::from(EXPORT_FORMAT_HELP), Line::from(EXPORT_RANGE_HELP), Line::from(EXPORT_ORDER_HELP)]))
                .style(Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            Rect { x: inner.x, y: inner.y.saturating_add(5), width: inner.width, height: 3 },
        );

        let (cursor_x, cursor_y) = match self.export_field {
            ExportField::Format => (
                format_row
                    .x
                    .saturating_add(format_label.chars().count() as u16)
                    .saturating_add(2)
                    .min(format_row.x.saturating_add(format_label.chars().count() as u16 + format_width)),
                format_row.y,
            ),
            ExportField::Filename => (
                filename_row
                    .x
                    .saturating_add(filename_label.chars().count() as u16)
                    .saturating_add(self.export_filename_cursor as u16)
                    .min(filename_row.x.saturating_add(filename_label.chars().count() as u16 + filename_width)),
                filename_row.y,
            ),
            ExportField::Range => (
                range_row
                    .x
                    .saturating_add(range_label.chars().count() as u16)
                    .saturating_add(self.export_range_cursor as u16)
                    .min(range_row.x.saturating_add(range_label.chars().count() as u16 + range_width)),
                range_row.y,
            ),
        };
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn render_export_error(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(42, 12, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(" Export Error ", Style::default().fg(Color::Red).bg(Color::White).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Red))
            .style(Style::default().fg(Color::White).bg(Color::Red));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if let Some(message) = &self.export_message {
            frame.render_widget(
                Paragraph::new(render_save_error_message(message)).style(Style::default().bg(Color::Red)).wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_modal(&self, frame: &mut Frame<'_>) {
        let screen = frame.area();
        let message = self.modal_text.as_deref().unwrap_or("No record loaded.");
        let popup_width = (screen.width * 80 / 100).max(20);
        let inner_width = popup_width.saturating_sub(2);

        if !self.modal_info_visible {
            let (wrapped, left_lines) = fit_modal_body(message, inner_width.saturating_sub(1) as usize);
            let info_lines = self.selected_detail.as_ref().map(render_modal_info_entries).map(|v| v.len() as u16).unwrap_or(0);
            let popup_height = (left_lines + 3).max(info_lines + 3).min(screen.height * 80 / 100).max(5);
            let area =
                Rect::new(screen.width.saturating_sub(popup_width) / 2, screen.height.saturating_sub(popup_height) / 2, popup_width, popup_height);
            frame.render_widget(Clear, area);

            let block = Block::default()
                .title(Span::styled(" Log record ", Style::default().fg(Color::Black).bg(Color::Indexed(30)).add_modifier(Modifier::BOLD)))
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .border_style(Style::default().fg(Color::Black).bg(Color::Gray))
                .style(Style::default().fg(Color::Black).bg(Color::Gray));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            frame.render_widget(
                Paragraph::new(wrapped.as_str()).style(Style::default().fg(Color::Black).bg(Color::Gray)).scroll((self.modal_scroll, 0)),
                chunks[0],
            );

            let mut scrollbar_state = ScrollbarState::new(left_lines as usize).position(self.modal_scroll as usize);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(Color::Black).bg(Color::Gray)),
                chunks[0],
                &mut scrollbar_state,
            );

            let footer = self.selected_detail.as_ref().map(render_modal_footer).unwrap_or_else(render_modal_footer_placeholder);
            frame.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Blue)), chunks[1]);
            return;
        }

        let info_entries = self
            .selected_detail
            .as_ref()
            .map(render_modal_info_entries)
            .unwrap_or_else(|| vec![("info".to_string(), "No record loaded.".to_string())]);
        let key_width = info_entries.iter().map(|(key, _)| key.chars().count()).max().unwrap_or(4).max(4);
        let preferred_info_width =
            info_entries.iter().map(|(_, value)| (key_width + 2 + value.chars().count() + 1) as u16).max().unwrap_or((key_width + 3) as u16);
        let info_width = preferred_info_width.min(inner_width.saturating_div(2).max(16)).max(16);
        let divider_width = 1u16;
        let message_width = inner_width.saturating_sub(info_width).saturating_sub(divider_width);
        let (wrapped, left_lines) = fit_modal_body(message, message_width.saturating_sub(1) as usize);
        let popup_height = (left_lines.max(info_entries.len() as u16) + 3).min(screen.height * 80 / 100).max(5);
        let area = Rect::new(screen.width.saturating_sub(popup_width) / 2, screen.height.saturating_sub(popup_height) / 2, popup_width, popup_height);
        frame.render_widget(Clear, area);

        let block = Block::default()
            .title(Span::styled(" Log record ", Style::default().fg(Color::Black).bg(Color::Indexed(30)).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(1), Constraint::Length(1)]).split(inner);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(message_width), Constraint::Length(divider_width), Constraint::Length(info_width)])
            .split(chunks[0]);

        let divider =
            (0..body[1].height).map(|_| Line::from(Span::styled("│", Style::default().fg(Color::White).bg(Color::Indexed(30))))).collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(divider).style(Style::default().bg(Color::Indexed(30))), body[1]);
        frame.render_widget(
            Paragraph::new(wrapped.as_str()).style(Style::default().fg(Color::Black).bg(Color::Gray)).scroll((self.modal_scroll, 0)),
            body[0],
        );

        let mut scrollbar_state = ScrollbarState::new(left_lines as usize).position(self.modal_scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(Color::Black).bg(Color::Gray)),
            body[0],
            &mut scrollbar_state,
        );

        let value_width = info_width.saturating_sub((key_width + 3) as u16) as usize;
        let info_lines = info_entries.into_iter().map(|(key, value)| modal_info_line(&key, value, key_width, value_width)).collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(info_lines)).style(Style::default().fg(Color::White).bg(Color::Indexed(30))).scroll((0, 0)),
            body[2],
        );
        let footer = self.selected_detail.as_ref().map(render_modal_footer).unwrap_or_else(render_modal_footer_placeholder);
        frame.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Blue)), chunks[1]);

        let split_x = body[1].x;
        let grey_style = Style::default().fg(Color::Black).bg(Color::Gray);
        let cyan_style = Style::default().fg(Color::White).bg(Color::Indexed(30));
        let buf = frame.buffer_mut();
        let top = area.y;
        let bot = area.y + area.height.saturating_sub(1);
        let left = area.x;
        let right = area.x + area.width.saturating_sub(1);
        for x in left..=right {
            let style = if x >= split_x { cyan_style } else { grey_style };
            buf[(x, top)].set_style(style);
            buf[(x, bot)].set_style(style);
        }
        for y in top..=bot {
            buf[(left, y)].set_style(grey_style);
            buf[(right, y)].set_style(cyan_style);
        }
    }

    fn render_field_filter(&self, frame: &mut Frame<'_>) {
        let catalog = self.field_catalog.lock().unwrap();
        let Some(cat) = catalog.as_ref() else { return };
        let Some(state) = &self.field_filter_state else { return };

        let screen = frame.area();
        let filter_lower = state.filter_text.to_lowercase();
        let filtered_sev: Vec<&String> = if state.panel == 0 && !filter_lower.is_empty() {
            cat.severities.iter().filter(|s| s.to_lowercase().contains(&filter_lower)).collect()
        } else {
            cat.severities.iter().collect()
        };
        let filtered_svc: Vec<&String> = if state.panel == 1 && !filter_lower.is_empty() {
            cat.services.iter().filter(|s| s.to_lowercase().contains(&filter_lower)).collect()
        } else {
            cat.services.iter().collect()
        };

        let body_height = filtered_sev.len().max(filtered_svc.len()).max(1) as u16;
        let popup_h = (body_height + 4).clamp(20, screen.height * 60 / 100);
        let popup_w = (screen.width * 60 / 100).max(40);
        let area = Rect::new(screen.width.saturating_sub(popup_w) / 2, screen.height * 20 / 100, popup_w, popup_h);
        frame.render_widget(Clear, area);

        let title = if state.filter_text.is_empty() {
            vec![Span::styled(" Field Filter ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD))]
        } else {
            vec![
                Span::styled(" Field Filter ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(" [{}▏]", state.filter_text),
                    Style::default().fg(Color::LightYellow).bg(Color::Indexed(30)).add_modifier(Modifier::BOLD),
                ),
            ]
        };

        let block = Block::default()
            .title(Line::from(title))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            .style(Style::default().fg(Color::Black).bg(Color::Indexed(30)));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(Rect::new(inner.x, inner.y, inner.width, inner.height.saturating_sub(1)));

        let sev_title_style =
            if state.panel == 0 { Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Blue) };
        let mut sev_lines: Vec<Line<'_>> = vec![Line::from(Span::styled(" Severity", sev_title_style))];
        for (i, sev) in filtered_sev.iter().enumerate() {
            let checked = if state.selected_severities.contains(*sev) { "▣" } else { "☐" };
            let style = if state.panel == 0 && i == state.severity_cursor {
                Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Black)
            };
            sev_lines.push(Line::from(Span::styled(format!(" {checked} {sev}"), style)));
        }
        frame.render_widget(Paragraph::new(sev_lines).style(Style::default().bg(Color::Indexed(30))).scroll((state.severity_scroll, 0)), panels[0]);

        let svc_title_style =
            if state.panel == 1 { Style::default().fg(Color::LightBlue).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Blue) };
        let mut svc_lines: Vec<Line<'_>> = vec![Line::from(Span::styled(" Services", svc_title_style))];
        for (i, svc) in filtered_svc.iter().enumerate() {
            let checked = if state.selected_services.contains(*svc) { "▣" } else { "☐" };
            let style = if state.panel == 1 && i == state.service_cursor {
                Style::default().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Black)
            };
            svc_lines.push(Line::from(Span::styled(format!(" {checked} {svc}"), style)));
        }
        frame.render_widget(Paragraph::new(svc_lines).style(Style::default().bg(Color::Indexed(30))).scroll((state.service_scroll, 0)), panels[1]);

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("SPACE", Style::default().fg(Color::LightYellow).bg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::styled(" toggle  ", Style::default().fg(Color::White).bg(Color::Blue)),
                Span::styled("TAB", Style::default().fg(Color::LightYellow).bg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::styled(" switch  ", Style::default().fg(Color::White).bg(Color::Blue)),
                Span::styled("ENTER", Style::default().fg(Color::LightYellow).bg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::styled(" apply  ", Style::default().fg(Color::White).bg(Color::Blue)),
                Span::styled("ESC", Style::default().fg(Color::LightYellow).bg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::styled(" cancel  ", Style::default().fg(Color::White).bg(Color::Blue)),
                Span::styled("type", Style::default().fg(Color::LightYellow).bg(Color::Blue).add_modifier(Modifier::BOLD)),
                Span::styled(" to search", Style::default().fg(Color::White).bg(Color::Blue)),
            ]))
            .style(Style::default().bg(Color::Blue)),
            Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 },
        );
    }

    fn render_dedup_prompt(&self, frame: &mut Frame<'_>) {
        let area = centered_rect_fixed_height(52, DEDUP_PROMPT_MIN_WIDTH, DEDUP_PROMPT_POPUP_HEIGHT, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(" Deduplicate ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let label = "Output: ";
        let input_width = inner.width.saturating_sub(label.chars().count() as u16 + 2);
        let row = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(fit_to_width(&self.dedup_filename, input_width as usize), Style::default().fg(Color::Black).bg(Color::White)),
            ])),
            row,
        );
        let cursor_x = row
            .x
            .saturating_add(label.chars().count() as u16)
            .saturating_add(1)
            .saturating_add(self.dedup_filename.chars().count() as u16)
            .min(row.x.saturating_add(label.chars().count() as u16 + input_width));
        frame.set_cursor_position((cursor_x, row.y));

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Mode:   ", Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(self.dedup_behavior.label(), Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", self.dedup_behavior.description()), Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            Rect { x: inner.x, y: inner.y.saturating_add(2), width: inner.width, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Match:  ", Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(self.dedup_match_mode.label(), Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", self.dedup_match_mode.description()), Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            Rect { x: inner.x, y: inner.y.saturating_add(3), width: inner.width, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("←/→", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(" mode   ", Style::default().fg(Color::DarkGray).bg(Color::Gray)),
                Span::styled("↑/↓", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(" match   ", Style::default().fg(Color::DarkGray).bg(Color::Gray)),
                Span::styled("ENTER", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(" start   ", Style::default().fg(Color::DarkGray).bg(Color::Gray)),
                Span::styled("ESC", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(" cancel", Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            Rect { x: inner.x, y: inner.y.saturating_add(5).min(inner.y + inner.height.saturating_sub(1)), width: inner.width, height: 1 },
        );
    }

    fn render_dedup_progress(&self, frame: &mut Frame<'_>) {
        let area = centered_rect_fixed_height(52, DEDUP_PROGRESS_MIN_WIDTH, DEDUP_PROGRESS_POPUP_HEIGHT, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(" Deduplicating… ", Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let pct = (self.dedup_progress * 100.0).min(100.0);
        let label = self.dedup_completion_message.clone().unwrap_or_else(|| format!("{pct:.0}%"));
        let label_style = if self.dedup_completion_message.is_some() {
            Style::default().fg(Color::LightYellow).bg(Color::Indexed(28)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        };
        frame.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(Color::Indexed(28)).bg(Color::White))
                .label(Span::styled(label, label_style))
                .ratio(self.dedup_progress.clamp(0.0, 1.0)),
            Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: 1 },
        );
        let phase_text = if self.dedup_completion_message.is_some() { "Press ENTER to open the deduped file" } else { self.dedup_phase.as_str() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Phase: ", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(phase_text, Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            Rect { x: inner.x + 1, y: inner.y + 3, width: inner.width.saturating_sub(2), height: 1 },
        );
    }
}
