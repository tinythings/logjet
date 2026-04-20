use std::collections::{HashMap, HashSet, VecDeque};
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
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use logjet::{LogjetReader, LogjetWriter, OwnedRecord, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Gauge, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::cli::ViewArgs;
use crate::dedup::{DedupMatchMode, DedupMode};
use crate::error::{Error, Result};
use crate::exporter::ExporterRegistry;
use crate::input::InputHandle;
use crate::predicate::{FieldFilter, FilterMode, parse_filter_query};

const SUMMARY_CACHE_LIMIT: usize = 256;
const DETAIL_PREVIEW_BYTES: usize = 1024;
const SCAN_BATCH_SIZE: usize = 128;
const TICK_RATE: Duration = Duration::from_millis(100);
const MODAL_ATTR_ENTRY_LIMIT_PER_KIND: usize = 32;

pub fn run(args: ViewArgs) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(Error::Usage("ljx view needs an interactive terminal; pipe-oriented output belongs in `ljx filter`".to_string()));
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
    FieldFilter,
    SavePrompt,
    SaveError,
    ExportPrompt,
    ExportError,
    DedupPrompt,
    DedupProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportField {
    Format,
    Filename,
    Range,
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
    Finished { scanned: u64, matched: u64 },
    Failed(String),
}

#[derive(Debug, Clone)]
enum DedupUpdate {
    Progress { ratio: f64, phase: String },
    Done { total: u64, groups: u64, pct: f64 },
    Failed(String),
}

impl DedupMode {
    fn label(self) -> &'static str {
        match self {
            Self::Distinct => "distinct",
            Self::Collapse => "collapse",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Distinct => "whole filtered set, SQL-like distinct",
            Self::Collapse => "nearby burst suppression within bucket",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Distinct => Self::Collapse,
            Self::Collapse => Self::Distinct,
        }
    }

    fn prev(self) -> Self {
        self.next()
    }
}

impl DedupMatchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Hash2 => "canon",
            Self::Full => "full",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Exact => "byte-identical bodies only",
            Self::Hash2 => "canonicalized body grouping",
            Self::Full => "canon plus Drain3 residuals",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Exact => Self::Hash2,
            Self::Hash2 => Self::Full,
            Self::Full => Self::Exact,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Exact => Self::Full,
            Self::Hash2 => Self::Exact,
            Self::Full => Self::Hash2,
        }
    }
}

#[derive(Debug, Clone)]
struct ExportFormatChoice {
    name: String,
    title: String,
    default_extension: String,
}

impl ExportFormatChoice {
    fn ndjson() -> Self {
        Self { name: "ndjson".to_string(), title: "NDJSON".to_string(), default_extension: "ndjson".to_string() }
    }

    fn from_plugin_name(name: String) -> Self {
        Self { title: name.to_ascii_uppercase(), default_extension: name.clone(), name }
    }

    fn label(&self) -> &str {
        self.name.as_str()
    }

    fn title(&self) -> &str {
        self.title.as_str()
    }

    fn default_extension(&self) -> &str {
        self.default_extension.as_str()
    }
}

fn discover_export_format_choices(exporters: &ExporterRegistry) -> Vec<ExportFormatChoice> {
    let mut out = vec![ExportFormatChoice::ndjson()];
    let mut plugins = exporters.available_formats().into_iter().filter(|name| name != "ndjson").collect::<Vec<_>>();
    plugins.sort();
    plugins.dedup();
    out.extend(plugins.into_iter().map(ExportFormatChoice::from_plugin_name));
    out
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

/// Distinct field values collected by the background catalog scan.
struct FieldCatalog {
    severities: Vec<String>,
    services: Vec<String>,
}

/// UI state for the field-filter popup.
struct FieldFilterState {
    /// 0 = severity panel, 1 = services panel
    panel: usize,
    severity_cursor: usize,
    service_cursor: usize,
    severity_scroll: u16,
    service_scroll: u16,
    filter_text: String,
    selected_severities: HashSet<String>,
    selected_services: HashSet<String>,
}

struct ViewApp {
    input: PathBuf,
    hex_payload: bool,
    exporters: ExporterRegistry,
    export_formats: Vec<ExportFormatChoice>,
    focus: Focus,
    filter_mode: FilterMode,
    query_input: String,
    applied_query: String,
    status: String,
    entries: Vec<EntryMeta>,
    selected: usize,
    list_offset: usize,
    modal_scroll: u16,
    modal_info_visible: bool,
    detail_scroll: u16,
    summary_cache: HashMap<usize, String>,
    summary_order: VecDeque<usize>,
    selected_detail: Option<DetailRecord>,
    modal_text: Option<String>,
    save_filename: String,
    save_filename_cursor: usize,
    save_message: Option<String>,
    export_format_index: usize,
    export_filename: String,
    export_filename_cursor: usize,
    export_range: String,
    export_range_cursor: usize,
    export_field: ExportField,
    export_message: Option<String>,
    current_scan: Option<ActiveScan>,
    field_catalog: Arc<std::sync::Mutex<Option<FieldCatalog>>>,
    field_filter_state: Option<FieldFilterState>,
    active_field_filter: FieldFilter,
    dedup_filename: String,
    dedup_behavior: DedupMode,
    dedup_match_mode: DedupMatchMode,
    dedup_output_path: Option<PathBuf>,
    dedup_rx: Option<Receiver<DedupUpdate>>,
    dedup_progress: f64,
    dedup_progress_target: f64,
    dedup_phase: String,
    dedup_completion_message: Option<String>,
}

impl ViewApp {
    fn new(args: ViewArgs) -> Result<Self> {
        let exporters = ExporterRegistry::discover();
        let export_formats = discover_export_format_choices(&exporters);
        let catalog: Arc<std::sync::Mutex<Option<FieldCatalog>>> = Arc::new(std::sync::Mutex::new(None));
        let catalog_bg = Arc::clone(&catalog);
        let input_bg = args.input.clone();
        thread::spawn(move || {
            if let Ok(cat) = scan_field_catalog(&input_bg) {
                *catalog_bg.lock().unwrap() = Some(cat);
            }
        });

        Ok(Self {
            input: args.input,
            hex_payload: args.hex_payload,
            exporters,
            export_formats,
            focus: Focus::Search,
            filter_mode: FilterMode::Strings,
            query_input: String::new(),
            applied_query: String::new(),
            status: "Type a filter and press Enter to scan matching records".to_string(),
            entries: Vec::new(),
            selected: 0,
            list_offset: 0,
            modal_scroll: 0,
            modal_info_visible: false,
            detail_scroll: 0,
            summary_cache: HashMap::new(),
            summary_order: VecDeque::new(),
            selected_detail: None,
            modal_text: None,
            save_filename: String::new(),
            save_filename_cursor: 0,
            save_message: None,
            export_format_index: 0,
            export_filename: String::new(),
            export_filename_cursor: 0,
            export_range: "all".to_string(),
            export_range_cursor: 3,
            export_field: ExportField::Format,
            export_message: None,
            current_scan: None,
            field_catalog: catalog,
            field_filter_state: None,
            active_field_filter: FieldFilter::default(),
            dedup_filename: String::new(),
            dedup_behavior: DedupMode::Distinct,
            dedup_match_mode: DedupMatchMode::Hash2,
            dedup_output_path: None,
            dedup_rx: None,
            dedup_progress: 0.0,
            dedup_progress_target: 0.0,
            dedup_phase: String::new(),
            dedup_completion_message: None,
        })
    }

    fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            self.drain_scan_updates()?;
            self.drain_dedup_updates();
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
        if self.focus == Focus::List && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            self.cancel_scan();
            return Ok(true);
        }

        match self.focus {
            Focus::Modal => self.handle_modal_key(key),
            Focus::FieldFilter => self.handle_field_filter_key(key),
            Focus::SavePrompt => self.handle_save_prompt_key(key),
            Focus::SaveError => self.handle_save_error_key(),
            Focus::ExportPrompt => self.handle_export_prompt_key(key),
            Focus::ExportError => self.handle_export_error_key(),
            Focus::DedupPrompt => self.handle_dedup_prompt_key(key),
            Focus::DedupProgress => self.handle_dedup_progress_key(key),
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
            KeyCode::Char('i') | KeyCode::Char('I') => {
                self.modal_info_visible = !self.modal_info_visible;
            }
            KeyCode::Left => {
                self.move_selection(-1)?;
                self.open_modal()?;
            }
            KeyCode::Right => {
                self.move_selection(1)?;
                self.open_modal()?;
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
                delete_char_before(&mut self.save_filename, &mut self.save_filename_cursor);
            }
            KeyCode::Delete => {
                delete_char_at(&mut self.save_filename, self.save_filename_cursor);
            }
            KeyCode::Left => {
                self.save_filename_cursor = self.save_filename_cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.save_filename_cursor = (self.save_filename_cursor + 1).min(char_count(&self.save_filename));
            }
            KeyCode::Home => {
                self.save_filename_cursor = 0;
            }
            KeyCode::End => {
                self.save_filename_cursor = char_count(&self.save_filename);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_filename.clear();
                self.save_filename_cursor = 0;
            }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                insert_char_at(&mut self.save_filename, &mut self.save_filename_cursor, ch);
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

    fn handle_export_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.export_message = None;
            }
            KeyCode::Enter => {
                self.export_current_results()?;
            }
            KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
                self.export_field = match self.export_field {
                    ExportField::Format => ExportField::Filename,
                    ExportField::Filename => ExportField::Range,
                    ExportField::Range => ExportField::Format,
                };
            }
            KeyCode::Backspace => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => delete_char_before(&mut self.export_filename, &mut self.export_filename_cursor),
                ExportField::Range => delete_char_before(&mut self.export_range, &mut self.export_range_cursor),
            },
            KeyCode::Delete => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => delete_char_at(&mut self.export_filename, self.export_filename_cursor),
                ExportField::Range => delete_char_at(&mut self.export_range, self.export_range_cursor),
            },
            KeyCode::Left => match self.export_field {
                ExportField::Format => self.cycle_export_format(-1),
                ExportField::Filename => self.export_filename_cursor = self.export_filename_cursor.saturating_sub(1),
                ExportField::Range => self.export_range_cursor = self.export_range_cursor.saturating_sub(1),
            },
            KeyCode::Right => match self.export_field {
                ExportField::Format => self.cycle_export_format(1),
                ExportField::Filename => self.export_filename_cursor = (self.export_filename_cursor + 1).min(char_count(&self.export_filename)),
                ExportField::Range => self.export_range_cursor = (self.export_range_cursor + 1).min(char_count(&self.export_range)),
            },
            KeyCode::Home => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => self.export_filename_cursor = 0,
                ExportField::Range => self.export_range_cursor = 0,
            },
            KeyCode::End => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => self.export_filename_cursor = char_count(&self.export_filename),
                ExportField::Range => self.export_range_cursor = char_count(&self.export_range),
            },
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => {
                    self.export_filename.clear();
                    self.export_filename_cursor = 0;
                }
                ExportField::Range => {
                    self.export_range.clear();
                    self.export_range_cursor = 0;
                }
            },
            KeyCode::Char(' ') if self.export_field == ExportField::Format => {
                self.cycle_export_format(1);
            }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => match self.export_field {
                ExportField::Format => {}
                ExportField::Filename => insert_char_at(&mut self.export_filename, &mut self.export_filename_cursor, ch),
                ExportField::Range => insert_char_at(&mut self.export_range, &mut self.export_range_cursor, ch),
            },
            _ => {}
        }
        Ok(false)
    }

    fn handle_export_error_key(&mut self) -> Result<bool> {
        self.focus = Focus::ExportPrompt;
        self.export_message = None;
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
            KeyCode::Char('e') | KeyCode::Char('E') => {
                self.open_export_prompt()?;
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.open_dedup_prompt();
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
            KeyCode::Char('f') | KeyCode::Char('F') => {
                self.open_field_filter();
            }
            KeyCode::Char('/') => {
                self.focus = Focus::Search;
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
        let mut predicate = parse_filter_query(&self.applied_query, self.filter_mode)?;
        predicate.field_filter = self.active_field_filter.clone();

        let (spool_path, spool_reader, spool_writer) = open_temp_spool_pair()?;
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
        self.current_scan = Some(ActiveScan { rx, cancel, spool_path, spool_reader, scanned: 0, matched: 0, finished: false });

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
        self.save_filename_cursor = char_count(&self.save_filename);
        self.focus = Focus::SavePrompt;
        Ok(())
    }

    fn open_export_prompt(&mut self) -> Result<()> {
        let Some(scan) = &self.current_scan else {
            self.status = "No active scan to export.".to_string();
            return Ok(());
        };
        if !scan.finished {
            self.status = "Wait for the scan to finish before exporting.".to_string();
            return Ok(());
        }
        self.export_formats = discover_export_format_choices(&self.exporters);
        self.export_message = None;
        let default_ext = self.current_export_format().default_extension().to_string();
        if self.export_filename.is_empty() {
            let stem = self.input.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
            self.export_filename = format!("{stem}.{default_ext}");
        } else {
            self.sync_export_filename_extension();
        }
        if self.export_range.is_empty() {
            self.export_range = "all".to_string();
        }
        self.export_filename_cursor = char_count(&self.export_filename);
        self.export_range_cursor = char_count(&self.export_range);
        self.export_field = ExportField::Format;
        self.focus = Focus::ExportPrompt;
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
        let output_dir = self.input.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let output_path = output_dir.join(filename);
        if output_path == self.input || output_path.exists() {
            self.save_message = Some(format!("File {filename} already exist"));
            self.focus = Focus::SaveError;
            return Ok(());
        }

        let file = OpenOptions::new().write(true).create_new(true).open(&output_path)?;
        let writer = BufWriter::new(file);
        let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
        for meta in &self.entries {
            let detail = read_spool_record(&mut scan.spool_reader, *meta)?;
            logjet.push(detail.meta.record_type, detail.meta.seq, detail.meta.ts_unix_ns, &detail.payload)?;
        }
        let mut writer = logjet.into_inner()?;
        writer.flush()?;

        self.focus = Focus::List;
        self.save_message = None;
        self.status = format!("Saved {} records to {}", self.entries.len(), output_path.display());
        Ok(())
    }

    fn export_current_results(&mut self) -> Result<()> {
        let filename = self.export_filename.trim();
        if filename.is_empty() {
            self.export_message = Some("Filename must not be empty.".to_string());
            return Ok(());
        }
        if filename.contains('/') {
            self.export_message = Some("Filename must not contain path separators.".to_string());
            return Ok(());
        }
        if self.input == Path::new("-") {
            self.export_message = Some("Cannot infer output directory when input is stdin.".to_string());
            return Ok(());
        }

        let selected = parse_export_selection(&self.export_range, self.entries.len(), self.selected).map_err(Error::Usage);
        let (start, end) = match selected {
            Ok(range) => range,
            Err(err) => {
                self.export_message = Some(err.to_string());
                self.focus = Focus::ExportError;
                return Ok(());
            }
        };

        let output_dir = self.input.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let output_path = output_dir.join(filename);
        if output_path == self.input || output_path.exists() {
            self.export_message = Some(format!("File {filename} already exist"));
            self.focus = Focus::ExportError;
            return Ok(());
        }

        let format = self.current_export_format().clone();
        let selected_entries = self.entries[start..end].to_vec();
        let mut exported = 0usize;
        match format.label() {
            "ndjson" => {
                let Some(scan) = &mut self.current_scan else {
                    self.export_message = Some("No scan data to export.".to_string());
                    return Ok(());
                };
                let mut out = OpenOptions::new().write(true).create_new(true).open(&output_path)?;
                for meta in selected_entries.iter().copied() {
                    let detail = read_spool_record(&mut scan.spool_reader, meta)?;
                    for object in export_ndjson_objects(&detail) {
                        serde_json::to_writer(&mut out, &object).map_err(|e| Error::Usage(e.to_string()))?;
                        out.write_all(b"\n")?;
                        exported += 1;
                    }
                }
                out.flush()?;
            }
            other => {
                let temp_input = {
                    let Some(scan) = &mut self.current_scan else {
                        self.export_message = Some("No scan data to export.".to_string());
                        return Ok(());
                    };
                    write_export_selection_to_temp_logjet(scan, &selected_entries)?
                };
                let plugin = self.exporters.plugin(other).ok_or_else(|| self.exporters.unknown_format_error(other))?;
                let plugin_result = plugin.export(&temp_input, &output_path, false, &[]);
                let _ = std::fs::remove_file(&temp_input);
                plugin_result?;
                exported = end.saturating_sub(start);
            }
        }

        self.focus = Focus::List;
        self.export_message = None;
        self.status = format!("Exported {exported} {} row(s) to {}", format.label(), output_path.display());
        Ok(())
    }

    fn current_export_format(&self) -> &ExportFormatChoice {
        &self.export_formats[self.export_format_index.min(self.export_formats.len().saturating_sub(1))]
    }

    fn cycle_export_format(&mut self, delta: isize) {
        if self.export_formats.is_empty() {
            return;
        }
        let len = self.export_formats.len() as isize;
        let idx = self.export_format_index as isize;
        self.export_format_index = (idx + delta).rem_euclid(len) as usize;
        self.sync_export_filename_extension();
    }

    fn sync_export_filename_extension(&mut self) {
        if self.export_filename.trim().is_empty() {
            return;
        }
        let extension = self.current_export_format().default_extension().to_string();
        let Some((stem, _)) = self.export_filename.rsplit_once('.') else {
            self.export_filename.push('.');
            self.export_filename.push_str(&extension);
            self.export_filename_cursor = char_count(&self.export_filename);
            return;
        };
        self.export_filename = format!("{stem}.{extension}");
        self.export_filename_cursor = char_count(&self.export_filename);
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
                        status_override = Some(format!("Scan complete: {matched} matches out of {scanned} records"));
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
        remember_summary(&mut self.summary_cache, &mut self.summary_order, index, summary.clone());
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

    fn open_field_filter(&mut self) {
        let catalog = self.field_catalog.lock().unwrap();
        let Some(cat) = catalog.as_ref() else {
            self.status = "Field catalog still scanning… try again in a moment".to_string();
            return;
        };
        self.field_filter_state = Some(FieldFilterState {
            panel: 0,
            severity_cursor: 0,
            service_cursor: 0,
            severity_scroll: 0,
            service_scroll: 0,
            filter_text: String::new(),
            selected_severities: self.active_field_filter.severities.clone().unwrap_or_default(),
            selected_services: self.active_field_filter.services.clone().unwrap_or_default(),
        });
        // Need to drop the lock before changing focus
        let _ = cat;
        drop(catalog);
        self.focus = Focus::FieldFilter;
    }

    fn handle_field_filter_key(&mut self, key: KeyEvent) -> Result<bool> {
        let catalog = self.field_catalog.lock().unwrap();
        let Some(cat) = catalog.as_ref() else {
            self.focus = Focus::List;
            return Ok(false);
        };
        let sev_list = cat.severities.clone();
        let svc_list = cat.services.clone();
        drop(catalog);

        let Some(state) = &mut self.field_filter_state else {
            self.focus = Focus::List;
            return Ok(false);
        };

        // Build filtered lists for the active panel.
        let filter_lower = state.filter_text.to_lowercase();
        let filtered_sev: Vec<&String> = sev_list.iter().filter(|s| filter_lower.is_empty() || s.to_lowercase().contains(&filter_lower)).collect();
        let filtered_svc: Vec<&String> = svc_list.iter().filter(|s| filter_lower.is_empty() || s.to_lowercase().contains(&filter_lower)).collect();
        let _active_count = if state.panel == 0 { filtered_sev.len() } else { filtered_svc.len() };

        // Visible rows for scroll calculation.
        let screen_h = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(40);
        let popup_h = ((screen_h as u32 * 70 / 100) as u16).max(6);
        let visible_rows = popup_h.saturating_sub(4) as usize;

        match key.code {
            KeyCode::Esc => {
                self.field_filter_state = None;
                self.focus = Focus::List;
            }
            KeyCode::Tab => {
                state.panel = 1 - state.panel;
                state.filter_text.clear();
            }
            KeyCode::Up => {
                if state.panel == 0 {
                    state.severity_cursor = state.severity_cursor.saturating_sub(1);
                } else {
                    state.service_cursor = state.service_cursor.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if state.panel == 0 {
                    if !filtered_sev.is_empty() {
                        state.severity_cursor = (state.severity_cursor + 1).min(filtered_sev.len() - 1);
                    }
                } else if !filtered_svc.is_empty() {
                    state.service_cursor = (state.service_cursor + 1).min(filtered_svc.len() - 1);
                }
            }
            KeyCode::Char(' ') => {
                if state.panel == 0 {
                    if let Some(&val) = filtered_sev.get(state.severity_cursor)
                        && !state.selected_severities.remove(val)
                    {
                        state.selected_severities.insert(val.clone());
                    }
                } else if let Some(&val) = filtered_svc.get(state.service_cursor)
                    && !state.selected_services.remove(val)
                {
                    state.selected_services.insert(val.clone());
                }
            }
            KeyCode::Char(c) => {
                state.filter_text.push(c);
                // Reset cursor to 0 since list changed.
                if state.panel == 0 {
                    state.severity_cursor = 0;
                    state.severity_scroll = 0;
                } else {
                    state.service_cursor = 0;
                    state.service_scroll = 0;
                }
            }
            KeyCode::Backspace => {
                state.filter_text.pop();
                if state.panel == 0 {
                    state.severity_cursor = 0;
                    state.severity_scroll = 0;
                } else {
                    state.service_cursor = 0;
                    state.service_scroll = 0;
                }
            }
            KeyCode::Enter => {
                self.apply_field_filter();
                return Ok(false);
            }
            _ => {}
        }

        // Clamp cursor to filtered list size and keep scroll in sync.
        if let Some(state) = &mut self.field_filter_state {
            let active_count = if state.panel == 0 {
                sev_list.iter().filter(|s| state.filter_text.is_empty() || s.to_lowercase().contains(&state.filter_text.to_lowercase())).count()
            } else {
                svc_list.iter().filter(|s| state.filter_text.is_empty() || s.to_lowercase().contains(&state.filter_text.to_lowercase())).count()
            };
            if state.panel == 0 {
                if active_count > 0 {
                    state.severity_cursor = state.severity_cursor.min(active_count - 1);
                } else {
                    state.severity_cursor = 0;
                }
                let row = state.severity_cursor as u16 + 1;
                if row < state.severity_scroll {
                    state.severity_scroll = row;
                } else if row >= state.severity_scroll + visible_rows as u16 {
                    state.severity_scroll = row - visible_rows as u16 + 1;
                }
            } else {
                if active_count > 0 {
                    state.service_cursor = state.service_cursor.min(active_count - 1);
                } else {
                    state.service_cursor = 0;
                }
                let row = state.service_cursor as u16 + 1;
                if row < state.service_scroll {
                    state.service_scroll = row;
                } else if row >= state.service_scroll + visible_rows as u16 {
                    state.service_scroll = row - visible_rows as u16 + 1;
                }
            }
        }

        Ok(false)
    }

    fn apply_field_filter(&mut self) {
        if let Some(state) = self.field_filter_state.take() {
            self.active_field_filter = FieldFilter {
                severities: if state.selected_severities.is_empty() { None } else { Some(state.selected_severities) },
                services: if state.selected_services.is_empty() { None } else { Some(state.selected_services) },
            };
            self.focus = Focus::List;
            self.status = if self.active_field_filter.is_empty() {
                "Field filter cleared".to_string()
            } else {
                let parts: Vec<String> = [
                    self.active_field_filter.severities.as_ref().map(|s| format!("severity: {}", s.iter().cloned().collect::<Vec<_>>().join(", "))),
                    self.active_field_filter.services.as_ref().map(|s| format!("service: {}", s.iter().cloned().collect::<Vec<_>>().join(", "))),
                ]
                .into_iter()
                .flatten()
                .collect();
                format!("Field filter: {}", parts.join(" | "))
            };
            // Re-scan with the new field filter
            let _ = self.apply_filter();
        }
    }

    fn cancel_scan(&mut self) {
        if let Some(scan) = &self.current_scan {
            scan.cancel();
        }
    }

    fn open_dedup_prompt(&mut self) {
        let stem = self.input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
        self.dedup_filename = format!("{stem}-dedup.logjet");
        self.dedup_behavior = DedupMode::Distinct;
        self.dedup_match_mode = DedupMatchMode::Hash2;
        self.focus = Focus::DedupPrompt;
    }

    fn handle_dedup_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Enter => {
                let filename = self.dedup_filename.clone();
                if !filename.is_empty() {
                    self.start_dedup(&filename, self.dedup_behavior, self.dedup_match_mode);
                }
                Ok(false)
            }
            KeyCode::Esc => {
                self.focus = Focus::List;
                Ok(false)
            }
            KeyCode::Left => {
                self.dedup_behavior = self.dedup_behavior.prev();
                Ok(false)
            }
            KeyCode::Right => {
                self.dedup_behavior = self.dedup_behavior.next();
                Ok(false)
            }
            KeyCode::Up => {
                self.dedup_match_mode = self.dedup_match_mode.prev();
                Ok(false)
            }
            KeyCode::Down | KeyCode::Tab => {
                self.dedup_match_mode = self.dedup_match_mode.next();
                Ok(false)
            }
            KeyCode::Backspace => {
                self.dedup_filename.pop();
                Ok(false)
            }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.dedup_filename.push(c);
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn start_dedup(&mut self, filename: &str, behavior: DedupMode, match_mode: DedupMatchMode) {
        let input_path = self.input.clone();
        let output_dir = self.input.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let output_path = output_dir.join(filename);
        let (tx, rx) = mpsc::channel();
        self.dedup_rx = Some(rx);
        self.dedup_output_path = Some(output_path.clone());
        self.dedup_progress = 0.0;
        self.dedup_progress_target = 0.0;
        self.dedup_phase = "starting".to_string();
        self.dedup_completion_message = None;
        self.focus = Focus::DedupProgress;

        thread::spawn(move || {
            let run = || -> std::result::Result<crate::dedup::DedupStats, String> {
                tx.send(DedupUpdate::Progress { ratio: 0.05, phase: "opening input".to_string() }).ok();
                let input = crate::input::InputHandle::open(&input_path).map_err(|e| e.to_string())?;
                tx.send(DedupUpdate::Progress { ratio: 0.18, phase: "unpacking records".to_string() }).ok();
                let mut reader = LogjetReader::new(input.into_buf_reader());
                let unpacked = crate::dedup::unpack::unpack(&mut reader).map_err(|e| e.to_string())?;
                tx.send(DedupUpdate::Progress { ratio: 0.32, phase: "preparing output".to_string() }).ok();

                let out_file = File::create(&output_path).map_err(|e| e.to_string())?;
                let mut writer = LogjetWriter::new(BufWriter::new(out_file));
                let phase = format!("running {} / {}", behavior.label(), match_mode.label());
                tx.send(DedupUpdate::Progress { ratio: 0.82, phase }).ok();

                let opts = crate::dedup::DedupOpts { mode: behavior, match_mode, ..crate::dedup::DedupOpts::default() };
                let stats = crate::dedup::dedup(unpacked.records, unpacked.passthrough, &mut writer, &opts).map_err(|e| e.to_string())?;

                tx.send(DedupUpdate::Progress { ratio: 0.94, phase: "flushing output".to_string() }).ok();
                let mut out = writer.into_inner().map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
                Ok(stats)
            };
            match run() {
                Ok(stats) => {
                    let _ = tx.send(DedupUpdate::Done { total: stats.total_records, groups: stats.group_count, pct: stats.reduction_pct() });
                }
                Err(e) => {
                    let _ = tx.send(DedupUpdate::Failed(e));
                }
            }
        });
    }

    fn drain_dedup_updates(&mut self) {
        let Some(rx) = &self.dedup_rx else { return };
        while let Ok(update) = rx.try_recv() {
            match update {
                DedupUpdate::Progress { ratio, phase } => {
                    self.dedup_progress_target = ratio;
                    self.dedup_phase = phase;
                }
                DedupUpdate::Done { total, groups, pct } => {
                    self.dedup_progress = 1.0;
                    self.dedup_progress_target = 1.0;
                    self.dedup_phase = "OK".to_string();
                    self.dedup_rx = None;
                    self.dedup_completion_message = Some(format!("{total} records → {groups} groups ({pct:.1}% reduction)"));
                    return;
                }
                DedupUpdate::Failed(e) => {
                    self.status = format!("Dedup failed: {e}");
                    self.dedup_rx = None;
                    self.dedup_output_path = None;
                    self.focus = Focus::List;
                    return;
                }
            }
        }
        if self.dedup_progress < self.dedup_progress_target {
            self.dedup_progress = (self.dedup_progress + 0.015).min(self.dedup_progress_target);
        }
    }

    fn handle_dedup_progress_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.dedup_rx.is_some() {
            return Ok(false);
        }

        match key.code {
            KeyCode::Enter => {
                let msg = self.dedup_completion_message.take();
                if let Some(path) = self.dedup_output_path.take() {
                    self.switch_to_file(path)?;
                } else {
                    self.focus = Focus::List;
                }
                if let Some(msg) = msg {
                    self.status = format!("Dedup: {msg}");
                }
                Ok(false)
            }
            KeyCode::Esc => {
                self.dedup_completion_message = None;
                self.dedup_output_path = None;
                self.focus = Focus::List;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Length(1)])
            .split(frame.area());

        self.render_search(frame, areas[0]);

        let body =
            Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(64), Constraint::Percentage(36)]).split(areas[1]);

        self.render_list(frame, body[0]);
        self.render_details(frame, body[1]);
        self.render_status(frame, areas[2]);

        if self.focus == Focus::Modal {
            self.render_modal(frame);
        } else if self.focus == Focus::FieldFilter {
            self.render_field_filter(frame);
        } else if self.focus == Focus::SaveError {
            self.render_save_error(frame);
        } else if self.focus == Focus::ExportError {
            self.render_export_error(frame);
        } else if self.focus == Focus::SavePrompt {
            self.render_save_prompt(frame);
        } else if self.focus == Focus::ExportPrompt {
            self.render_export_prompt(frame);
        } else if self.focus == Focus::DedupPrompt {
            self.render_dedup_prompt(frame);
        } else if self.focus == Focus::DedupProgress {
            self.render_dedup_progress(frame);
        }
    }

    fn render_search(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = match self.filter_mode {
            FilterMode::Strings => " Filter (strings) ",
            FilterMode::Regex => " Filter (regex) ",
        };
        let block = pane_block(title, self.focus == Focus::Search);
        let paragraph = Paragraph::new(self.query_input.as_str()).block(block).style(Style::default().fg(Color::White));
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
                    Style::default().fg(Color::White).bg(Color::Indexed(28)).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let summary = self.summary_for(index).unwrap_or_else(|_| "<failed to render summary>".to_string());
                let summary = fit_to_width(&summary, row_width);
                lines.push(Line::from(Span::styled(summary, style)));
            }
        }

        let paragraph = Paragraph::new(Text::from(lines)).scroll((0, 0)).wrap(Wrap { trim: false }).style(Style::default().fg(Color::White));
        frame.render_widget(paragraph, inner);

        if !self.entries.is_empty() {
            let mut scrollbar_state = ScrollbarState::new(self.entries.len()).position(self.selected.min(self.entries.len().saturating_sub(1)));
            frame.render_stateful_widget(Scrollbar::new(ScrollbarOrientation::VerticalRight), inner, &mut scrollbar_state);
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

        let paragraph =
            Paragraph::new(Text::from(lines)).scroll((self.detail_scroll, 0)).wrap(Wrap { trim: false }).style(Style::default().fg(Color::White));
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
                    status_text(" close   "),
                    status_key("UP/DOWN"),
                    status_text(" scroll   "),
                    status_key("LEFT/RIGHT"),
                    status_text(" prev/next   "),
                    status_key("I"),
                    status_text(" info panel"),
                ],
            );
            return;
        }
        if self.focus == Focus::SavePrompt {
            draw_status_spans(buf, area.x, y, area.width, &[status_key("ENTER"), status_text(" save   "), status_key("ESC"), status_text(" cancel")]);
            return;
        }
        if self.focus == Focus::SaveError {
            draw_status_spans(buf, area.x, y, area.width, &[status_text("Press any key to return")]);
            return;
        }
        if self.focus == Focus::ExportPrompt {
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
        if self.focus == Focus::ExportError {
            draw_status_spans(buf, area.x, y, area.width, &[status_text("Press any key to return")]);
            return;
        }
        if self.focus == Focus::DedupPrompt {
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
        if self.focus == Focus::DedupProgress {
            draw_status_spans(buf, area.x, y, area.width, &[status_text("Deduplicating…")]);
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
            buf.set_stringn(status_x, y, status, status_width as usize, Style::default().fg(Color::LightGreen).bg(Color::Indexed(28)));
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
        let cursor_y = row.y;
        frame.set_cursor_position((cursor_x, cursor_y));
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
        let area = centered_rect(62, 16, frame.area());
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

        let hint_row = Rect { x: inner.x, y: inner.y.saturating_add(5), width: inner.width, height: 3 };
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::from("Format: use ←/→ or SPACE to choose (built-in + plugins)"),
                Line::from("Range:  a / all  |  c / current / 0  |  N  |  N-N"),
                Line::from("Uses the current filtered view order."),
            ]))
            .style(Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            hint_row,
        );

        let (cursor_x, cursor_y) = match self.export_field {
            ExportField::Format => {
                let x = format_row
                    .x
                    .saturating_add(format_label.chars().count() as u16)
                    .saturating_add(2)
                    .min(format_row.x.saturating_add(format_label.chars().count() as u16 + format_width));
                (x, format_row.y)
            }
            ExportField::Filename => {
                let x = filename_row
                    .x
                    .saturating_add(filename_label.chars().count() as u16)
                    .saturating_add(self.export_filename_cursor as u16)
                    .min(filename_row.x.saturating_add(filename_label.chars().count() as u16 + filename_width));
                (x, filename_row.y)
            }
            ExportField::Range => {
                let x = range_row
                    .x
                    .saturating_add(range_label.chars().count() as u16)
                    .saturating_add(self.export_range_cursor as u16)
                    .min(range_row.x.saturating_add(range_label.chars().count() as u16 + range_width));
                (x, range_row.y)
            }
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

        // Fixed width: 80% of screen.
        let popup_width = (screen.width * 80 / 100).max(20);
        let inner_width = popup_width.saturating_sub(2);

        if !self.modal_info_visible {
            // ── Collapsed mode: full-width log view, no info panel ───────
            let wrap_width = inner_width.saturating_sub(1) as usize; // -1 for scrollbar
            let wrapped = smart_wrap(message, wrap_width);
            let left_lines = wrapped.lines().count() as u16;
            let info_lines = if let Some(detail) = &self.selected_detail { render_modal_info_entries(detail).len() as u16 } else { 0 };
            let min_height = info_lines + 3;
            let desired_height = left_lines + 3;
            let max_height = screen.height * 80 / 100;
            let popup_height = desired_height.max(min_height).min(max_height).max(5);

            let x = screen.width.saturating_sub(popup_width) / 2;
            let y = screen.height.saturating_sub(popup_height) / 2;
            let area = Rect::new(x, y, popup_width, popup_height);
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

            let paragraph = Paragraph::new(wrapped.as_str()).style(Style::default().fg(Color::Black).bg(Color::Gray)).scroll((self.modal_scroll, 0));
            frame.render_widget(paragraph, chunks[0]);

            // Scrollbar
            let mut scrollbar_state = ScrollbarState::new(left_lines as usize).position(self.modal_scroll as usize);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(Color::Black).bg(Color::Gray)),
                chunks[0],
                &mut scrollbar_state,
            );

            let footer = if let Some(detail) = &self.selected_detail { render_modal_footer(detail) } else { render_modal_footer_placeholder() };
            frame.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Blue)), chunks[1]);
            return;
        }

        // ── Expanded mode: message + divider + info panel ────────────────

        // Compute info entries and column widths.
        let info_entries = if let Some(detail) = &self.selected_detail {
            render_modal_info_entries(detail)
        } else {
            vec![("info".to_string(), "No record loaded.".to_string())]
        };
        let key_width = info_entries.iter().map(|(key, _)| key.chars().count()).max().unwrap_or(4).max(4);
        let preferred_info_width =
            info_entries.iter().map(|(_, value)| (key_width + 2 + value.chars().count() + 1) as u16).max().unwrap_or((key_width + 3) as u16);
        let max_info_width = inner_width.saturating_div(2).max(16);
        let info_width = preferred_info_width.min(max_info_width).max(16);
        let divider_width: u16 = 1;
        let message_width = inner_width.saturating_sub(info_width).saturating_sub(divider_width);

        // Measure content heights.
        let right_lines = info_entries.len() as u16;
        let wrap_width = message_width.saturating_sub(1) as usize; // -1 for scrollbar
        let wrapped = smart_wrap(message, wrap_width);
        let left_lines = wrapped.lines().count() as u16;
        let body_height = right_lines.max(left_lines);

        let desired_height = body_height + 3;
        let max_height = screen.height * 80 / 100;
        let popup_height = desired_height.min(max_height).max(5);

        let x = screen.width.saturating_sub(popup_width) / 2;
        let y = screen.height.saturating_sub(popup_height) / 2;
        let area = Rect::new(x, y, popup_width, popup_height);
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

        let footer = if let Some(detail) = &self.selected_detail { render_modal_footer(detail) } else { render_modal_footer_placeholder() };
        let paragraph = Paragraph::new(wrapped.as_str()).style(Style::default().fg(Color::Black).bg(Color::Gray)).scroll((self.modal_scroll, 0));
        frame.render_widget(paragraph, body[0]);

        // Scrollbar on the left panel — always visible.
        {
            let mut scrollbar_state = ScrollbarState::new(left_lines as usize).position(self.modal_scroll as usize);
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(Color::Black).bg(Color::Gray)),
                body[0],
                &mut scrollbar_state,
            );
        }

        let value_width = info_width.saturating_sub((key_width + 2 + 1) as u16) as usize;
        let info_lines = info_entries.into_iter().map(|(key, value)| modal_info_line(&key, value, key_width, value_width)).collect::<Vec<_>>();
        let info = Paragraph::new(Text::from(info_lines)).style(Style::default().fg(Color::White).bg(Color::Indexed(30))).scroll((0, 0));
        frame.render_widget(info, body[2]);
        frame.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Blue)), chunks[1]);

        // Repaint the double-frame border: grey side = black on grey, cyan side = white on cyan.
        let split_x = body[1].x;
        let grey_style = Style::default().fg(Color::Black).bg(Color::Gray);
        let cyan_style = Style::default().fg(Color::White).bg(Color::Indexed(30));
        let buf = frame.buffer_mut();
        let top = area.y;
        let bot = area.y + area.height.saturating_sub(1);
        let left = area.x;
        let right = area.x + area.width.saturating_sub(1);

        // Top and bottom edges
        for x in left..=right {
            let s = if x >= split_x { cyan_style } else { grey_style };
            buf[(x, top)].set_style(s);
            buf[(x, bot)].set_style(s);
        }
        // Left edge (grey side)
        for y in top..=bot {
            buf[(left, y)].set_style(grey_style);
        }
        // Right edge (cyan side)
        for y in top..=bot {
            buf[(right, y)].set_style(cyan_style);
        }
    }

    fn render_field_filter(&self, frame: &mut Frame<'_>) {
        let catalog = self.field_catalog.lock().unwrap();
        let Some(cat) = catalog.as_ref() else { return };
        let Some(state) = &self.field_filter_state else { return };

        let screen = frame.area();
        let filter_lower = state.filter_text.to_lowercase();

        // Filter lists: active panel gets filtered, inactive shows all.
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
        let max_popup_h = screen.height * 60 / 100;
        let popup_h = (body_height + 4).clamp(20, max_popup_h);
        let popup_w = (screen.width * 60 / 100).max(40);
        let x = screen.width.saturating_sub(popup_w) / 2;
        let y = screen.height * 20 / 100;
        let area = Rect::new(x, y, popup_w, popup_h);
        frame.render_widget(Clear, area);

        // DOS-style inverted title: white bg, black text. Search text in yellow on teal.
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

        // Severity panel — left-aligned header, bold blue.
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

        // Services panel — left-aligned header, bold blue.
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

        // DOS-blue status bar.
        let footer_area = Rect::new(inner.x, inner.y + inner.height.saturating_sub(1), inner.width, 1);
        let footer = Line::from(vec![
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
        ]);
        frame.render_widget(Paragraph::new(footer).style(Style::default().bg(Color::Blue)), footer_area);
    }

    fn render_dedup_prompt(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(52, 12, frame.area());
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

        let mode_row = Rect { x: inner.x, y: inner.y.saturating_add(2), width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Mode:   ", Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(self.dedup_behavior.label(), Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", self.dedup_behavior.description()), Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            mode_row,
        );

        let matcher_row = Rect { x: inner.x, y: inner.y.saturating_add(3), width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Match:  ", Style::default().fg(Color::Black).bg(Color::Gray)),
                Span::styled(self.dedup_match_mode.label(), Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", self.dedup_match_mode.description()), Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            matcher_row,
        );

        // Footer hint
        let hint_y = inner.y.saturating_add(5).min(inner.y + inner.height.saturating_sub(1));
        let hint_area = Rect { x: inner.x, y: hint_y, width: inner.width, height: 1 };
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
            hint_area,
        );
    }

    fn render_dedup_progress(&self, frame: &mut Frame<'_>) {
        let area = centered_rect(52, 10, frame.area());
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
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Indexed(28)).bg(Color::White))
            .label(Span::styled(label, label_style))
            .ratio(self.dedup_progress.clamp(0.0, 1.0));
        let bar_area = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: 1 };
        frame.render_widget(gauge, bar_area);
        let phase_area = Rect { x: inner.x + 1, y: inner.y + 3, width: inner.width.saturating_sub(2), height: 1 };
        let phase_text = if self.dedup_completion_message.is_some() { "Press ENTER to open the deduped file" } else { self.dedup_phase.as_str() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Phase: ", Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)),
                Span::styled(phase_text, Style::default().fg(Color::DarkGray).bg(Color::Gray)),
            ])),
            phase_area,
        );
    }

    fn switch_to_file(&mut self, path: PathBuf) -> Result<()> {
        self.cancel_scan();
        if let Some(scan) = self.current_scan.take() {
            drop(scan.spool_reader);
            let _ = std::fs::remove_file(scan.spool_path);
        }
        self.input = path;

        // Re-launch background field catalog scan for the new file.
        let catalog_bg = Arc::clone(&self.field_catalog);
        let input_bg = self.input.clone();
        *self.field_catalog.lock().unwrap() = None;
        thread::spawn(move || {
            if let Ok(cat) = scan_field_catalog(&input_bg) {
                *catalog_bg.lock().unwrap() = Some(cat);
            }
        });

        self.query_input.clear();
        self.apply_filter()
    }
}

/// Scans the logjet file in the background to collect distinct severity texts and service names.
fn scan_field_catalog(input: &Path) -> Result<FieldCatalog> {
    let handle = InputHandle::open(input)?;
    let mut reader = LogjetReader::new(handle.into_buf_reader());
    let mut severities = HashSet::new();
    let mut services = HashSet::new();

    while let Some(record) = reader.next_record()? {
        if record.record_type != RecordType::Logs {
            continue;
        }
        if let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) {
            for rl in &batch.resource_logs {
                if let Some(res) = &rl.resource {
                    for attr in &res.attributes {
                        if attr.key == "service.name"
                            && let Some(AnyValue { value: Some(Value::StringValue(s)) }) = &attr.value
                        {
                            services.insert(s.clone());
                        }
                    }
                }
                for sl in &rl.scope_logs {
                    for lr in &sl.log_records {
                        if !lr.severity_text.is_empty() {
                            severities.insert(lr.severity_text.clone());
                        }
                    }
                }
            }
        }
    }

    let mut severities: Vec<_> = severities.into_iter().collect();
    let mut services: Vec<_> = services.into_iter().collect();
    severities.sort();
    services.sort();
    Ok(FieldCatalog { severities, services })
}

fn scan_matches(
    input_path: &Path, predicate: crate::predicate::RecordPredicate, mut spool: File, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ScanUpdate>,
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
        scanned = scanned.checked_add(1).ok_or(logjet::Error::NumericOverflow("view scanned"))?;

        if predicate.matches(&record) {
            let meta = write_spool_record(&mut spool, &record)?;
            tx_batch.push(meta);
            matched = matched.checked_add(1).ok_or(logjet::Error::NumericOverflow("view matched"))?;

            if tx_batch.len() >= SCAN_BATCH_SIZE {
                tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
            }
        }
    }

    if !tx_batch.is_empty() {
        tx.send(ScanUpdate::Batch(tx_batch)).map_err(|err| Error::Usage(err.to_string()))?;
    }

    Ok((scanned, matched))
}

fn write_spool_record(file: &mut File, record: &OwnedRecord) -> Result<EntryMeta> {
    let offset = file.seek(SeekFrom::End(0))?;
    file.write_all(&[record.record_type as u8])?;
    file.write_all(&record.seq.to_le_bytes())?;
    file.write_all(&record.ts_unix_ns.to_le_bytes())?;
    let payload_len = u64::try_from(record.payload.len()).map_err(|_| logjet::Error::NumericOverflow("view payload_len"))?;
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&record.payload)?;
    file.flush()?;

    Ok(EntryMeta { offset, record_type: record.record_type, seq: record.seq, ts_unix_ns: record.ts_unix_ns, payload_len })
}

fn read_spool_record(file: &mut File, meta: EntryMeta) -> Result<DetailRecord> {
    file.seek(SeekFrom::Start(meta.offset + 1 + 8 + 8 + 8))?;
    let mut payload = vec![0u8; meta.payload_len as usize];
    file.read_exact(&mut payload)?;
    Ok(DetailRecord { meta, payload })
}

fn remember_summary(cache: &mut HashMap<usize, String>, order: &mut VecDeque<usize>, index: usize, summary: String) {
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
        key_value_line("Timestamp:", format_timestamp(detail.meta.ts_unix_ns), Style::default().fg(Color::White)),
        key_value_line("Payload:", format!("{} bytes", detail.meta.payload_len), Style::default().fg(Color::White)),
        Line::from(""),
    ];

    lines.extend(render_otlp_lines(detail));
    if lines.len() == 5 {
        let preview = if hex_payload { hex_preview(&detail.payload, 64) } else { text_preview(&detail.payload, DETAIL_PREVIEW_BYTES) };
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
                if !log_record.severity_text.is_empty() && !severities.iter().any(|existing| existing == &log_record.severity_text) {
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
        lines.push(key_value_line("Services:", services.join(", "), Style::default().fg(Color::White)));
    }
    if !severities.is_empty() {
        lines.push(key_value_line("Severity:", severities.join(", "), severity_style(severities.first().map(String::as_str).unwrap_or(""))));
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

    if hex_payload { hex_dump(&detail.payload) } else { String::from_utf8_lossy(&detail.payload).into_owned() }
}

fn render_modal_footer(detail: &DetailRecord) -> Line<'static> {
    let (size_num, size_unit) = format_size_parts(detail.meta.payload_len);
    Line::from(vec![
        Span::styled(format!("#{}", detail.meta.seq), Style::default().fg(Color::LightGreen)),
        footer_sep(),
        Span::styled(format_timestamp(detail.meta.ts_unix_ns), Style::default().fg(Color::White)),
        footer_sep(),
        Span::styled(record_kind_label(detail.meta.record_type).to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        footer_sep(),
        Span::styled(size_num, Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::styled(size_unit, Style::default().fg(Color::White)),
    ])
}

fn render_modal_footer_placeholder() -> Line<'static> {
    Line::from(vec![
        Span::styled("#", Style::default().fg(Color::LightGreen)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::White)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        footer_sep(),
        Span::styled("", Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::styled("", Style::default().fg(Color::White)),
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
    let mut scope_attr_count = 0usize;
    let mut record_attr_count = 0usize;
    let mut trace_ids = 0usize;
    let mut span_ids = 0usize;
    let mut resource_attr_entries = Vec::new();
    let mut scope_attr_entries = Vec::new();
    let mut record_attr_entries = Vec::new();
    let mut resource_attr_omitted = 0usize;
    let mut scope_attr_omitted = 0usize;
    let mut record_attr_omitted = 0usize;

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
                push_modal_attribute_entry(&mut resource_attr_entries, &mut resource_attr_omitted, "resource", &attr.key, attr.value.as_ref());
            }
        }

        for scope_logs in &resource_logs.scope_logs {
            if let Some(scope) = &scope_logs.scope
                && !scope.name.is_empty()
                && !scopes.iter().any(|existing| existing == &scope.name)
            {
                scopes.push(scope.name.clone());
            }
            if let Some(scope) = &scope_logs.scope {
                scope_attr_count += scope.attributes.len();
                for attr in &scope.attributes {
                    push_modal_attribute_entry(&mut scope_attr_entries, &mut scope_attr_omitted, "scope", &attr.key, attr.value.as_ref());
                }
            }
            for record in &scope_logs.log_records {
                record_attr_count += record.attributes.len();
                if !record.severity_text.is_empty() && !severities.iter().any(|existing| existing == &record.severity_text) {
                    severities.push(record.severity_text.clone());
                }
                if !record.event_name.is_empty() && !event_names.iter().any(|existing| existing == &record.event_name) {
                    event_names.push(record.event_name.clone());
                }
                if !record.trace_id.is_empty() {
                    trace_ids += 1;
                }
                if !record.span_id.is_empty() {
                    span_ids += 1;
                }
                for attr in &record.attributes {
                    push_modal_attribute_entry(&mut record_attr_entries, &mut record_attr_omitted, "record", &attr.key, attr.value.as_ref());
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
    lines.push(("scope.attrs".to_string(), scope_attr_count.to_string()));
    lines.push(("record.attrs".to_string(), record_attr_count.to_string()));
    for (kind, key, value) in resource_attr_entries {
        if kind.is_empty() && key.is_empty() {
            lines.push((String::new(), value));
        } else {
            lines.push((format!("{kind}.{key}"), value));
        }
    }
    if resource_attr_omitted > 0 {
        lines.push(("resource.attrs.more".to_string(), format!("{resource_attr_omitted} not shown")));
    }
    for (kind, key, value) in scope_attr_entries {
        if kind.is_empty() && key.is_empty() {
            lines.push((String::new(), value));
        } else {
            lines.push((format!("{kind}.{key}"), value));
        }
    }
    if scope_attr_omitted > 0 {
        lines.push(("scope.attrs.more".to_string(), format!("{scope_attr_omitted} not shown")));
    }
    for (kind, key, value) in record_attr_entries {
        if kind.is_empty() && key.is_empty() {
            lines.push((String::new(), value));
        } else {
            lines.push((format!("{kind}.{key}"), value));
        }
    }
    if record_attr_omitted > 0 {
        lines.push(("record.attrs.more".to_string(), format!("{record_attr_omitted} not shown")));
    }
    if trace_ids > 0 {
        lines.push(("trace_id".to_string(), format!("{trace_ids} present")));
    }
    if span_ids > 0 {
        lines.push(("span_id".to_string(), format!("{span_ids} present")));
    }

    lines
}

fn parse_export_selection(input: &str, total: usize, selected: usize) -> std::result::Result<(usize, usize), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Range must not be empty.".to_string());
    }
    if trimmed.eq_ignore_ascii_case("all") || trimmed.eq_ignore_ascii_case("a") {
        return Ok((0, total));
    }
    if trimmed.eq_ignore_ascii_case("current") || trimmed.eq_ignore_ascii_case("c") || trimmed == "0" {
        if total == 0 {
            return Ok((0, 0));
        }
        let current = selected.min(total.saturating_sub(1));
        return Ok((current, current + 1));
    }
    if let Some((start, end)) = trimmed.split_once('-') {
        let start = start.trim().parse::<usize>().map_err(|_| "Invalid range start.".to_string())?;
        let end = end.trim().parse::<usize>().map_err(|_| "Invalid range end.".to_string())?;
        if start == 0 || end == 0 {
            return Err("Range is 1-based; values must be >= 1.".to_string());
        }
        if start > end {
            return Err("Range start must be <= end.".to_string());
        }
        if start > total {
            return Ok((total, total));
        }
        return Ok((start - 1, end.min(total)));
    }

    let amount = trimmed.parse::<usize>().map_err(|_| "Range must be all/a, current/c/0, N, or N-N.".to_string())?;
    if amount == 0 {
        return Err("Amount must be >= 1.".to_string());
    }
    Ok((0, amount.min(total)))
}

fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn char_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices().nth(char_idx).map(|(idx, _)| idx).unwrap_or(text.len())
}

fn insert_char_at(text: &mut String, cursor: &mut usize, ch: char) {
    let idx = char_to_byte_idx(text, *cursor);
    text.insert(idx, ch);
    *cursor += 1;
}

fn delete_char_before(text: &mut String, cursor: &mut usize) {
    if *cursor == 0 {
        return;
    }
    let end = char_to_byte_idx(text, *cursor);
    let start = char_to_byte_idx(text, cursor.saturating_sub(1));
    text.replace_range(start..end, "");
    *cursor = cursor.saturating_sub(1);
}

fn delete_char_at(text: &mut String, cursor: usize) {
    if cursor >= char_count(text) {
        return;
    }
    let start = char_to_byte_idx(text, cursor);
    let end = char_to_byte_idx(text, cursor + 1);
    text.replace_range(start..end, "");
}

fn export_ndjson_objects(detail: &DetailRecord) -> Vec<JsonValue> {
    if detail.meta.record_type != RecordType::Logs {
        let mut obj = JsonMap::new();
        obj.insert("record_type".to_string(), JsonValue::String(record_kind_label(detail.meta.record_type).to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(detail.meta.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&detail.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        let mut obj = JsonMap::new();
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(detail.meta.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&detail.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    };

    let mut out = Vec::new();
    for resource_logs in &batch.resource_logs {
        let resource_attrs = resource_logs.resource.as_ref().map(|r| &r.attributes).map(Vec::as_slice).unwrap_or(&[]);
        for scope_logs in &resource_logs.scope_logs {
            let scope_attrs = scope_logs.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]);
            for record in &scope_logs.log_records {
                let mut obj = JsonMap::new();
                obj.insert(
                    "body".to_string(),
                    JsonValue::String(record.body.as_ref().map(|v| format_any_value(Some(v))).filter(|s| !s.is_empty()).unwrap_or_default()),
                );
                obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.time_unix_nano.max(detail.meta.ts_unix_ns))));
                if !record.event_name.is_empty() {
                    obj.insert("event_name".to_string(), JsonValue::String(record.event_name.clone()));
                }
                flatten_otlp_attrs_into_json(&mut obj, resource_attrs);
                flatten_otlp_attrs_into_json(&mut obj, scope_attrs);
                flatten_otlp_attrs_into_json(&mut obj, &record.attributes);
                out.push(JsonValue::Object(obj));
            }
        }
    }
    out
}

fn flatten_otlp_attrs_into_json(target: &mut JsonMap<String, JsonValue>, attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue]) {
    for attr in attrs {
        let key = attr.key.replace('.', "_");
        if target.contains_key(&key) {
            continue;
        }
        let Some(value) = attr.value.as_ref() else {
            continue;
        };
        if let Some(json) = any_value_to_json(value) {
            target.insert(key, json);
        }
    }
}

fn any_value_to_json(value: &AnyValue) -> Option<JsonValue> {
    match &value.value {
        Some(Value::StringValue(text)) => Some(JsonValue::String(text.clone())),
        Some(Value::BoolValue(flag)) => Some(JsonValue::Bool(*flag)),
        Some(Value::IntValue(number)) => Some(JsonValue::Number((*number).into())),
        Some(Value::DoubleValue(number)) => serde_json::Number::from_f64(*number).map(JsonValue::Number),
        Some(Value::BytesValue(bytes)) => Some(JsonValue::String(format!("<{} bytes>", bytes.len()))),
        Some(Value::ArrayValue(array)) => Some(JsonValue::Array(array.values.iter().filter_map(any_value_to_json).collect())),
        Some(Value::KvlistValue(map)) => {
            let mut obj = JsonMap::new();
            for item in &map.values {
                if let Some(inner) = item.value.as_ref().and_then(any_value_to_json) {
                    obj.insert(item.key.clone().replace('.', "_"), inner);
                }
            }
            Some(JsonValue::Object(obj))
        }
        None => None,
    }
}

fn push_modal_attribute_entry(entries: &mut Vec<(String, String, String)>, omitted: &mut usize, kind: &str, key: &str, value: Option<&AnyValue>) {
    let Some(any) = value else {
        if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
            entries.push((kind.to_string(), key.to_string(), "null".to_string()));
        } else {
            *omitted += 1;
        }
        return;
    };

    // Expand arrays: first element on the key line, rest as continuation lines.
    if let Some(Value::ArrayValue(array)) = &any.value {
        let elements: Vec<String> = array.values.iter().map(|e| format_any_value(Some(e))).filter(|s| !s.is_empty()).collect();
        if elements.is_empty() {
            if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
                entries.push((kind.to_string(), key.to_string(), "\x00N/A".to_string()));
            } else {
                *omitted += 1;
            }
        } else {
            for (i, formatted) in elements.into_iter().enumerate() {
                if entries.len() >= MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
                    *omitted += 1;
                    continue;
                }
                if i == 0 {
                    entries.push((kind.to_string(), key.to_string(), formatted));
                } else {
                    entries.push((String::new(), String::new(), formatted));
                }
            }
        }
        return;
    }

    if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
        entries.push((kind.to_string(), key.to_string(), format_any_value(value)));
    } else {
        *omitted += 1;
    }
}

fn modal_info_line(key: &str, value: String, key_width: usize, value_width: usize) -> Line<'static> {
    let bg = Color::Indexed(30);

    // Empty array → red "N/A" (checked before trim which would strip the sentinel).
    if value == "\x00N/A" {
        let key_style = if is_otlp_attribute_entry(key) && !is_standard_otlp_attribute_entry(key) {
            Style::default().fg(Color::LightCyan).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::LightYellow).bg(bg).add_modifier(Modifier::BOLD)
        };
        return Line::from(vec![
            Span::styled(format!("{key:<width$}: ", width = key_width), key_style),
            Span::styled("N/A", Style::default().fg(Color::Red).bg(bg).add_modifier(Modifier::BOLD)),
        ]);
    }

    let value = trim_single_line(&value, value_width);

    // Continuation line (array element after the first) — indent to value column.
    if key.is_empty() {
        let value_style = Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD);
        return Line::from(vec![
            Span::styled(format!("{:<width$}  ", "", width = key_width), Style::default().bg(bg)),
            Span::styled(value, value_style),
        ]);
    }

    let (key_style, value_style) = if is_otlp_attribute_entry(key) && !is_standard_otlp_attribute_entry(key) {
        // Custom attributes (demo.*): bold bright-cyan key, bold bright-white value
        (
            Style::default().fg(Color::LightCyan).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        // Standard keys + non-attribute entries: bold bright-yellow key, bright-white value
        (Style::default().fg(Color::LightYellow).bg(bg).add_modifier(Modifier::BOLD), Style::default().fg(Color::White).bg(bg))
    };
    Line::from(vec![Span::styled(format!("{key:<width$}: ", width = key_width), key_style), Span::styled(value, value_style)])
}

fn format_any_value(value: Option<&AnyValue>) -> String {
    let Some(value) = value else {
        return "null".to_string();
    };
    match &value.value {
        Some(Value::StringValue(text)) => text.clone(),
        Some(Value::BoolValue(flag)) => flag.to_string(),
        Some(Value::IntValue(number)) => number.to_string(),
        Some(Value::DoubleValue(number)) => number.to_string(),
        Some(Value::BytesValue(bytes)) => format!("<{} bytes>", bytes.len()),
        Some(Value::ArrayValue(array)) => format!("<array:{}>", array.values.len()),
        Some(Value::KvlistValue(map)) => format!("<map:{}>", map.values.len()),
        None => "null".to_string(),
    }
}

fn is_otlp_attribute_entry(key: &str) -> bool {
    (key.starts_with("resource.") && key != "resource.attrs")
        || (key.starts_with("scope.") && key != "scope.attrs")
        || (key.starts_with("record.") && key != "record.attrs")
}

fn is_standard_otlp_attribute_entry(key: &str) -> bool {
    let Some((_, attr_key)) = key.split_once('.') else {
        return false;
    };

    const STANDARD_PREFIXES: &[&str] = &[
        "service.",
        "telemetry.",
        "host.",
        "os.",
        "process.",
        "container.",
        "k8s.",
        "cloud.",
        "deployment.",
        "device.",
        "faas.",
        "enduser.",
        "server.",
        "client.",
        "http.",
        "url.",
        "network.",
        "net.",
        "rpc.",
        "db.",
        "messaging.",
        "exception.",
        "code.",
        "thread.",
        "gen_ai.",
        "browser.",
        "user_agent.",
        "aws.",
        "gcp.",
        "azure.",
        "vcs.",
    ];

    STANDARD_PREFIXES.iter().any(|prefix| attr_key.starts_with(prefix))
}

fn footer_sep() -> Span<'static> {
    Span::styled(" | ", Style::default().fg(Color::White))
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

/// Word-wraps `text` at space boundaries. Words longer than `width` are
/// truncated with `…` instead of wrapping — prevents Java FQCNs and other
/// long tokens from creating multi-line visual noise.
fn smart_wrap(text: &str, width: usize) -> String {
    if width == 0 {
        return text.to_string();
    }
    // Expand tabs to spaces — raw \t punches holes in the terminal background.
    let text = text.replace('\t', "    ");
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    for (li, line) in text.split('\n').enumerate() {
        if li > 0 {
            out.push('\n');
        }
        let mut col: usize = 0;
        for word in line.split(' ') {
            let wlen = word.chars().count();
            if wlen == 0 {
                // Consecutive spaces — preserve one space.
                if col > 0 && col < width {
                    out.push(' ');
                    col += 1;
                }
                continue;
            }
            if wlen > width {
                // Long token: start a new line if needed, then truncate.
                if col > 0 {
                    out.push('\n');
                }
                out.extend(word.chars().take(width.saturating_sub(1)));
                out.push('…');
                col = width;
            } else if col + (if col > 0 { 1 } else { 0 }) + wlen > width {
                // Word doesn't fit on current line — wrap.
                out.push('\n');
                out.push_str(word);
                col = wlen;
            } else {
                if col > 0 {
                    out.push(' ');
                    col += 1;
                }
                out.push_str(word);
                col += wlen;
            }
        }
    }
    out
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
    Line::from(vec![Span::styled(format!("{label:<12} "), Style::default().fg(Color::Indexed(136))), Span::styled(value, value_style)])
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
    let mut out = shown.map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" ");
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
    let title_style = if active { Style::default().fg(Color::Black).bg(Color::LightGreen) } else { Style::default().fg(Color::Gray) };
    let border_style = if active { Style::default().fg(Color::LightGreen) } else { Style::default().fg(Color::Gray) };

    Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style)
        .style(Style::default().fg(Color::White))
}

fn status_help_spans(focus: Focus) -> Vec<Span<'static>> {
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
            status_key("F"),
            status_text(" field filter  "),
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

fn status_key(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::White).bg(Color::Indexed(28)).add_modifier(Modifier::BOLD))
}

fn status_text(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(Color::Black).bg(Color::Indexed(28)))
}

fn draw_status_spans(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, width: u16, spans: &[Span<'static>]) {
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

    if let Some(filename) = message.strip_prefix(PREFIX).and_then(|rest| rest.strip_suffix(SUFFIX)) {
        return Line::from(vec![
            Span::styled(PREFIX, Style::default().fg(Color::White).bg(Color::Red)),
            Span::styled(filename.to_string(), Style::default().fg(Color::Yellow).bg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::styled(SUFFIX, Style::default().fg(Color::White).bg(Color::Red)),
        ]);
    }

    Line::from(Span::styled(message.to_string(), Style::default().fg(Color::White).bg(Color::Red)))
}

fn create_temp_path() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|err| Error::Usage(format!("system clock error: {err}")))?.as_nanos();
    for attempt in 0..1000u32 {
        let candidate = base.join(format!("ljx-view-{pid}-{nanos}-{attempt}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::Usage("unable to allocate a temporary view file".to_string()))
}

fn open_temp_spool_pair() -> Result<(PathBuf, File, File)> {
    let spool_path = create_temp_path()?;
    let spool_reader = OpenOptions::new().read(true).write(true).create_new(true).open(&spool_path)?;
    let spool_writer = OpenOptions::new().read(true).write(true).open(&spool_path)?;
    Ok((spool_path, spool_reader, spool_writer))
}

fn write_export_selection_to_temp_logjet(scan: &mut ActiveScan, entries: &[EntryMeta]) -> Result<PathBuf> {
    let temp_input = create_temp_path()?;
    let file = OpenOptions::new().write(true).create_new(true).open(&temp_input)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
    for meta in entries.iter().copied() {
        let detail = read_spool_record(&mut scan.spool_reader, meta)?;
        logjet.push(detail.meta.record_type, detail.meta.seq, detail.meta.ts_unix_ns, &detail.payload)?;
    }
    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    Ok(temp_input)
}

#[cfg(test)]
#[path = "../../tests/unit/commands/view_ut.rs"]
mod view_ut;
