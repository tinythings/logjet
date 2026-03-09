use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use logjet::{LogjetReader, LogjetWriter, OwnedRecord, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::{Frame, Terminal};

use crate::cli::ViewArgs;
use crate::error::{Error, Result};
use crate::input::InputHandle;
use crate::predicate::{FilterMode, parse_filter_query};

const SUMMARY_CACHE_LIMIT: usize = 256;
const DETAIL_PREVIEW_BYTES: usize = 1024;
const SCAN_BATCH_SIZE: usize = 128;
const TICK_RATE: Duration = Duration::from_millis(100);

pub fn run(args: ViewArgs) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(Error::Usage(
            "ljx view needs an interactive terminal; pipe-oriented output belongs in `ljx filter`".to_string(),
        ));
    }

    let mut stdout = io::stdout();
    enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = ViewApp::new(args)?;
    app.apply_filter()?;
    let outcome = app.run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    outcome
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Search,
    List,
    Modal,
    SavePrompt,
    SaveError,
}

#[derive(Debug, Clone, Copy)]
struct EntryMeta {
    offset: u64,
    record_type: RecordType,
    seq: u64,
    ts_unix_ns: u64,
    payload_len: u64,
}

#[derive(Debug, Clone)]
struct DetailRecord {
    meta: EntryMeta,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
enum ScanUpdate {
    Batch(Vec<EntryMeta>),
    Finished {
        scanned: u64,
        matched: u64,
    },
    Failed(String),
}

struct ActiveScan {
    rx: Receiver<ScanUpdate>,
    cancel: Arc<AtomicBool>,
    spool_path: PathBuf,
    spool_reader: File,
    scanned: u64,
    matched: u64,
    finished: bool,
}

impl ActiveScan {
    fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct ViewApp {
    input: PathBuf,
    hex_payload: bool,
    focus: Focus,
    filter_mode: FilterMode,
    query_input: String,
    applied_query: String,
    status: String,
    entries: Vec<EntryMeta>,
    selected: usize,
    list_offset: usize,
    modal_scroll: u16,
    detail_scroll: u16,
    summary_cache: HashMap<usize, String>,
    summary_order: VecDeque<usize>,
    selected_detail: Option<DetailRecord>,
    modal_text: Option<String>,
    save_filename: String,
    save_message: Option<String>,
    current_scan: Option<ActiveScan>,
}

impl ViewApp {
    fn new(args: ViewArgs) -> Result<Self> {
        Ok(Self {
            input: args.input,
            hex_payload: args.hex_payload,
            focus: Focus::Search,
            filter_mode: FilterMode::Strings,
            query_input: String::new(),
            applied_query: String::new(),
            status: "Type a filter and press Enter to scan matching records".to_string(),
            entries: Vec::new(),
            selected: 0,
            list_offset: 0,
            modal_scroll: 0,
            detail_scroll: 0,
            summary_cache: HashMap::new(),
            summary_order: VecDeque::new(),
            selected_detail: None,
            modal_text: None,
            save_filename: String::new(),
            save_message: None,
            current_scan: None,
        })
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            self.drain_scan_updates()?;
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(TICK_RATE)? {
                let Event::Key(key) = event::read()? else {
                    continue;
                };

                if self.handle_key(key)? {
                    return Ok(());
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if !matches!(self.focus, Focus::Modal | Focus::SavePrompt | Focus::SaveError)
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
        {
            self.cancel_scan();
            return Ok(true);
        }

        match self.focus {
            Focus::Modal => self.handle_modal_key(key),
            Focus::SavePrompt => self.handle_save_prompt_key(key),
            Focus::SaveError => self.handle_save_error_key(),
            Focus::Search => self.handle_search_key(key),
            Focus::List => self.handle_list_key(key),
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.modal_text = None;
                self.modal_scroll = 0;
            }
            KeyCode::Up => {
                self.modal_scroll = self.modal_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.modal_scroll = self.modal_scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.modal_scroll = self.modal_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.modal_scroll = self.modal_scroll.saturating_add(10);
            }
            _ => {}
        }

        Ok(false)
    }

    fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Tab => {
                self.focus = Focus::List;
            }
            KeyCode::Up | KeyCode::Down => {
                self.cycle_filter_mode();
            }
            KeyCode::Esc => {
                self.query_input.clear();
                self.apply_filter()?;
            }
            KeyCode::Enter => {
                self.apply_filter()?;
            }
            KeyCode::Backspace => {
                self.query_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query_input.clear();
            }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.query_input.push(ch);
            }
            _ => {}
        }

        Ok(false)
    }

    fn handle_save_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.save_message = None;
            }
            KeyCode::Enter => {
                self.save_current_results()?;
            }
            KeyCode::Backspace => {
                self.save_filename.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_filename.clear();
            }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.save_filename.push(ch);
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_save_error_key(&mut self) -> Result<bool> {
        self.focus = Focus::SavePrompt;
        self.save_message = None;
        Ok(false)
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Tab => {
                self.focus = Focus::Search;
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.open_save_prompt()?;
            }
            KeyCode::Up => {
                self.move_selection(-1)?;
            }
            KeyCode::Down => {
                self.move_selection(1)?;
            }
            KeyCode::PageUp => {
                self.move_selection(-10)?;
            }
            KeyCode::PageDown => {
                self.move_selection(10)?;
            }
            KeyCode::Home => {
                self.selected = 0;
                self.list_offset = 0;
                self.refresh_selected_detail()?;
            }
            KeyCode::End => {
                if !self.entries.is_empty() {
                    self.selected = self.entries.len() - 1;
                    self.refresh_selected_detail()?;
                }
            }
            KeyCode::Enter => {
                self.open_modal()?;
            }
            _ => {}
        }

        Ok(false)
    }

    fn move_selection(&mut self, delta: isize) -> Result<()> {
        if self.entries.is_empty() {
            return Ok(());
        }

        let max = self.entries.len().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, max) as usize;
        if next != self.selected {
            self.selected = next;
            self.refresh_selected_detail()?;
        }

        Ok(())
    }

    fn apply_filter(&mut self) -> Result<()> {
        if let Some(scan) = self.current_scan.take() {
            scan.cancel();
            drop(scan.spool_reader);
            let _ = std::fs::remove_file(scan.spool_path);
        }
        self.entries.clear();
        self.summary_cache.clear();
        self.summary_order.clear();
        self.selected = 0;
        self.list_offset = 0;
        self.modal_scroll = 0;
        self.detail_scroll = 0;
        self.selected_detail = None;
        self.modal_text = None;
        self.applied_query = self.query_input.clone();
        self.focus = Focus::List;
        let predicate = parse_filter_query(&self.applied_query, self.filter_mode)?;

        let spool_path = create_temp_path()?;
        let spool_reader = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&spool_path)?;
        let spool_writer = spool_reader.try_clone()?;
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let input = self.input.clone();
        let (tx, rx) = mpsc::channel();
        let tx_worker = tx.clone();

        thread::spawn(move || {
            let result = scan_matches(input.as_path(), predicate, spool_writer, cancel_worker, tx_worker.clone());
            match result {
                Ok((scanned, matched)) => {
                    let _ = tx_worker.send(ScanUpdate::Finished { scanned, matched });
                }
                Err(err) => {
                    let _ = tx_worker.send(ScanUpdate::Failed(err.to_string()));
                }
            }
        });

        self.status = format!("Scanning matches for {:?}", self.applied_query);
        self.current_scan = Some(ActiveScan {
            rx,
            cancel,
            spool_path,
            spool_reader,
            scanned: 0,
            matched: 0,
            finished: false,
        });

        Ok(())
    }

    fn cycle_filter_mode(&mut self) {
        self.filter_mode = match self.filter_mode {
            FilterMode::Strings => FilterMode::Regex,
            FilterMode::Regex => FilterMode::Strings,
        };
    }

    fn open_save_prompt(&mut self) -> Result<()> {
        let Some(scan) = &self.current_scan else {
            self.status = "No active scan to save.".to_string();
            return Ok(());
        };
        if !scan.finished {
            self.status = "Wait for the scan to finish before saving.".to_string();
            return Ok(());
        }
        self.save_message = None;
        if self.save_filename.is_empty() {
            self.save_filename = "filtered.logjet".to_string();
        }
        self.focus = Focus::SavePrompt;
        Ok(())
    }

    fn save_current_results(&mut self) -> Result<()> {
        let filename = self.save_filename.trim();
        if filename.is_empty() {
            self.save_message = Some("Filename must not be empty.".to_string());
            return Ok(());
        }
        if filename.contains('/') {
            self.save_message = Some("Filename must not contain path separators.".to_string());
            return Ok(());
        }
        if self.input == Path::new("-") {
            self.save_message = Some("Cannot infer output directory when input is stdin.".to_string());
            return Ok(());
        }

        let Some(scan) = &mut self.current_scan else {
            self.save_message = Some("No scan data to save.".to_string());
            return Ok(());
        };
        let output_dir = self
            .input
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let output_path = output_dir.join(filename);
        if output_path == self.input || output_path.exists() {
            self.save_message = Some(format!("File {filename} already exist"));
            self.focus = Focus::SaveError;
            return Ok(());
        }

        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)?;
        let writer = BufWriter::new(file);
        let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
        for meta in &self.entries {
            let detail = read_spool_record(&mut scan.spool_reader, *meta)?;
            logjet.push(
                detail.meta.record_type,
                detail.meta.seq,
                detail.meta.ts_unix_ns,
                &detail.payload,
            )?;
        }
        let mut writer = logjet.into_inner()?;
        writer.flush()?;

        self.focus = Focus::List;
        self.save_message = None;
        self.status = format!("Saved {} records to {}", self.entries.len(), output_path.display());
        Ok(())
    }

    fn drain_scan_updates(&mut self) -> Result<()> {
        let Some(scan) = &self.current_scan else {
            return Ok(());
        };

        let mut updates = Vec::new();
        while let Ok(update) = scan.rx.try_recv() {
            updates.push(update);
        }

        let mut finished = false;
        let mut should_refresh_selection = false;
        let mut status_override = None;
        {
            let Some(scan) = &mut self.current_scan else {
                return Ok(());
            };
            for update in updates {
                match update {
                    ScanUpdate::Batch(batch) => {
                        self.entries.extend(batch);
                        scan.matched = self.entries.len() as u64;
                        if self.selected_detail.is_none() && !self.entries.is_empty() {
                            should_refresh_selection = true;
                        }
                    }
                    ScanUpdate::Finished { scanned, matched } => {
                        scan.scanned = scanned;
                        scan.matched = matched;
                        scan.finished = true;
                        finished = true;
                        status_override =
                            Some(format!("Scan complete: {matched} matches out of {scanned} records"));
                    }
                    ScanUpdate::Failed(message) => {
                        scan.finished = true;
                        finished = true;
                        status_override = Some(format!("Scan failed: {message}"));
                    }
                }
            }
        }

        if should_refresh_selection {
            self.refresh_selected_detail()?;
        }

        if let Some(status) = status_override {
            self.status = status;
        }

        if !finished {
            let matched = self.entries.len();
            self.status = if self.applied_query.is_empty() {
                format!("Scanning all records: {matched} matches buffered")
            } else {
                format!("Scanning {:?}: {matched} matches buffered", self.applied_query)
            };
        }

        Ok(())
    }

    fn refresh_selected_detail(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            self.selected_detail = None;
            return Ok(());
        }

        let record = self.load_record(self.selected)?;
        self.selected_detail = Some(record);
        self.detail_scroll = 0;
        Ok(())
    }

    fn load_record(&mut self, index: usize) -> Result<DetailRecord> {
        let Some(scan) = &mut self.current_scan else {
            return Err(Error::Usage("no active scan".to_string()));
        };
        let meta = self.entries[index];
        read_spool_record(&mut scan.spool_reader, meta)
    }

    fn summary_for(&mut self, index: usize) -> Result<String> {
        if let Some(summary) = self.summary_cache.get(&index) {
            return Ok(summary.clone());
        }

        let Some(scan) = &mut self.current_scan else {
            return Ok(String::new());
        };
        let meta = self.entries[index];
        let detail = read_spool_record(&mut scan.spool_reader, meta)?;
        let summary = format_summary(&detail, self.hex_payload);
        remember_summary(
            &mut self.summary_cache,
            &mut self.summary_order,
            index,
            summary.clone(),
        );
        Ok(summary)
    }

    fn open_modal(&mut self) -> Result<()> {
        let Some(detail) = &self.selected_detail else {
            return Ok(());
        };

        self.modal_text = Some(render_modal_message(detail, self.hex_payload));
        self.modal_scroll = 0;
        self.focus = Focus::Modal;
        Ok(())
    }

    fn cancel_scan(&mut self) {
        if let Some(scan) = &self.current_scan {
            scan.cancel();
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(frame.area());

        self.render_search(frame, areas[0]);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
            .split(areas[1]);

        self.render_list(frame, body[0]);
        self.render_details(frame, body[1]);
        self.render_status(frame, areas[2]);

        if self.focus == Focus::Modal {
            self.render_modal(frame);
        } else if self.focus == Focus::SaveError {
            self.render_save_error(frame);
        } else if self.focus == Focus::SavePrompt {
            self.render_save_prompt(frame);
        }
    }

    fn render_search(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = match self.filter_mode {
            FilterMode::Strings => " Filter (strings) ",
            FilterMode::Regex => " Filter (regex) ",
        };
        let block = pane_block(title, self.focus == Focus::Search);
        let paragraph = Paragraph::new(self.query_input.as_str())
            .block(block)
            .style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, area);

        if self.focus == Focus::Search {
            let x = area.x.saturating_add(self.query_input.chars().count() as u16 + 1);
            let y = area.y.saturating_add(1);
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), y));
        }
    }

    fn render_list(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = pane_block(" Log entries ", self.focus == Focus::List);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.height == 0 {
            return;
        }

        let visible_rows = inner.height as usize;
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
            let row_width = inner.width.saturating_sub(1) as usize;
            for index in self.list_offset..end {
                let style = if index == self.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Indexed(28))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let summary = self
                    .summary_for(index)
                    .unwrap_or_else(|_| "<failed to render summary>".to_string());
                let summary = fit_to_width(&summary, row_width);
                lines.push(Line::from(Span::styled(summary, style)));
            }
        }

        let paragraph = Paragraph::new(Text::from(lines))
            .scroll((0, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, inner);

        if !self.entries.is_empty() {
            let mut scrollbar_state = ScrollbarState::new(self.entries.len())
                .position(self.selected.min(self.entries.len().saturating_sub(1)));
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                inner,
                &mut scrollbar_state,
            );
        }
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

        let paragraph = Paragraph::new(Text::from(lines))
            .scroll((self.detail_scroll, 0))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, inner);
    }

    fn render_status(&self, frame: &mut Frame<'_>, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let bar_style = Style::default().bg(Color::Indexed(28));
        let buf = frame.buffer_mut();
        buf.set_style(area, bar_style);
        let y = area.y;

        if self.focus == Focus::Modal {
            draw_status_spans(
                buf,
                area.x,
                y,
                area.width,
                &[
                    status_key("ESC"),
                    status_text(" to close   "),
                    status_key("UP/DOWN"),
                    status_text(" scroll"),
                ],
            );
            return;
        }
        if self.focus == Focus::SavePrompt {
            draw_status_spans(
                buf,
                area.x,
                y,
                area.width,
                &[
                    status_key("ENTER"),
                    status_text(" save   "),
                    status_key("ESC"),
                    status_text(" cancel"),
                ],
            );
            return;
        }
        if self.focus == Focus::SaveError {
            draw_status_spans(buf, area.x, y, area.width, &[status_text("Press any key to return")]);
            return;
        }

        let left_spans = status_help_spans(self.focus);
        let status = trim_single_line(&self.status, area.width as usize);
        let status_width = status.chars().count().min(area.width as usize) as u16;
        let gap_width = if area.width > status_width { 1 } else { 0 };
        let left_width = area.width.saturating_sub(status_width).saturating_sub(gap_width);
        draw_status_spans(buf, area.x, y, left_width, &left_spans);

        if status_width > 0 {
            let status_x = area.right().saturating_sub(status_width);
            buf.set_stringn(
                status_x,
                y,
                status,
                status_width as usize,
                Style::default()
                    .fg(Color::LightGreen)
                    .bg(Color::Indexed(28)),
            );
        }
    }

    fn render_save_prompt(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(52, 10, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(
                " Save current content ",
                Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::White).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let label = "Filename: ";
        let input_width = inner.width.saturating_sub(label.chars().count() as u16 + 2);
        let row = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(
                    fit_to_width(&self.save_filename, input_width as usize),
                    Style::default().fg(Color::Black).bg(Color::White),
                ),
            ])),
            row,
        );
        let cursor_x = row
            .x
            .saturating_add(label.chars().count() as u16)
            .saturating_add(1)
            .saturating_add(self.save_filename.chars().count() as u16)
            .min(row.x.saturating_add(label.chars().count() as u16 + input_width));
        let cursor_y = row.y;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn render_save_error(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(38, 12, frame.area());
        frame.render_widget(Clear, area);
        let block = Block::default()
            .title(Span::styled(
                " Error ",
                Style::default()
                    .fg(Color::Red)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::White).bg(Color::Red))
            .style(Style::default().fg(Color::White).bg(Color::Red));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if let Some(message) = &self.save_message {
            frame.render_widget(
                Paragraph::new(render_save_error_message(message))
                    .style(Style::default().bg(Color::Red))
                    .wrap(Wrap { trim: false }),
                inner,
            );
        }
    }

    fn render_modal(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(80, 80, frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .title(Span::styled(
                " Log record ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Indexed(30))
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(Color::Indexed(30)).bg(Color::Gray))
            .style(Style::default().fg(Color::Black).bg(Color::Gray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        let info_entries = if let Some(detail) = &self.selected_detail {
            render_modal_info_entries(detail)
        } else {
            vec![("info".to_string(), "No record loaded.".to_string())]
        };
        let key_width = info_entries
            .iter()
            .map(|(key, _)| key.chars().count())
            .max()
            .unwrap_or(4)
            .max(4);
        let preferred_info_width = info_entries
            .iter()
            .map(|(_, value)| (key_width + 2 + value.chars().count() + 1) as u16)
            .max()
            .unwrap_or((key_width + 3) as u16);
        let max_info_width = chunks[0].width.saturating_div(2).max(16);
        let info_width = preferred_info_width.min(max_info_width).max(16);
        let divider_width = 1;
        let message_width = chunks[0]
            .width
            .saturating_sub(info_width)
            .saturating_sub(divider_width);

        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(message_width),
                Constraint::Length(divider_width),
                Constraint::Length(info_width),
            ])
            .split(chunks[0]);

        let divider = (0..body[1].height)
            .map(|_| Line::from(Span::styled("│", Style::default().fg(Color::Indexed(30)).bg(Color::Gray))))
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(divider).style(Style::default().bg(Color::Gray)),
            body[1],
        );

        let footer = if let Some(detail) = &self.selected_detail {
            render_modal_footer(detail)
        } else {
            render_modal_footer_placeholder()
        };
        let message = self.modal_text.as_deref().unwrap_or("No record loaded.");
        let paragraph = Paragraph::new(message)
            .style(Style::default().fg(Color::Black).bg(Color::Gray))
            .scroll((self.modal_scroll, 0))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, body[0]);

        let value_width = info_width
            .saturating_sub((key_width + 2 + 1) as u16) as usize;
        let info_lines = info_entries
            .into_iter()
            .map(|(key, value)| modal_info_line(&key, value, key_width, value_width))
            .collect::<Vec<_>>();
        let info = Paragraph::new(Text::from(info_lines))
            .style(Style::default().fg(Color::Black).bg(Color::Gray))
            .scroll((0, 0));
        frame.render_widget(info, body[2]);
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().bg(Color::Indexed(30))),
            chunks[1],
        );
    }
}

fn scan_matches(
    input_path: &Path,
    predicate: crate::predicate::RecordPredicate,
    mut spool: File,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ScanUpdate>,
) -> Result<(u64, u64)> {
    let input = InputHandle::open(input_path)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut tx_batch = Vec::with_capacity(SCAN_BATCH_SIZE);
    let mut scanned = 0u64;
    let mut matched = 0u64;

    while !cancel.load(Ordering::Relaxed) {
        let Some(record) = reader.next_record()? else {
            break;
        };
        scanned = scanned
            .checked_add(1)
            .ok_or(logjet::Error::NumericOverflow("view scanned"))?;

        if predicate.matches(&record) {
            let meta = write_spool_record(&mut spool, &record)?;
            tx_batch.push(meta);
            matched = matched
                .checked_add(1)
                .ok_or(logjet::Error::NumericOverflow("view matched"))?;

            if tx_batch.len() >= SCAN_BATCH_SIZE {
                tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch)))
                    .map_err(|err| Error::Usage(err.to_string()))?;
            }
        }
    }

    if !tx_batch.is_empty() {
        tx.send(ScanUpdate::Batch(tx_batch))
            .map_err(|err| Error::Usage(err.to_string()))?;
    }

    Ok((scanned, matched))
}

fn write_spool_record(file: &mut File, record: &OwnedRecord) -> Result<EntryMeta> {
    let offset = file.seek(SeekFrom::End(0))?;
    file.write_all(&[record.record_type as u8])?;
    file.write_all(&record.seq.to_le_bytes())?;
    file.write_all(&record.ts_unix_ns.to_le_bytes())?;
    let payload_len = u64::try_from(record.payload.len())
        .map_err(|_| logjet::Error::NumericOverflow("view payload_len"))?;
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&record.payload)?;
    file.flush()?;

    Ok(EntryMeta {
        offset,
        record_type: record.record_type,
        seq: record.seq,
        ts_unix_ns: record.ts_unix_ns,
        payload_len,
    })
}

fn read_spool_record(file: &mut File, meta: EntryMeta) -> Result<DetailRecord> {
    file.seek(SeekFrom::Start(meta.offset + 1 + 8 + 8 + 8))?;
    let mut payload = vec![0u8; meta.payload_len as usize];
    file.read_exact(&mut payload)?;
    Ok(DetailRecord { meta, payload })
}

fn remember_summary(
    cache: &mut HashMap<usize, String>,
    order: &mut VecDeque<usize>,
    index: usize,
    summary: String,
) {
    cache.insert(index, summary);
    order.push_back(index);
    while order.len() > SUMMARY_CACHE_LIMIT {
        if let Some(old) = order.pop_front() {
            cache.remove(&old);
        }
    }
}

fn format_summary(detail: &DetailRecord, hex_payload: bool) -> String {
    
    if hex_payload {
        hex_preview(&detail.payload, 32)
    } else if let Some(message) = extract_otlp_log_message(&detail.payload) {
        trim_single_line(&message, 160)
    } else {
        text_preview(&detail.payload, 160)
    }
}

fn render_detail_lines(detail: &DetailRecord, hex_payload: bool) -> Vec<Line<'static>> {
    let mut lines = vec![
        key_value_line(
            "Record type:",
            record_kind_label(detail.meta.record_type).to_string(),
            Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
        ),
        key_value_line("Sequence:", detail.meta.seq.to_string(), Style::default().fg(Color::White)),
        key_value_line(
            "Timestamp:",
            format_timestamp(detail.meta.ts_unix_ns),
            Style::default().fg(Color::White),
        ),
        key_value_line(
            "Payload:",
            format!("{} bytes", detail.meta.payload_len),
            Style::default().fg(Color::White),
        ),
        Line::from(""),
    ];

    lines.extend(render_otlp_lines(detail));
    if lines.len() == 5 {
        let preview = if hex_payload {
            hex_preview(&detail.payload, 64)
        } else {
            text_preview(&detail.payload, DETAIL_PREVIEW_BYTES)
        };
        lines.push(key_value_line("Preview:", preview, Style::default().fg(Color::White)));
    }

    lines
}

fn render_otlp_lines(detail: &DetailRecord) -> Vec<Line<'static>> {
    if detail.meta.record_type != RecordType::Logs {
        return Vec::new();
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![Line::from(vec![
            Span::styled("OTLP logs: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("payload decode failed; showing raw preview"),
        ])];
    };

    let mut services = Vec::new();
    let mut severities = Vec::new();
    let mut record_count = 0usize;
    let mut scope_count = 0usize;

    for resource_logs in &batch.resource_logs {
        if let Some(resource) = &resource_logs.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !services.iter().any(|existing| existing == service)
                {
                    services.push(service.clone());
                }
            }
        }

        for scope_logs in &resource_logs.scope_logs {
            scope_count += 1;
            for log_record in &scope_logs.log_records {
                record_count += 1;
                if !log_record.severity_text.is_empty()
                    && !severities.iter().any(|existing| existing == &log_record.severity_text)
                {
                    severities.push(log_record.severity_text.clone());
                }
            }
        }
    }

    let mut lines = vec![
        key_value_line("OTLP kind:", "logs".to_string(), Style::default().fg(Color::White)),
        key_value_line("Resources:", batch.resource_logs.len().to_string(), Style::default().fg(Color::White)),
        key_value_line("Scopes:", scope_count.to_string(), Style::default().fg(Color::White)),
        key_value_line("Log records:", record_count.to_string(), Style::default().fg(Color::White)),
    ];

    if !services.is_empty() {
        lines.push(key_value_line(
            "Services:",
            services.join(", "),
            Style::default().fg(Color::White),
        ));
    }
    if !severities.is_empty() {
        lines.push(key_value_line(
            "Severity:",
            severities.join(", "),
            severity_style(severities.first().map(String::as_str).unwrap_or("")),
        ));
    }

    lines
}

fn extract_otlp_log_message(payload: &[u8]) -> Option<String> {
    let batch = ExportLogsServiceRequest::decode(payload).ok()?;
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                if let Some(body) = &log_record.body
                    && let Some(Value::StringValue(message)) = &body.value
                {
                    return Some(message.clone());
                }
            }
        }
    }
    None
}

fn render_modal_message(detail: &DetailRecord, hex_payload: bool) -> String {
    if let Some(message) = extract_otlp_log_message(&detail.payload) {
        return message;
    }

    if hex_payload {
        hex_dump(&detail.payload)
    } else {
        String::from_utf8_lossy(&detail.payload).into_owned()
    }
}

fn render_modal_footer(detail: &DetailRecord) -> Line<'static> {
    let (size_num, size_unit) = format_size_parts(detail.meta.payload_len);
    Line::from(vec![
        Span::styled(
            format!("#{}", detail.meta.seq),
            Style::default().fg(Color::LightGreen),
        ),
        footer_sep(),
        Span::styled(
            format_timestamp(detail.meta.ts_unix_ns),
            Style::default().fg(Color::White),
        ),
        footer_sep(),
        Span::styled(
            record_kind_label(detail.meta.record_type).to_string(),
            Style::default().fg(Color::Black).add_modifier(Modifier::BOLD),
        ),
        footer_sep(),
        Span::styled(size_num, Style::default().fg(Color::Yellow)),
        Span::styled(size_unit, Style::default().fg(Color::Black)),
    ])
}

fn render_modal_footer_placeholder() -> Line<'static> {
    Line::from(vec![
        Span::styled("#", Style::default().fg(Color::LightGreen)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::White)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::Black).add_modifier(Modifier::BOLD)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::Yellow)),
        Span::styled("", Style::default().fg(Color::Black)),
    ])
}

fn render_modal_info_entries(detail: &DetailRecord) -> Vec<(String, String)> {
    let mut lines = vec![
        ("type".to_string(), record_kind_label(detail.meta.record_type).to_string()),
        ("seq".to_string(), detail.meta.seq.to_string()),
        ("ts_unix_ns".to_string(), detail.meta.ts_unix_ns.to_string()),
        ("time".to_string(), format_timestamp(detail.meta.ts_unix_ns)),
        ("payload_bytes".to_string(), detail.meta.payload_len.to_string()),
    ];

    if detail.meta.record_type != RecordType::Logs {
        return lines;
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        lines.push(("otlp".to_string(), "decode failed".to_string()));
        return lines;
    };

    let mut service_names = Vec::new();
    let mut scopes = Vec::new();
    let mut severities = Vec::new();
    let mut event_names = Vec::new();
    let mut resource_attr_count = 0usize;
    let mut record_attr_count = 0usize;
    let mut trace_ids = 0usize;
    let mut span_ids = 0usize;

    for resource_logs in &batch.resource_logs {
        if let Some(resource) = &resource_logs.resource {
            resource_attr_count += resource.attributes.len();
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !service_names.iter().any(|existing| existing == service)
                {
                    service_names.push(service.clone());
                }
            }
        }

        for scope_logs in &resource_logs.scope_logs {
            if let Some(scope) = &scope_logs.scope
                && !scope.name.is_empty()
                && !scopes.iter().any(|existing| existing == &scope.name)
            {
                scopes.push(scope.name.clone());
            }
            for record in &scope_logs.log_records {
                record_attr_count += record.attributes.len();
                if !record.severity_text.is_empty()
                    && !severities.iter().any(|existing| existing == &record.severity_text)
                {
                    severities.push(record.severity_text.clone());
                }
                if !record.event_name.is_empty()
                    && !event_names.iter().any(|existing| existing == &record.event_name)
                {
                    event_names.push(record.event_name.clone());
                }
                if !record.trace_id.is_empty() {
                    trace_ids += 1;
                }
                if !record.span_id.is_empty() {
                    span_ids += 1;
                }
            }
        }
    }

    lines.push(("otlp.kind".to_string(), "logs".to_string()));
    lines.push(("resources".to_string(), batch.resource_logs.len().to_string()));
    if !service_names.is_empty() {
        lines.push(("service.name".to_string(), service_names.join(", ")));
    }
    if !scopes.is_empty() {
        lines.push(("scope".to_string(), scopes.join(", ")));
    }
    if !severities.is_empty() {
        lines.push(("severity".to_string(), severities.join(", ")));
    }
    if !event_names.is_empty() {
        lines.push(("event".to_string(), event_names.join(", ")));
    }
    lines.push(("resource.attrs".to_string(), resource_attr_count.to_string()));
    lines.push(("record.attrs".to_string(), record_attr_count.to_string()));
    if trace_ids > 0 {
        lines.push(("trace_id".to_string(), format!("{trace_ids} present")));
    }
    if span_ids > 0 {
        lines.push(("span_id".to_string(), format!("{span_ids} present")));
    }

    lines
}

fn modal_info_line(key: &str, value: String, key_width: usize, value_width: usize) -> Line<'static> {
    let value = trim_single_line(&value, value_width);
    Line::from(vec![
        Span::styled(
            format!("{key:<width$}: ", width = key_width),
            Style::default().fg(Color::Indexed(136)),
        ),
        Span::styled(value, Style::default().fg(Color::Black)),
    ])
}

fn footer_sep() -> Span<'static> {
    Span::styled(" | ", Style::default().fg(Color::Black))
}

fn format_size_parts(bytes: u64) -> (String, String) {
    if bytes >= 1024 * 1024 {
        (format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)), " Mb".to_string())
    } else if bytes >= 1024 {
        (format!("{:.1}", bytes as f64 / 1024.0), " Kb".to_string())
    } else {
        (bytes.to_string(), " Bt".to_string())
    }
}

fn record_kind_label(record_type: RecordType) -> &'static str {
    match record_type {
        RecordType::Logs => "logs",
        RecordType::Metrics => "metrics",
        RecordType::Traces => "traces",
    }
}

fn text_preview(bytes: &[u8], limit: usize) -> String {
    trim_single_line(&String::from_utf8_lossy(bytes), limit)
}

fn trim_single_line(input: &str, limit: usize) -> String {
    let flattened = input
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' | '\t' => ' ',
            other if other.is_control() => ' ',
            other => other,
        })
        .collect::<String>();

    let mut output = flattened.chars().take(limit).collect::<String>();
    if flattened.chars().count() > limit {
        output.push_str("...");
    }
    output
}

fn fit_to_width(input: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let char_count = input.chars().count();
    if char_count <= width {
        let mut padded = input.to_string();
        padded.push_str(&" ".repeat(width - char_count));
        return padded;
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut out = input.chars().take(width - 3).collect::<String>();
    out.push_str("...");
    out
}

fn key_value_line(label: &str, value: String, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12} "), Style::default().fg(Color::Indexed(136))),
        Span::styled(value, value_style),
    ])
}

fn severity_style(value: &str) -> Style {
    let upper = value.to_ascii_uppercase();
    let color = if upper.contains("ERROR") || upper.contains("ERR") || upper.contains("FATAL") {
        Color::LightRed
    } else if upper.contains("WARN") {
        Color::Indexed(214)
    } else if upper.contains("INFO") {
        Color::LightGreen
    } else if upper.contains("DEBUG") {
        Color::LightCyan
    } else if upper.contains("TRACE") {
        Color::LightBlue
    } else {
        Color::White
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn format_timestamp(ts_unix_ns: u64) -> String {
    let secs = (ts_unix_ns / 1_000_000_000) as i64;
    let nanos = (ts_unix_ns % 1_000_000_000) as u32;
    match Utc.timestamp_opt(secs, nanos).single() {
        Some(ts) => ts.format("%Y-%m-%d %H:%M:%S.%f UTC").to_string(),
        None => ts_unix_ns.to_string(),
    }
}

fn hex_preview(bytes: &[u8], limit: usize) -> String {
    let shown = bytes.iter().take(limit);
    let mut out = shown
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > limit {
        out.push_str(" ...");
    }
    out
}

fn hex_dump(bytes: &[u8]) -> String {
    let mut out = String::new();
    for (chunk_index, chunk) in bytes.chunks(16).enumerate() {
        out.push_str(&format!("{:08x}: ", chunk_index * 16));
        for byte in chunk {
            out.push_str(&format!("{byte:02x} "));
        }
        out.push('\n');
    }
    out
}

fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
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

fn pane_block<'a>(title: &'a str, active: bool) -> Block<'a> {
    let title_style = if active {
        Style::default().fg(Color::Black).bg(Color::LightGreen)
    } else {
        Style::default().fg(Color::Indexed(28))
    };
    let border_style = if active {
        Style::default().fg(Color::LightGreen)
    } else {
        Style::default().fg(Color::Indexed(28))
    };

    Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(if active { BorderType::Double } else { BorderType::Plain })
        .border_style(border_style)
        .style(Style::default().fg(Color::White))
}

fn status_help_spans(focus: Focus) -> Vec<Span<'static>> {
    match focus {
        Focus::Search => vec![
            status_key("Q"),
            status_text(" quit  "),
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
            status_text(" save to file  "),
            status_key("UP/DOWN"),
            status_text(" navigate"),
        ],
        Focus::Modal => Vec::new(),
        Focus::SavePrompt => Vec::new(),
        Focus::SaveError => Vec::new(),
    }
}

fn status_key(text: &str) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::White)
            .bg(Color::Indexed(28))
            .add_modifier(Modifier::BOLD),
    )
}

fn status_text(text: &str) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::default().fg(Color::Black).bg(Color::Indexed(28)),
    )
}

fn draw_status_spans(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    spans: &[Span<'static>],
) {
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

fn render_save_error_message(message: &str) -> Line<'static> {
    const PREFIX: &str = "File ";
    const SUFFIX: &str = " already exist";

    if let Some(filename) = message
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
    {
        return Line::from(vec![
            Span::styled(PREFIX, Style::default().fg(Color::White).bg(Color::Red)),
            Span::styled(
                filename.to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(SUFFIX, Style::default().fg(Color::White).bg(Color::Red)),
        ]);
    }

    Line::from(Span::styled(
        message.to_string(),
        Style::default().fg(Color::White).bg(Color::Red),
    ))
}

fn create_temp_path() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| Error::Usage(format!("system clock error: {err}")))?
        .as_nanos();
    for attempt in 0..1000u32 {
        let candidate = base.join(format!("ljx-view-{pid}-{nanos}-{attempt}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::Usage("unable to allocate a temporary view file".to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        DetailRecord, EntryMeta, extract_otlp_log_message, format_summary,
        render_modal_message, text_preview,
    };
    use logjet::RecordType;
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    #[test]
    fn text_preview_flattens_newlines() {
        assert_eq!(text_preview(b"hello\nworld", 32), "hello world");
    }

    #[test]
    fn summary_uses_trimmed_single_line_preview() {
        let detail = DetailRecord {
            meta: EntryMeta {
                offset: 0,
                record_type: RecordType::Logs,
                seq: 7,
                ts_unix_ns: 9,
                payload_len: 13,
            },
            payload: b"line one\nline two".to_vec(),
        };
        let summary = format_summary(&detail, false);
        assert_eq!(summary, "line one line two");
    }

    #[test]
    fn summary_prefers_decoded_otlp_log_message() {
        let batch = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope {
                        name: "test".to_string(),
                        version: String::new(),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                    }),
                    log_records: vec![LogRecord {
                        time_unix_nano: 0,
                        observed_time_unix_nano: 0,
                        severity_number: 0,
                        severity_text: String::new(),
                        body: Some(AnyValue {
                            value: Some(Value::StringValue("hello from body".to_string())),
                        }),
                        attributes: Vec::new(),
                        dropped_attributes_count: 0,
                        flags: 0,
                        trace_id: Vec::new(),
                        span_id: Vec::new(),
                        event_name: String::new(),
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let payload = batch.encode_to_vec();
        let detail = DetailRecord {
            meta: EntryMeta {
                offset: 0,
                record_type: RecordType::Logs,
                seq: 1,
                ts_unix_ns: 2,
                payload_len: payload.len() as u64,
            },
            payload,
        };

        assert_eq!(extract_otlp_log_message(&detail.payload).as_deref(), Some("hello from body"));
        assert_eq!(format_summary(&detail, false), "hello from body");
    }

    #[test]
    fn modal_falls_back_to_raw_payload() {
        let detail = DetailRecord {
            meta: EntryMeta {
                offset: 0,
                record_type: RecordType::Metrics,
                seq: 1,
                ts_unix_ns: 2,
                payload_len: 5,
            },
            payload: b"hello".to_vec(),
        };
        let body = render_modal_message(&detail, false);
        assert_eq!(body, "hello");
    }
}
