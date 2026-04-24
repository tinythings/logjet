use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use logjet::{LogjetReader, LogjetWriter, WriterConfig};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::detail::{export_ndjson_objects, extract_otlp_log_severity, format_summary, parse_export_selection, render_modal_message};
use super::scan::{
    follow_appended_matches, open_temp_spool_pair, push_preserving_view_order, read_spool_record, remember_summary, scan_field_catalog, scan_matches,
    write_export_selection_to_temp_logjet,
};
use super::text::{char_count, delete_char_at, delete_char_before, insert_char_at};
use super::types::{
    ActiveScan, DedupUpdate, ExportField, ExportUpdate, Focus, ListRowSummary, ScanUpdate, ViewApp, ViewOrder, discover_export_format_choices,
};
use crate::cli::ViewArgs;
use crate::dataset::Dataset;
use crate::dedup::{DedupMatchMode, DedupMode};
use crate::error::{Error, Result};
use crate::input::InputHandle;
use crate::predicate::{FieldFilter, FilterMode, parse_filter_query};

pub(super) const TICK_RATE: Duration = Duration::from_millis(100);

impl ViewApp {
    pub(crate) fn new(args: ViewArgs) -> Result<Self> {
        let dataset = Dataset::from_inputs(&args.inputs)?;
        let exporters = crate::exporter::ExporterRegistry::discover();
        let export_formats = discover_export_format_choices(&exporters);
        let catalog: Arc<std::sync::Mutex<Option<super::types::FieldCatalog>>> = Arc::new(std::sync::Mutex::new(None));
        let catalog_bg = Arc::clone(&catalog);
        let dataset_bg = dataset.clone();
        thread::spawn(move || {
            if let Ok(cat) = scan_field_catalog(&dataset_bg) {
                *catalog_bg.lock().unwrap() = Some(cat);
            }
        });

        Ok(Self {
            input: dataset.primary_path().to_path_buf(),
            dataset,
            view_order: ViewOrder::from(args.dataset_order),
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
            details_visible: false,
            detail_scroll: 0,
            summary_cache: std::collections::HashMap::new(),
            summary_order: std::collections::VecDeque::new(),
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
            export_rx: None,
            export_progress: 0.0,
            export_phase: String::new(),
            current_scan: None,
            tail_on_launch: args.tail,
            tail_mode: false,
            tail_marker_index: None,
            tail_rx: None,
            tail_cancel: None,
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

    pub(super) fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        loop {
            self.drain_scan_updates()?;
            self.drain_export_updates();
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
        if self.tail_mode {
            let continue_with_key =
                matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End)
                    && self.focus == Focus::List;
            self.stop_tail_mode();
            if !continue_with_key {
                return Ok(false);
            }
        }

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
            Focus::ExportProgress => self.handle_export_progress_key(),
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
            KeyCode::Up => self.modal_scroll = self.modal_scroll.saturating_sub(1),
            KeyCode::Down => self.modal_scroll = self.modal_scroll.saturating_add(1),
            KeyCode::PageUp => self.modal_scroll = self.modal_scroll.saturating_sub(10),
            KeyCode::PageDown => self.modal_scroll = self.modal_scroll.saturating_add(10),
            KeyCode::Char('i') | KeyCode::Char('I') => self.modal_info_visible = !self.modal_info_visible,
            KeyCode::Char('t') | KeyCode::Char('T') => self.start_tail_mode()?,
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
            KeyCode::Tab => self.focus = Focus::List,
            KeyCode::Up | KeyCode::Down => self.cycle_filter_mode(),
            KeyCode::Esc => {
                self.query_input.clear();
                self.apply_filter()?;
            }
            KeyCode::Enter => self.apply_filter()?,
            KeyCode::Backspace => {
                self.query_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => self.query_input.clear(),
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => self.query_input.push(ch),
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
            KeyCode::Enter => self.save_current_results()?,
            KeyCode::Backspace => delete_char_before(&mut self.save_filename, &mut self.save_filename_cursor),
            KeyCode::Delete => delete_char_at(&mut self.save_filename, self.save_filename_cursor),
            KeyCode::Left => self.save_filename_cursor = self.save_filename_cursor.saturating_sub(1),
            KeyCode::Right => self.save_filename_cursor = (self.save_filename_cursor + 1).min(char_count(&self.save_filename)),
            KeyCode::Home => self.save_filename_cursor = 0,
            KeyCode::End => self.save_filename_cursor = char_count(&self.save_filename),
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

    pub(crate) fn handle_export_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.export_message = None;
            }
            KeyCode::Enter => self.export_current_results()?,
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
            KeyCode::Char(' ') if self.export_field == ExportField::Format => self.cycle_export_format(1),
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

    fn handle_export_progress_key(&mut self) -> Result<bool> {
        Ok(false)
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Tab => self.focus = Focus::Search,
            KeyCode::Char('s') | KeyCode::Char('S') => self.open_save_prompt()?,
            KeyCode::Char('e') | KeyCode::Char('E') => self.open_export_prompt()?,
            KeyCode::Char('d') | KeyCode::Char('D') => self.open_dedup_prompt(),
            KeyCode::Char('i') | KeyCode::Char('I') => self.details_visible = !self.details_visible,
            KeyCode::Char('t') | KeyCode::Char('T') => self.start_tail_mode()?,
            KeyCode::Up => self.move_selection(-1)?,
            KeyCode::Down => self.move_selection(1)?,
            KeyCode::PageUp => self.move_selection(-10)?,
            KeyCode::PageDown => self.move_selection(10)?,
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
            KeyCode::Enter => self.open_modal()?,
            KeyCode::Char('f') | KeyCode::Char('F') => self.open_field_filter(),
            KeyCode::Char('/') => self.focus = Focus::Search,
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

    pub(super) fn apply_filter(&mut self) -> Result<()> {
        self.stop_tail_mode();
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
        let dataset = self.dataset.clone();
        let view_order = self.view_order;
        let (tx, rx) = mpsc::channel();
        let tx_worker = tx.clone();

        thread::spawn(move || {
            let result = scan_matches(&dataset, view_order, predicate, spool_writer, cancel_worker, tx_worker.clone());
            match result {
                Ok((scanned, matched)) => {
                    let _ = tx_worker.send(ScanUpdate::Finished { scanned, matched });
                }
                Err(err) => {
                    let _ = tx_worker.send(ScanUpdate::Failed(err.to_string()));
                }
            }
        });

        self.status = self.scan_status(self.applied_query.as_str(), 0, false);
        self.current_scan = Some(ActiveScan { rx, cancel, spool_path, spool_reader, scanned: 0, matched: 0, finished: false });

        Ok(())
    }

    fn cycle_filter_mode(&mut self) {
        self.filter_mode = match self.filter_mode {
            FilterMode::Strings => FilterMode::Regex,
            FilterMode::Regex => FilterMode::Strings,
        };
    }

    pub(crate) fn open_save_prompt(&mut self) -> Result<()> {
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

    pub(crate) fn open_export_prompt(&mut self) -> Result<()> {
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
            self.export_filename = format!("{}.{}", self.default_stem("export"), default_ext);
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

    pub(crate) fn save_current_results(&mut self) -> Result<()> {
        let filename = self.save_filename.trim();
        if filename.is_empty() {
            self.save_message = Some("Filename must not be empty.".to_string());
            return Ok(());
        }
        if filename.contains('/') {
            self.save_message = Some("Filename must not contain path separators.".to_string());
            return Ok(());
        }
        let output_dir = self.output_dir();
        let Some(scan) = &mut self.current_scan else {
            self.save_message = Some("No scan data to save.".to_string());
            return Ok(());
        };
        let output_path = output_dir.join(filename);
        if self.dataset.paths().any(|input| input == output_path) || output_path.exists() {
            self.save_message = Some(format!("File {filename} already exist"));
            self.focus = Focus::SaveError;
            return Ok(());
        }

        let file = OpenOptions::new().write(true).create_new(true).open(&output_path)?;
        let writer = BufWriter::new(file);
        let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
        let mut block_last = None;
        for meta in &self.entries {
            let detail = read_spool_record(&mut scan.spool_reader, meta.clone())?;
            push_preserving_view_order(
                &mut logjet,
                &mut block_last,
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

    pub(crate) fn export_current_results(&mut self) -> Result<()> {
        let filename = self.export_filename.trim();
        if filename.is_empty() {
            self.export_message = Some("Filename must not be empty.".to_string());
            return Ok(());
        }
        if filename.contains('/') {
            self.export_message = Some("Filename must not contain path separators.".to_string());
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

        let output_dir = self.output_dir();
        let output_path = output_dir.join(filename);
        if self.dataset.paths().any(|input| input == output_path) || output_path.exists() {
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
                for meta in &selected_entries {
                    let detail = read_spool_record(&mut scan.spool_reader, meta.clone())?;
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
                self.start_plugin_export(other.to_string(), temp_input, output_path, end.saturating_sub(start));
                return Ok(());
            }
        }

        self.focus = Focus::List;
        self.export_message = None;
        self.status = format!("Exported {exported} {} row(s) to {}", format.label(), output_path.display());
        Ok(())
    }

    fn start_plugin_export(&mut self, format: String, temp_input: PathBuf, output_path: PathBuf, rows: usize) {
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        self.export_progress = 0.0;
        self.export_phase = format!("Exporting {rows} {format} row(s)...");
        self.focus = Focus::ExportProgress;

        thread::spawn(move || {
            let result = (|| -> std::result::Result<(), String> {
                let registry = crate::exporter::ExporterRegistry::discover();
                let plugin = registry.plugin(&format).ok_or_else(|| registry.unknown_format_error(&format).to_string())?;
                let mut last_sent = 0usize;
                let mut finalizing_sent = false;
                plugin
                    .export_with_progress(&temp_input, &output_path, false, &[], |processed| {
                        let processed = processed as usize;
                        if processed == rows || processed.saturating_sub(last_sent) >= 128 {
                            last_sent = processed;
                            let _ = tx.send(ExportUpdate::Progress { processed, total: rows });
                            if processed >= rows && !finalizing_sent {
                                finalizing_sent = true;
                                let _ = tx.send(ExportUpdate::Finalizing);
                            }
                        }
                    })
                    .map_err(|err| err.to_string())?;
                Ok(())
            })();
            let _ = std::fs::remove_file(&temp_input);
            match result {
                Ok(()) => {
                    let _ = tx.send(ExportUpdate::Done { format, rows, output: output_path });
                }
                Err(err) => {
                    let _ = tx.send(ExportUpdate::Failed(err));
                }
            }
        });
    }

    pub(crate) fn drain_export_updates(&mut self) {
        let Some(rx) = &self.export_rx else { return };
        while let Ok(update) = rx.try_recv() {
            match update {
                ExportUpdate::Progress { processed, total } => {
                    let total = total.max(1);
                    let ratio = (processed.min(total) as f64 / total as f64).clamp(0.0, 1.0);
                    self.export_progress = (ratio * 0.92).min(0.92);
                    self.export_phase = format!("Writing records {}/{}", processed.min(total), total);
                }
                ExportUpdate::Finalizing => {
                    self.export_progress = self.export_progress.max(0.94);
                    self.export_phase = "Finalizing output file...".to_string();
                }
                ExportUpdate::Done { format, rows, output } => {
                    self.export_rx = None;
                    self.export_progress = 1.0;
                    self.focus = Focus::List;
                    self.export_message = None;
                    self.export_phase.clear();
                    self.status = format!("Exported {rows} {format} row(s) to {}", output.display());
                    return;
                }
                ExportUpdate::Failed(err) => {
                    self.export_rx = None;
                    self.export_message = Some(err);
                    self.export_progress = 0.0;
                    self.export_phase.clear();
                    self.focus = Focus::ExportError;
                    return;
                }
            }
        }
    }

    pub(crate) fn current_export_format(&self) -> &super::types::ExportFormatChoice {
        &self.export_formats[self.export_format_index.min(self.export_formats.len().saturating_sub(1))]
    }

    pub(crate) fn cycle_export_format(&mut self, delta: isize) {
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

    pub(crate) fn drain_scan_updates(&mut self) -> Result<()> {
        if self.current_scan.is_none() {
            return Ok(());
        }

        let mut updates = Vec::new();
        if let Some(scan) = &self.current_scan {
            while let Ok(update) = scan.rx.try_recv() {
                updates.push(update);
            }
        }
        if let Some(rx) = &self.tail_rx {
            while let Ok(update) = rx.try_recv() {
                updates.push(update);
            }
        }

        let mut finished = self.current_scan.as_ref().map(|scan| scan.finished).unwrap_or(false);
        let mut should_refresh_selection = false;
        let mut status_override = None;
        let mut stop_tail = false;
        {
            let Some(scan) = &mut self.current_scan else {
                return Ok(());
            };
            for update in updates {
                match update {
                    ScanUpdate::Batch(batch) => {
                        self.entries.extend(batch);
                        scan.matched = self.entries.len() as u64;
                        if self.tail_mode && !self.entries.is_empty() {
                            self.selected = self.entries.len() - 1;
                            should_refresh_selection = true;
                        } else if self.selected_detail.is_none() && !self.entries.is_empty() {
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
                        stop_tail = true;
                        status_override = Some(format!("Scan failed: {message}"));
                    }
                }
            }
        }

        if stop_tail {
            self.stop_tail_mode();
        }

        if should_refresh_selection {
            self.refresh_selected_detail()?;
        }
        if let Some(status) = status_override {
            self.status = status;
        }

        if self.tail_on_launch && self.current_scan.as_ref().map(|scan| scan.finished).unwrap_or(false) && !self.tail_mode {
            self.tail_on_launch = false;
            self.start_tail_mode()?;
            return Ok(());
        }

        if self.tail_mode {
            return Ok(());
        }

        if !finished {
            let matched = self.entries.len();
            self.status = self.scan_status(self.applied_query.as_str(), matched, true);
        }

        Ok(())
    }

    fn refresh_selected_detail(&mut self) -> Result<()> {
        if self.entries.is_empty() {
            self.selected_detail = None;
            return Ok(());
        }

        let Some(scan) = &mut self.current_scan else {
            return Err(Error::Usage("no active scan".to_string()));
        };
        let detail = read_spool_record(&mut scan.spool_reader, self.entries[self.selected].clone())?;
        self.selected_detail = Some(detail);
        self.detail_scroll = 0;
        if self.focus == Focus::Modal {
            if let Some(detail) = &self.selected_detail {
                self.modal_text = Some(render_modal_message(detail, self.hex_payload));
            }
            self.modal_scroll = 0;
        }
        Ok(())
    }

    pub(super) fn summary_for(&mut self, index: usize) -> Result<ListRowSummary> {
        if let Some(summary) = self.summary_cache.get(&index) {
            return Ok(summary.clone());
        }

        let Some(scan) = &mut self.current_scan else {
            return Ok(ListRowSummary { message: String::new(), severity: None });
        };
        let detail = read_spool_record(&mut scan.spool_reader, self.entries[index].clone())?;
        let summary = ListRowSummary { message: format_summary(&detail, self.hex_payload), severity: extract_otlp_log_severity(&detail.payload) };
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

    fn start_tail_mode(&mut self) -> Result<()> {
        if self.dataset.len() != 1 {
            self.status = "Tail mode needs exactly one input file.".to_string();
            return Ok(());
        }
        if self.view_order != ViewOrder::Concat {
            self.status = "Tail mode only works with concat ordering.".to_string();
            return Ok(());
        }
        if self.dataset.is_stdin() {
            self.status = "Tail mode needs a real file, not stdin.".to_string();
            return Ok(());
        }

        let Some(scan) = &self.current_scan else {
            self.status = "No active scan to tail.".to_string();
            return Ok(());
        };
        if !scan.finished {
            self.status = "Wait for the scan to finish before tailing.".to_string();
            return Ok(());
        }
        if self.tail_mode {
            return Ok(());
        }

        let mut predicate = parse_filter_query(&self.applied_query, self.filter_mode)?;
        predicate.field_filter = self.active_field_filter.clone();
        let spool_path = scan.spool_path.clone();
        let input = self.input.clone();
        let writer = OpenOptions::new().read(true).write(true).open(&spool_path)?;
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = follow_appended_matches(input.as_path(), predicate, writer, cancel_worker, tx.clone());
            if let Err(err) = result {
                let _ = tx.send(ScanUpdate::Failed(err.to_string()));
            }
        });

        self.tail_mode = true;
        self.tail_marker_index = self.entries.len().checked_sub(1);
        self.tail_rx = Some(rx);
        self.tail_cancel = Some(cancel);
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
            self.refresh_selected_detail()?;
        }
        Ok(())
    }

    fn stop_tail_mode(&mut self) {
        if let Some(cancel) = self.tail_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.tail_rx = None;
        self.tail_mode = false;
        self.tail_marker_index = None;
    }

    fn open_field_filter(&mut self) {
        let catalog = self.field_catalog.lock().unwrap();
        let Some(cat) = catalog.as_ref() else {
            self.status = "Field catalog still scanning… try again in a moment".to_string();
            return;
        };
        self.field_filter_state = Some(super::types::FieldFilterState {
            panel: 0,
            severity_cursor: 0,
            service_cursor: 0,
            severity_scroll: 0,
            service_scroll: 0,
            filter_text: String::new(),
            selected_severities: self.active_field_filter.severities.clone().unwrap_or_default(),
            selected_services: self.active_field_filter.services.clone().unwrap_or_default(),
        });
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

        let filter_lower = state.filter_text.to_lowercase();
        let filtered_sev: Vec<&String> = sev_list.iter().filter(|s| filter_lower.is_empty() || s.to_lowercase().contains(&filter_lower)).collect();
        let filtered_svc: Vec<&String> = svc_list.iter().filter(|s| filter_lower.is_empty() || s.to_lowercase().contains(&filter_lower)).collect();

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

        if let Some(state) = &mut self.field_filter_state {
            let active_count = if state.panel == 0 {
                sev_list.iter().filter(|s| state.filter_text.is_empty() || s.to_lowercase().contains(&state.filter_text.to_lowercase())).count()
            } else {
                svc_list.iter().filter(|s| state.filter_text.is_empty() || s.to_lowercase().contains(&state.filter_text.to_lowercase())).count()
            };
            if state.panel == 0 {
                state.severity_cursor = active_count.checked_sub(1).map(|max| state.severity_cursor.min(max)).unwrap_or(0);
                let row = state.severity_cursor as u16 + 1;
                if row < state.severity_scroll {
                    state.severity_scroll = row;
                } else if row >= state.severity_scroll + visible_rows as u16 {
                    state.severity_scroll = row - visible_rows as u16 + 1;
                }
            } else {
                state.service_cursor = active_count.checked_sub(1).map(|max| state.service_cursor.min(max)).unwrap_or(0);
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
            let _ = self.apply_filter();
        }
    }

    fn cancel_scan(&mut self) {
        if let Some(scan) = &self.current_scan {
            scan.cancel();
        }
    }

    pub(crate) fn open_dedup_prompt(&mut self) {
        if self.current_scan.is_none() && (self.dataset.len() != 1 || self.dataset.is_stdin()) {
            self.status = "Dedup needs a finished scan or one real input file.".to_string();
            return;
        }
        self.dedup_filename = format!("{}-dedup.logjet", self.default_stem("output"));
        self.dedup_behavior = DedupMode::Distinct;
        self.dedup_match_mode = DedupMatchMode::Hash2;
        self.focus = Focus::DedupPrompt;
    }

    pub(crate) fn handle_dedup_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Enter => {
                let filename = self.dedup_filename.clone();
                if !filename.is_empty() {
                    self.start_dedup(&filename, self.dedup_behavior, self.dedup_match_mode);
                }
            }
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Left => self.dedup_behavior = self.dedup_behavior.prev(),
            KeyCode::Right => self.dedup_behavior = self.dedup_behavior.next(),
            KeyCode::Up => self.dedup_match_mode = self.dedup_match_mode.prev(),
            KeyCode::Down | KeyCode::Tab => self.dedup_match_mode = self.dedup_match_mode.next(),
            KeyCode::Backspace => {
                self.dedup_filename.pop();
            }
            KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => self.dedup_filename.push(c),
            _ => {}
        }
        Ok(false)
    }

    pub(crate) fn start_dedup(&mut self, filename: &str, behavior: DedupMode, match_mode: DedupMatchMode) {
        let temp_input = if self.can_dedup() {
            let Some(scan) = &mut self.current_scan else {
                self.status = "No scan data to dedup.".to_string();
                self.focus = Focus::List;
                return;
            };
            match write_export_selection_to_temp_logjet(scan, &self.entries) {
                Ok(path) => Some(path),
                Err(err) => {
                    self.status = format!("Dedup preparation failed: {err}");
                    self.focus = Focus::List;
                    return;
                }
            }
        } else if self.dataset.len() == 1 && !self.dataset.is_stdin() {
            None
        } else {
            self.status = "Dedup needs a finished scan or one real input file.".to_string();
            self.focus = Focus::List;
            return;
        };
        let source_input = self.input.clone();
        let output_dir = self.output_dir();
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
                let input = InputHandle::open(temp_input.as_ref().unwrap_or(&source_input)).map_err(|e| e.to_string())?;
                tx.send(DedupUpdate::Progress { ratio: 0.18, phase: "unpacking records".to_string() }).ok();
                let mut reader = LogjetReader::new(input.into_buf_reader());
                let unpacked = crate::dedup::unpack::unpack(&mut reader).map_err(|e| e.to_string())?;
                tx.send(DedupUpdate::Progress { ratio: 0.32, phase: "preparing output".to_string() }).ok();

                let out_file = File::create(&output_path).map_err(|e| e.to_string())?;
                let mut writer = LogjetWriter::new(BufWriter::new(out_file));
                tx.send(DedupUpdate::Progress { ratio: 0.82, phase: format!("running {} / {}", behavior.label(), match_mode.label()) }).ok();

                let opts = crate::dedup::DedupOpts { mode: behavior, match_mode, ..crate::dedup::DedupOpts::default() };
                let stats = crate::dedup::dedup(unpacked.records, unpacked.passthrough, &mut writer, &opts).map_err(|e| e.to_string())?;

                tx.send(DedupUpdate::Progress { ratio: 0.94, phase: "flushing output".to_string() }).ok();
                let mut out = writer.into_inner().map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
                Ok(stats)
            };
            match run() {
                Ok(stats) => {
                    if let Some(path) = &temp_input {
                        let _ = std::fs::remove_file(path);
                    }
                    let _ = tx.send(DedupUpdate::Done { total: stats.total_records, groups: stats.group_count, pct: stats.reduction_pct() });
                }
                Err(e) => {
                    if let Some(path) = &temp_input {
                        let _ = std::fs::remove_file(path);
                    }
                    let _ = tx.send(DedupUpdate::Failed(e));
                }
            }
        });
    }

    pub(crate) fn drain_dedup_updates(&mut self) {
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

    pub(crate) fn handle_dedup_progress_key(&mut self, key: KeyEvent) -> Result<bool> {
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
            }
            KeyCode::Esc => {
                self.dedup_completion_message = None;
                self.dedup_output_path = None;
                self.focus = Focus::List;
            }
            _ => {}
        }
        Ok(false)
    }

    fn switch_to_file(&mut self, path: PathBuf) -> Result<()> {
        self.cancel_scan();
        if let Some(scan) = self.current_scan.take() {
            drop(scan.spool_reader);
            let _ = std::fs::remove_file(scan.spool_path);
        }
        self.input = path;
        self.dataset = Dataset::from_inputs(std::slice::from_ref(&self.input))?;

        let catalog_bg = Arc::clone(&self.field_catalog);
        let dataset_bg = self.dataset.clone();
        *self.field_catalog.lock().unwrap() = None;
        thread::spawn(move || {
            if let Ok(cat) = scan_field_catalog(&dataset_bg) {
                *catalog_bg.lock().unwrap() = Some(cat);
            }
        });

        self.query_input.clear();
        self.apply_filter()
    }

    fn default_stem(&self, fallback: &str) -> String {
        self.dataset.default_stem(fallback)
    }

    fn output_dir(&self) -> PathBuf {
        self.dataset.output_dir()
    }

    fn scan_status(&self, query: &str, matched: usize, buffered: bool) -> String {
        let target = if self.dataset.len() == 1 {
            if query.is_empty() { "all records".to_string() } else { format!("{query:?}") }
        } else if query.is_empty() {
            format!("all records across {} files", self.dataset.len())
        } else {
            format!("{query:?} across {} files", self.dataset.len())
        };
        if buffered {
            format!("Scanning {target} [{}]: {matched} matches buffered", self.view_order.label())
        } else {
            format!("Scanning {target} [{}]", self.view_order.label())
        }
    }

    pub(crate) fn current_record_filename(&self) -> Option<String> {
        let meta = self.entries.get(self.selected)?;
        meta.source_path.file_name().and_then(|s| s.to_str()).map(|s| s.to_string())
    }

    pub(crate) fn can_tail(&self) -> bool {
        self.dataset.len() == 1 && !self.dataset.is_stdin() && self.view_order == ViewOrder::Concat
    }

    pub(crate) fn can_dedup(&self) -> bool {
        self.current_scan.as_ref().map(|scan| scan.finished).unwrap_or(false) && !self.entries.is_empty()
    }
}
