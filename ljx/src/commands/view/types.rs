use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use logjet::RecordType;

use crate::dedup::{DedupMatchMode, DedupMode};
use crate::exporter::ExporterRegistry;
use crate::predicate::{FieldFilter, FilterMode};

pub(super) const SUMMARY_CACHE_LIMIT: usize = 256;
pub(super) const DETAIL_PREVIEW_BYTES: usize = 1024;
pub(super) const SCAN_BATCH_SIZE: usize = 128;
pub(crate) const MODAL_ATTR_ENTRY_LIMIT_PER_KIND: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
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
pub(crate) enum ExportField {
    Format,
    Filename,
    Range,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EntryMeta {
    pub(crate) offset: u64,
    pub(crate) record_type: RecordType,
    pub(crate) seq: u64,
    pub(crate) ts_unix_ns: u64,
    pub(crate) payload_len: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct DetailRecord {
    pub(crate) meta: EntryMeta,
    pub(crate) payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) enum ScanUpdate {
    Batch(Vec<EntryMeta>),
    Finished { scanned: u64, matched: u64 },
    Failed(String),
}

#[derive(Debug, Clone)]
pub(crate) enum DedupUpdate {
    Progress { ratio: f64, phase: String },
    Done { total: u64, groups: u64, pct: f64 },
    Failed(String),
}

impl DedupMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Distinct => "distinct",
            Self::Collapse => "collapse",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Distinct => "whole filtered set, SQL-like distinct",
            Self::Collapse => "nearby burst suppression within bucket",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            Self::Distinct => Self::Collapse,
            Self::Collapse => Self::Distinct,
        }
    }

    pub(super) fn prev(self) -> Self {
        self.next()
    }
}

impl DedupMatchMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Hash2 => "canon",
            Self::Full => "full",
        }
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Exact => "byte-identical bodies only",
            Self::Hash2 => "canonicalized body grouping",
            Self::Full => "canon plus Drain3 residuals",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            Self::Exact => Self::Hash2,
            Self::Hash2 => Self::Full,
            Self::Full => Self::Exact,
        }
    }

    pub(super) fn prev(self) -> Self {
        match self {
            Self::Exact => Self::Full,
            Self::Hash2 => Self::Exact,
            Self::Full => Self::Hash2,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExportFormatChoice {
    name: String,
    title: String,
    default_extension: String,
}

impl ExportFormatChoice {
    pub(crate) fn ndjson() -> Self {
        Self { name: "ndjson".to_string(), title: "NDJSON".to_string(), default_extension: "ndjson".to_string() }
    }

    pub(crate) fn from_plugin_name(name: String) -> Self {
        Self { title: name.to_ascii_uppercase(), default_extension: name.clone(), name }
    }

    pub(super) fn label(&self) -> &str {
        self.name.as_str()
    }

    pub(super) fn title(&self) -> &str {
        self.title.as_str()
    }

    pub(super) fn default_extension(&self) -> &str {
        self.default_extension.as_str()
    }
}

pub(super) fn discover_export_format_choices(exporters: &ExporterRegistry) -> Vec<ExportFormatChoice> {
    let mut out = vec![ExportFormatChoice::ndjson()];
    let mut plugins = exporters.available_formats().into_iter().filter(|name| name != "ndjson").collect::<Vec<_>>();
    plugins.sort();
    plugins.dedup();
    out.extend(plugins.into_iter().map(ExportFormatChoice::from_plugin_name));
    out
}

pub(crate) struct ActiveScan {
    pub(super) rx: Receiver<ScanUpdate>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) spool_path: PathBuf,
    pub(super) spool_reader: File,
    pub(super) scanned: u64,
    pub(super) matched: u64,
    pub(crate) finished: bool,
}

impl ActiveScan {
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Distinct field values collected by the background catalog scan.
pub(super) struct FieldCatalog {
    pub(super) severities: Vec<String>,
    pub(super) services: Vec<String>,
}

/// UI state for the field-filter popup.
pub(super) struct FieldFilterState {
    /// 0 = severity panel, 1 = services panel
    pub(super) panel: usize,
    pub(super) severity_cursor: usize,
    pub(super) service_cursor: usize,
    pub(super) severity_scroll: u16,
    pub(super) service_scroll: u16,
    pub(super) filter_text: String,
    pub(super) selected_severities: HashSet<String>,
    pub(super) selected_services: HashSet<String>,
}

pub(crate) struct ViewApp {
    pub(crate) input: PathBuf,
    pub(super) hex_payload: bool,
    pub(super) exporters: ExporterRegistry,
    pub(crate) export_formats: Vec<ExportFormatChoice>,
    pub(crate) focus: Focus,
    pub(super) filter_mode: FilterMode,
    pub(super) query_input: String,
    pub(super) applied_query: String,
    pub(crate) status: String,
    pub(super) entries: Vec<EntryMeta>,
    pub(crate) selected: usize,
    pub(super) list_offset: usize,
    pub(super) modal_scroll: u16,
    pub(super) modal_info_visible: bool,
    pub(super) details_visible: bool,
    pub(super) detail_scroll: u16,
    pub(super) summary_cache: HashMap<usize, String>,
    pub(super) summary_order: VecDeque<usize>,
    pub(super) selected_detail: Option<DetailRecord>,
    pub(super) modal_text: Option<String>,
    pub(super) save_filename: String,
    pub(super) save_filename_cursor: usize,
    pub(super) save_message: Option<String>,
    pub(crate) export_format_index: usize,
    pub(crate) export_filename: String,
    pub(crate) export_filename_cursor: usize,
    pub(crate) export_range: String,
    pub(crate) export_range_cursor: usize,
    pub(crate) export_field: ExportField,
    pub(super) export_message: Option<String>,
    pub(crate) current_scan: Option<ActiveScan>,
    pub(super) tail_on_launch: bool,
    pub(super) tail_mode: bool,
    pub(super) tail_marker_index: Option<usize>,
    pub(super) tail_rx: Option<Receiver<ScanUpdate>>,
    pub(super) tail_cancel: Option<Arc<AtomicBool>>,
    pub(super) field_catalog: Arc<std::sync::Mutex<Option<FieldCatalog>>>,
    pub(super) field_filter_state: Option<FieldFilterState>,
    pub(super) active_field_filter: FieldFilter,
    pub(super) dedup_filename: String,
    pub(crate) dedup_behavior: DedupMode,
    pub(crate) dedup_match_mode: DedupMatchMode,
    pub(crate) dedup_output_path: Option<PathBuf>,
    pub(crate) dedup_rx: Option<Receiver<DedupUpdate>>,
    pub(crate) dedup_progress: f64,
    pub(crate) dedup_progress_target: f64,
    pub(crate) dedup_phase: String,
    pub(crate) dedup_completion_message: Option<String>,
}
