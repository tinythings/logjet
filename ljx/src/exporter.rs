//! Dynamic exporter discovery and plugin execution.
//!
//! Built-in export formats stay inside `commands::export`, while this module
//! discovers shared-library exporters, validates their ABI, resolves conflicts,
//! and runs them through the stable C ABI defined in `liblogjet::export`.

use std::collections::HashMap;
use std::env;
use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};

use libloading::Library;
use liblogjet::export::{
    LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST, LJX_EXPORT_CAP_RECORD_LOGS, LJX_EXPORT_CAP_RECORD_METRICS, LJX_EXPORT_CAP_RECORD_TRACES,
    LJX_EXPORT_STATUS_IO, LJX_EXPORT_STATUS_OK, LJX_EXPORTER_ABI_MAJOR, LJX_EXPORTER_ABI_MINOR, LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL, LjxAbiBytes,
    LjxAbiString, LjxExportHostV1, LjxExportInitV1, LjxExportRecordV1, LjxExporterCtx, LjxExporterDescriptorV1,
};
use logjet::{LogjetReader, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;

use crate::error::{Error, Result};
use crate::input::{InputHandle, open_output};

const BUILTIN_EXPORTERS: &[&str] = &["ndjson"];

/// Registry of exporter plugins discovered for the current `ljx` process.
pub(crate) struct ExporterRegistry {
    plugins: HashMap<String, LoadedExporter>,
    diagnostics: Vec<String>,
    search_roots: Vec<PathBuf>,
}

impl ExporterRegistry {
    /// Discovers exporter plugins from configured search roots.
    pub(crate) fn discover() -> Self {
        let (search_roots, mut diagnostics) = collect_search_roots();
        let mut plugins = HashMap::new();
        let mut winners = HashMap::<String, PathBuf>::new();

        for candidate in collect_plugin_candidates(&search_roots, &mut diagnostics) {
            match LoadedExporter::load(&candidate) {
                Ok(plugin) => {
                    let format = plugin.format_name.clone();
                    if BUILTIN_EXPORTERS.iter().any(|built_in| *built_in == format) {
                        diagnostics.push(format!(
                            "ignoring exporter plugin {}: format `{}` conflicts with built-in exporter",
                            candidate.display(),
                            format
                        ));
                        continue;
                    }
                    if let Some(existing) = winners.get(&format) {
                        diagnostics.push(format!(
                            "ignoring exporter plugin {}: format `{}` already provided by {}",
                            candidate.display(),
                            format,
                            existing.display()
                        ));
                        continue;
                    }
                    winners.insert(format.clone(), candidate.clone());
                    plugins.insert(format, plugin);
                }
                Err(err) => diagnostics.push(err),
            }
        }

        Self { plugins, diagnostics, search_roots }
    }

    /// Returns the discovered plugin for one normalized format name.
    pub(crate) fn plugin(&self, format: &str) -> Option<&LoadedExporter> {
        self.plugins.get(format)
    }

    /// Returns a sorted list of all known export formats, built-in first.
    pub(crate) fn available_formats(&self) -> Vec<String> {
        let mut out = BUILTIN_EXPORTERS.iter().map(|name| (*name).to_string()).chain(self.plugins.keys().cloned()).collect::<Vec<_>>();
        out.sort();
        out.dedup();
        out
    }

    /// Builds a user-facing error for an unknown export format.
    pub(crate) fn unknown_format_error(&self, requested: &str) -> Error {
        let mut message = format!("unknown export format `{requested}`");
        let formats = self.available_formats();
        if formats.is_empty() {
            message.push_str("; no exporters are available");
        } else {
            message.push_str(&format!("; available formats: {}", formats.join(", ")));
        }
        if !self.search_roots.is_empty() {
            message.push_str(&format!("; searched: {}", self.search_roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" -> ")));
        }
        if !self.diagnostics.is_empty() {
            message.push_str(&format!("; loader notes: {}", self.diagnostics.join(" | ")));
        }
        Error::Usage(message)
    }
}

/// One loaded exporter plugin and its resolved ABI entry points.
pub(crate) struct LoadedExporter {
    _lib: Library,
    path: PathBuf,
    format_name: String,
    display_name: String,
    capabilities: u64,
    create: unsafe extern "C" fn(host: *const LjxExportHostV1, init: *const LjxExportInitV1) -> *mut LjxExporterCtx,
    write_record: unsafe extern "C" fn(ctx: *mut LjxExporterCtx, record: *const LjxExportRecordV1) -> i32,
    finish: unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> i32,
    last_error: unsafe extern "C" fn(ctx: *mut LjxExporterCtx) -> LjxAbiString,
    free: unsafe extern "C" fn(ctx: *mut LjxExporterCtx),
}

impl LoadedExporter {
    fn load(path: &Path) -> std::result::Result<Self, String> {
        // SAFETY: operator-controlled plugin paths are loaded explicitly.
        let lib = unsafe { Library::new(path) }.map_err(|err| format!("dlopen {}: {err}", path.display()))?;

        // SAFETY: symbol signature is defined by the stable exporter ABI.
        let descriptor_fn = unsafe {
            lib.get::<unsafe extern "C" fn() -> *const LjxExporterDescriptorV1>(LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL).map_err(|err| {
                format!("symbol {} in {}: {err}", String::from_utf8_lossy(LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL).trim_end_matches('\0'), path.display())
            })?
        };

        // SAFETY: plugin exported the descriptor function above.
        let descriptor_ptr = unsafe { descriptor_fn() };
        if descriptor_ptr.is_null() {
            return Err(format!("exporter {} returned NULL descriptor", path.display()));
        }

        // SAFETY: descriptor pointer was checked for null and points to plugin-owned static storage.
        let descriptor = unsafe { &*descriptor_ptr };
        if descriptor.struct_size < std::mem::size_of::<LjxExporterDescriptorV1>() as u32 {
            return Err(format!(
                "exporter {} advertises descriptor size {} but host needs {}",
                path.display(),
                descriptor.struct_size,
                std::mem::size_of::<LjxExporterDescriptorV1>()
            ));
        }
        if descriptor.abi_major != LJX_EXPORTER_ABI_MAJOR {
            return Err(format!(
                "exporter {} uses ABI {}.{} but host needs {}.x",
                path.display(),
                descriptor.abi_major,
                descriptor.abi_minor,
                LJX_EXPORTER_ABI_MAJOR
            ));
        }
        if descriptor.abi_minor > LJX_EXPORTER_ABI_MINOR {
            return Err(format!(
                "exporter {} uses newer ABI minor {}.{} than host supports {}.{}",
                path.display(),
                descriptor.abi_major,
                descriptor.abi_minor,
                LJX_EXPORTER_ABI_MAJOR,
                LJX_EXPORTER_ABI_MINOR
            ));
        }

        let format_name = abi_string_to_string(descriptor.format_name).trim().to_ascii_lowercase();
        if !is_valid_format_name(&format_name) {
            return Err(format!("exporter {} returned invalid format name `{}`", path.display(), format_name));
        }
        let display_name = non_empty_or(abi_string_to_string(descriptor.display_name), &format_name);
        Ok(Self {
            _lib: lib,
            path: path.to_path_buf(),
            format_name,
            display_name,
            capabilities: descriptor.capabilities,
            create: descriptor.create,
            write_record: descriptor.write_record,
            finish: descriptor.finish,
            last_error: descriptor.last_error,
            free: descriptor.free,
        })
    }

    /// Runs one export through this plugin.
    pub(crate) fn export(&self, input: &Path, output: &Path) -> Result<()> {
        let input = InputHandle::open(input)?;
        let mut reader = LogjetReader::new(input.into_buf_reader());
        let writer = open_output(output)?;
        let mut sink = HostOutputSink { writer, last_error: None };
        let host = LjxExportHostV1 {
            struct_size: std::mem::size_of::<LjxExportHostV1>() as u32,
            flags: 0,
            user: (&mut sink as *mut HostOutputSink).cast::<c_void>(),
            write: host_write,
            flush: Some(host_flush),
            reserved: [0; 6],
        };
        let init = LjxExportInitV1::default();

        // SAFETY: host/init pointers stay valid for the lifetime of the plugin context.
        let ctx = unsafe { (self.create)(&host, &init) };
        if ctx.is_null() {
            return Err(Error::Usage(format!("exporter `{}` from {} failed to start: create returned NULL", self.display_name, self.path.display())));
        }
        let guard = PluginCtxGuard { exporter: self, ctx };

        while let Some(record) = reader.next_record()? {
            let payload_kind = payload_kind(&record);
            self.validate_capabilities(&record, payload_kind)?;
            let raw = LjxExportRecordV1 {
                struct_size: std::mem::size_of::<LjxExportRecordV1>() as u32,
                record_type: abi_record_type(record.record_type),
                payload_kind,
                flags: 0,
                seq: record.seq,
                timestamp_unix_ns: record.ts_unix_ns,
                payload: LjxAbiBytes { ptr: record.payload.as_ptr(), len: record.payload.len() },
            };
            // SAFETY: ctx belongs to this plugin; raw borrows `record` for the duration of the call.
            let status = unsafe { (self.write_record)(guard.ctx, &raw) };
            if status != LJX_EXPORT_STATUS_OK {
                return Err(self.status_error(guard.ctx, status, Some(&record), "write record"));
            }
        }

        // SAFETY: ctx belongs to this plugin and remains valid until guard drop.
        let finish_status = unsafe { (self.finish)(guard.ctx) };
        if finish_status != LJX_EXPORT_STATUS_OK {
            return Err(self.status_error(guard.ctx, finish_status, None, "finish export"));
        }
        if let Some(err) = sink.last_error.take() {
            return Err(Error::Usage(format!("exporter `{}` write error: {err}", self.display_name)));
        }
        sink.writer.flush()?;
        Ok(())
    }

    fn validate_capabilities(&self, record: &OwnedRecord, payload_kind: u32) -> Result<()> {
        let record_flag = match record.record_type {
            RecordType::Logs => LJX_EXPORT_CAP_RECORD_LOGS,
            RecordType::Metrics => LJX_EXPORT_CAP_RECORD_METRICS,
            RecordType::Traces => LJX_EXPORT_CAP_RECORD_TRACES,
        };
        if self.capabilities & record_flag == 0 {
            return Err(Error::Usage(format!(
                "exporter `{}` from {} does not accept {} records",
                self.display_name,
                self.path.display(),
                record_kind_label(record.record_type)
            )));
        }
        if payload_kind == liblogjet::export::LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST
            && self.capabilities & LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST == 0
        {
            return Err(Error::Usage(format!("exporter `{}` from {} does not accept OTLP logs payloads", self.display_name, self.path.display())));
        }
        Ok(())
    }

    fn status_error(&self, ctx: *mut LjxExporterCtx, status: i32, record: Option<&OwnedRecord>, action: &str) -> Error {
        let tail = record.map(|r| format!(" at seq {} ({})", r.seq, record_kind_label(r.record_type))).unwrap_or_default();
        let detail = self.last_error_message(ctx);
        if detail.is_empty() {
            Error::Usage(format!("exporter `{}` from {} failed to {action}{tail} (status {status})", self.display_name, self.path.display()))
        } else {
            Error::Usage(format!(
                "exporter `{}` from {} failed to {action}{tail}: {} (status {status})",
                self.display_name,
                self.path.display(),
                detail
            ))
        }
    }

    fn last_error_message(&self, ctx: *mut LjxExporterCtx) -> String {
        // SAFETY: last_error belongs to this plugin context and returns a borrowed string view.
        abi_string_to_string(unsafe { (self.last_error)(ctx) })
    }
}

struct PluginCtxGuard<'a> {
    exporter: &'a LoadedExporter,
    ctx: *mut LjxExporterCtx,
}

impl Drop for PluginCtxGuard<'_> {
    fn drop(&mut self) {
        // SAFETY: ctx belongs to exporter and may be freed exactly once here.
        unsafe { (self.exporter.free)(self.ctx) };
    }
}

struct HostOutputSink {
    writer: Box<dyn Write>,
    last_error: Option<String>,
}

unsafe extern "C" fn host_write(user: *mut c_void, data: *const u8, len: usize) -> i32 {
    if user.is_null() || (data.is_null() && len != 0) {
        return liblogjet::export::LJX_EXPORT_STATUS_BAD_ARG;
    }
    // SAFETY: user is created from a mutable HostOutputSink in `export`.
    let sink = unsafe { &mut *(user.cast::<HostOutputSink>()) };
    let bytes = if len == 0 {
        &[][..]
    } else {
        // SAFETY: plugin promises the data buffer is valid for this callback.
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    match sink.writer.write_all(bytes) {
        Ok(()) => {
            sink.last_error = None;
            LJX_EXPORT_STATUS_OK
        }
        Err(err) => {
            sink.last_error = Some(err.to_string());
            LJX_EXPORT_STATUS_IO
        }
    }
}

unsafe extern "C" fn host_flush(user: *mut c_void) -> i32 {
    if user.is_null() {
        return liblogjet::export::LJX_EXPORT_STATUS_BAD_ARG;
    }
    // SAFETY: user is created from a mutable HostOutputSink in `export`.
    let sink = unsafe { &mut *(user.cast::<HostOutputSink>()) };
    match sink.writer.flush() {
        Ok(()) => {
            sink.last_error = None;
            LJX_EXPORT_STATUS_OK
        }
        Err(err) => {
            sink.last_error = Some(err.to_string());
            LJX_EXPORT_STATUS_IO
        }
    }
}

fn collect_search_roots() -> (Vec<PathBuf>, Vec<String>) {
    let mut roots = Vec::new();
    let mut diagnostics = Vec::new();

    if let Some(raw) = env::var_os("LJX_EXPORTER_PATH") {
        for entry in env::split_paths(&raw) {
            if entry.as_os_str().is_empty() {
                continue;
            }
            if !entry.exists() {
                diagnostics.push(format!("configured exporter search path {} does not exist", entry.display()));
            }
            push_unique_path(&mut roots, entry);
        }
    }

    if let Ok(cwd) = env::current_dir() {
        push_unique_path(&mut roots, cwd.join("exporters"));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        push_unique_path(&mut roots, dir.join("exporters"));
        push_unique_path(&mut roots, dir.join("../lib/logjet/exporters"));
    }

    (roots, diagnostics)
}

fn collect_plugin_candidates(roots: &[PathBuf], diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        if root.is_file() {
            out.push(root.clone());
            continue;
        }
        if !root.exists() {
            continue;
        }
        let read_dir = match std::fs::read_dir(root) {
            Ok(read_dir) => read_dir,
            Err(err) => {
                diagnostics.push(format!("cannot read exporter directory {}: {err}", root.display()));
                continue;
            }
        };
        let mut entries = read_dir.filter_map(|entry| entry.ok().map(|ok| ok.path())).filter(|path| is_shared_library(path)).collect::<Vec<_>>();
        entries.sort();
        out.extend(entries);
    }
    out
}

fn push_unique_path(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.iter().any(|path| path == &candidate) {
        paths.push(candidate);
    }
}

fn is_shared_library(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case(std::env::consts::DLL_EXTENSION))
}

fn abi_string_to_string(value: LjxAbiString) -> String {
    if value.ptr.is_null() || value.len == 0 {
        return String::new();
    }
    // SAFETY: ABI strings are borrowed `(ptr, len)` views provided by the plugin.
    let bytes = unsafe { std::slice::from_raw_parts(value.ptr.cast::<u8>(), value.len) };
    String::from_utf8_lossy(bytes).into_owned()
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() { fallback.to_string() } else { value }
}

fn is_valid_format_name(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
}

fn abi_record_type(value: RecordType) -> u32 {
    match value {
        RecordType::Logs => liblogjet::export::LJX_RECORD_TYPE_LOGS,
        RecordType::Metrics => liblogjet::export::LJX_RECORD_TYPE_METRICS,
        RecordType::Traces => liblogjet::export::LJX_RECORD_TYPE_TRACES,
    }
}

fn payload_kind(record: &OwnedRecord) -> u32 {
    if record.record_type == RecordType::Logs && ExportLogsServiceRequest::decode(record.payload.as_slice()).is_ok() {
        liblogjet::export::LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST
    } else {
        liblogjet::export::LJX_PAYLOAD_KIND_OPAQUE
    }
}

fn record_kind_label(record_type: RecordType) -> &'static str {
    match record_type {
        RecordType::Logs => "logs",
        RecordType::Metrics => "metrics",
        RecordType::Traces => "traces",
    }
}
