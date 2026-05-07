//! Ingest plugin loader.
//!
//! Opens a shared library that implements the `lj_ingest_*` C ABI defined in
//! `liblogjet.h`, accepts raw TCP connections, feeds bytes into the plugin
//! parser, and appends parsed records to the spool.

use std::env;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::io::{self, BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::WireRecord;
use liblogjet::export::{LJX_EXPORTER_ABI_MAJOR, LJX_EXPORTER_ABI_MINOR, LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL, LjxAbiString, LjxExporterDescriptorV1};

const LJ_INGEST_ABI_MAJOR: u32 = 1;
const LJ_INGEST_ABI_MINOR: u32 = 1;
const LJ_INGEST_DESCRIPTOR_SYMBOL: &[u8] = b"lj_ingest_descriptor_v1\0";

#[repr(C)]
struct LjIngestDescriptorV1 {
    struct_size: u32,
    abi_major: u32,
    abi_minor: u32,
    name: *const c_char,
    display_name: *const c_char,
    mode: u32,
    reserved: [u64; 8],
}

pub fn resolve_ingest_plugin_path(path: &Path) -> PathBuf {
    if path.exists() || path.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
        return path.to_path_buf();
    }

    resolve_ingest_plugin_path_from_roots(path, &collect_ingest_plugin_search_roots())
}

pub fn resolve_ingest_plugin(plugin_path: Option<&Path>, plugin_dir: Option<&Path>, plugin_name: Option<&str>) -> io::Result<PathBuf> {
    if let Some(path) = plugin_path
        && plugin_name.is_none()
    {
        return Ok(resolve_ingest_plugin_path(path));
    }

    let mut roots = Vec::new();
    if let Some(dir) = plugin_dir {
        push_unique_path(&mut roots, dir.to_path_buf());
    }
    if let Some(path) = plugin_path {
        if path.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
            push_unique_path(&mut roots, path.to_path_buf());
        } else {
            push_unique_path(&mut roots, resolve_ingest_plugin_path(path));
        }
    }
    for root in collect_ingest_plugin_search_roots() {
        push_unique_path(&mut roots, root);
    }

    let name = plugin_name.ok_or_else(|| io::Error::other("ingest.plugin-path or ingest.plugin is required for plugin protocol"))?;
    find_ingest_plugin_by_name(name, &roots)
}

pub fn ingest_plugin_label(path: &Path) -> String {
    match read_ingest_descriptor(path) {
        Ok(descriptor) if descriptor.display_name == descriptor.name => descriptor.name,
        Ok(descriptor) => format!("{} ({})", descriptor.name, descriptor.display_name),
        Err(_) => path.file_stem().and_then(|stem| stem.to_str()).map(normalise_legacy_plugin_stem).unwrap_or_else(|| "unknown".to_string()),
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct VisiblePlugin {
    pub name: String,
    pub display_name: String,
    pub path: PathBuf,
}

pub fn print_visible_plugins(ingest_plugin_dir: Option<&Path>, ingest_plugin_path: Option<&Path>) -> io::Result<()> {
    print_plugin_section("ingestors", &list_visible_ingest_plugins(ingest_plugin_dir, ingest_plugin_path));
    println!();
    print_plugin_section("exporters", &list_visible_exporter_plugins());
    Ok(())
}

fn print_plugin_section(label: &str, plugins: &[VisiblePlugin]) {
    println!("{label}:");
    for plugin in plugins {
        println!("\t{}\t{}", shell_field(&plugin.name), shell_field(&plugin.display_name));
        println!("\t\t{}", plugin.path.display());
    }
}

fn shell_field(value: &str) -> String {
    value.chars().map(|ch| if matches!(ch, '\t' | '\r' | '\n') { ' ' } else { ch }).collect()
}

pub fn list_visible_ingest_plugins(ingest_plugin_dir: Option<&Path>, ingest_plugin_path: Option<&Path>) -> Vec<VisiblePlugin> {
    let mut roots = Vec::new();
    if let Some(dir) = ingest_plugin_dir {
        push_unique_path(&mut roots, dir.to_path_buf());
    }
    if let Some(path) = ingest_plugin_path {
        push_unique_path(&mut roots, path.to_path_buf());
    }
    for root in collect_ingest_plugin_search_roots() {
        push_unique_path(&mut roots, root);
    }

    let mut diagnostics = Vec::new();
    let mut plugins = collect_ingest_plugin_candidates(&roots, &mut diagnostics)
        .into_iter()
        .filter_map(|path| match read_ingest_descriptor(&path) {
            Ok(descriptor) => Some(VisiblePlugin { name: descriptor.name, display_name: descriptor.display_name, path }),
            Err(_) => legacy_ingest_plugin_name(&path).map(|name| VisiblePlugin { display_name: name.clone(), name, path }),
        })
        .collect::<Vec<_>>();
    plugins.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.path.cmp(&right.path)));
    plugins
}

pub fn list_visible_exporter_plugins() -> Vec<VisiblePlugin> {
    let roots = collect_exporter_plugin_search_roots();
    let mut diagnostics = Vec::new();
    let mut plugins = collect_plugin_candidates(&roots, "exporter", &mut diagnostics)
        .into_iter()
        .filter_map(|path| {
            read_exporter_descriptor(&path).ok().map(|descriptor| VisiblePlugin {
                name: descriptor.format_name,
                display_name: descriptor.display_name,
                path,
            })
        })
        .collect::<Vec<_>>();
    plugins.sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.path.cmp(&right.path)));
    plugins
}

fn resolve_ingest_plugin_path_from_roots(path: &Path, roots: &[PathBuf]) -> PathBuf {
    if path.exists() || path.parent().is_some_and(|parent| !parent.as_os_str().is_empty()) {
        return path.to_path_buf();
    }

    for root in roots {
        let candidate = root.join(path);
        if candidate.exists() {
            return candidate;
        }
    }

    path.to_path_buf()
}

fn find_ingest_plugin_by_name(name: &str, roots: &[PathBuf]) -> io::Result<PathBuf> {
    let mut diagnostics = Vec::new();
    for candidate in collect_ingest_plugin_candidates(roots, &mut diagnostics) {
        match read_ingest_descriptor(&candidate) {
            Ok(descriptor) if descriptor.name == name => return Ok(candidate),
            Ok(_) => {}
            Err(_) if legacy_ingest_plugin_name_matches(&candidate, name) => return Ok(candidate),
            Err(err) => diagnostics.push(err),
        }
    }

    let searched = roots.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(" -> ");
    let mut message = format!("ingest plugin `{name}` not found");
    if !searched.is_empty() {
        message.push_str(&format!("; searched: {searched}"));
    }
    if !diagnostics.is_empty() {
        message.push_str(&format!("; loader notes: {}", diagnostics.join(" | ")));
    }
    Err(io::Error::other(message))
}

fn legacy_ingest_plugin_name_matches(path: &Path, requested: &str) -> bool {
    legacy_ingest_plugin_name(path).is_some_and(|name| name == requested)
}

fn legacy_ingest_plugin_name(path: &Path) -> Option<String> {
    let stem = path.file_stem().and_then(|stem| stem.to_str())?;
    if !stem.ends_with("_ingest") && !stem.ends_with("-ingest") {
        return None;
    }
    Some(normalise_legacy_plugin_stem(stem))
}

fn normalise_legacy_plugin_stem(stem: &str) -> String {
    let mut name = stem.strip_prefix("lib").unwrap_or(stem);
    name = name.strip_prefix("lj_").or_else(|| name.strip_prefix("lj-")).or_else(|| name.strip_prefix("logjet_")).unwrap_or(name);
    name = name.strip_suffix("_ingest").or_else(|| name.strip_suffix("-ingest")).unwrap_or(name);
    name.to_ascii_lowercase().replace('_', "-")
}

fn collect_ingest_plugin_candidates(roots: &[PathBuf], diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    collect_plugin_candidates(roots, "ingest plugin", diagnostics)
}

fn collect_plugin_candidates(roots: &[PathBuf], kind: &str, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
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
                diagnostics.push(format!("cannot read {kind} directory {}: {err}", root.display()));
                continue;
            }
        };
        let mut entries = read_dir.filter_map(|entry| entry.ok().map(|ok| ok.path())).filter(|path| is_shared_library(path)).collect::<Vec<_>>();
        entries.sort();
        out.extend(entries);
    }
    out
}

fn is_shared_library(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if cfg!(target_os = "windows") {
        name.ends_with(".dll")
    } else if cfg!(target_os = "macos") {
        name.ends_with(".dylib")
    } else {
        name.ends_with(".so")
    }
}

struct IngestDescriptor {
    name: String,
    display_name: String,
    #[allow(dead_code)]
    supported_signals: u32,
}

struct ExporterDescriptor {
    format_name: String,
    display_name: String,
}

fn read_ingest_descriptor(path: &Path) -> std::result::Result<IngestDescriptor, String> {
    // SAFETY: operator-controlled plugin paths are loaded only to query a static descriptor.
    let lib = unsafe { libloading::Library::new(path) }.map_err(|err| format!("dlopen {}: {err}", path.display()))?;
    // SAFETY: symbol signature is defined by the ingest descriptor ABI.
    let descriptor_fn = unsafe {
        lib.get::<unsafe extern "C" fn() -> *const LjIngestDescriptorV1>(LJ_INGEST_DESCRIPTOR_SYMBOL)
            .map_err(|err| format!("symbol lj_ingest_descriptor_v1 in {}: {err}", path.display()))?
    };
    // SAFETY: plugin exported the descriptor function above.
    let descriptor_ptr = unsafe { descriptor_fn() };
    if descriptor_ptr.is_null() {
        return Err(format!("ingest plugin {} returned NULL descriptor", path.display()));
    }
    // SAFETY: descriptor pointer was checked for null and points to plugin-owned static storage.
    let descriptor = unsafe { &*descriptor_ptr };
    if descriptor.struct_size < std::mem::size_of::<LjIngestDescriptorV1>() as u32 {
        return Err(format!(
            "ingest plugin {} advertises descriptor size {} but host needs {}",
            path.display(),
            descriptor.struct_size,
            std::mem::size_of::<LjIngestDescriptorV1>()
        ));
    }
    if descriptor.abi_major != LJ_INGEST_ABI_MAJOR {
        return Err(format!(
            "ingest plugin {} uses ABI {}.{} but host needs {}.x",
            path.display(),
            descriptor.abi_major,
            descriptor.abi_minor,
            LJ_INGEST_ABI_MAJOR
        ));
    }
    if descriptor.abi_minor > LJ_INGEST_ABI_MINOR {
        return Err(format!(
            "ingest plugin {} uses newer ABI minor {}.{} than host supports {}.{}",
            path.display(),
            descriptor.abi_major,
            descriptor.abi_minor,
            LJ_INGEST_ABI_MAJOR,
            LJ_INGEST_ABI_MINOR
        ));
    }
    let name = read_c_string(descriptor.name, "name", path)?.trim().to_ascii_lowercase();
    if !is_valid_plugin_name(&name) {
        return Err(format!("ingest plugin {} returned invalid name `{name}`", path.display()));
    }
    let display_name = read_optional_c_string(descriptor.display_name).unwrap_or_else(|| name.clone());
    let raw_signals = (descriptor.reserved[0] & 0xFFFF_FFFF) as u32;
    let supported_signals = if raw_signals == 0 { LJ_INGEST_SIGNAL_LOGS } else { raw_signals };
    Ok(IngestDescriptor { name, display_name, supported_signals })
}

fn read_c_string(ptr: *const c_char, label: &str, path: &Path) -> std::result::Result<String, String> {
    if ptr.is_null() {
        return Err(format!("ingest plugin {} returned NULL {label}", path.display()));
    }
    // SAFETY: descriptor strings are plugin-owned NUL-terminated static strings.
    Ok(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

fn read_optional_c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: descriptor strings are plugin-owned NUL-terminated static strings.
    Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

fn is_valid_plugin_name(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'))
}

fn read_exporter_descriptor(path: &Path) -> std::result::Result<ExporterDescriptor, String> {
    // SAFETY: operator-controlled plugin paths are loaded only to query a static descriptor.
    let lib = unsafe { libloading::Library::new(path) }.map_err(|err| format!("dlopen {}: {err}", path.display()))?;
    // SAFETY: symbol signature is defined by the stable exporter descriptor ABI.
    let descriptor_fn = unsafe {
        lib.get::<unsafe extern "C" fn() -> *const LjxExporterDescriptorV1>(LJX_EXPORTER_DESCRIPTOR_V1_SYMBOL)
            .map_err(|err| format!("symbol ljx_exporter_descriptor_v1 in {}: {err}", path.display()))?
    };
    // SAFETY: plugin exported the descriptor function above.
    let descriptor_ptr = unsafe { descriptor_fn() };
    if descriptor_ptr.is_null() {
        return Err(format!("exporter plugin {} returned NULL descriptor", path.display()));
    }
    // SAFETY: descriptor pointer was checked for null and points to plugin-owned static storage.
    let descriptor = unsafe { &*descriptor_ptr };
    if descriptor.struct_size < std::mem::size_of::<LjxExporterDescriptorV1>() as u32 {
        return Err(format!(
            "exporter plugin {} advertises descriptor size {} but host needs {}",
            path.display(),
            descriptor.struct_size,
            std::mem::size_of::<LjxExporterDescriptorV1>()
        ));
    }
    if descriptor.abi_major != LJX_EXPORTER_ABI_MAJOR {
        return Err(format!(
            "exporter plugin {} uses ABI {}.{} but host needs {}.x",
            path.display(),
            descriptor.abi_major,
            descriptor.abi_minor,
            LJX_EXPORTER_ABI_MAJOR
        ));
    }
    if descriptor.abi_minor > LJX_EXPORTER_ABI_MINOR {
        return Err(format!(
            "exporter plugin {} uses newer ABI minor {}.{} than host supports {}.{}",
            path.display(),
            descriptor.abi_major,
            descriptor.abi_minor,
            LJX_EXPORTER_ABI_MAJOR,
            LJX_EXPORTER_ABI_MINOR
        ));
    }
    let format_name = abi_string_to_string(descriptor.format_name).trim().to_ascii_lowercase();
    if !is_valid_plugin_name(&format_name) {
        return Err(format!("exporter plugin {} returned invalid format name `{format_name}`", path.display()));
    }
    let display_name = non_empty_or(abi_string_to_string(descriptor.display_name), &format_name);
    Ok(ExporterDescriptor { format_name, display_name })
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

fn collect_ingest_plugin_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(raw) = env::var_os("LJD_INGEST_PLUGIN_PATH") {
        for entry in env::split_paths(&raw) {
            if !entry.as_os_str().is_empty() {
                push_unique_path(&mut roots, entry);
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        push_unique_path(&mut roots, cwd.join("ingestors"));
    }
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        push_unique_path(&mut roots, dir.join("ingestors"));
        push_unique_path(&mut roots, dir.join("../lib/logjet/ingestors"));
    }
    #[cfg(unix)]
    {
        push_unique_path(&mut roots, PathBuf::from("/usr/lib/logjet/ingestors"));
        push_unique_path(&mut roots, PathBuf::from("/usr/lib/logjet"));
    }

    roots
}

fn collect_exporter_plugin_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(raw) = env::var_os("LJX_EXPORTER_PATH") {
        for entry in env::split_paths(&raw) {
            if !entry.as_os_str().is_empty() {
                push_unique_path(&mut roots, entry);
            }
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
    #[cfg(unix)]
    {
        push_unique_path(&mut roots, PathBuf::from("/usr/lib/logjet/exporters"));
        push_unique_path(&mut roots, PathBuf::from("/usr/lib/logjet"));
    }

    roots
}

fn push_unique_path(roots: &mut Vec<PathBuf>, path: PathBuf) {
    if !roots.iter().any(|existing| existing == &path) {
        roots.push(path);
    }
}

// C ABI types mirroring liblogjet.h

// Legacy log-only signal mask (used when reserved[0] == 0 for old plugins).
#[allow(dead_code)]
const LJ_INGEST_SIGNAL_LOGS: u32 = 1 << 0;
#[allow(dead_code)]
const LJ_INGEST_SIGNAL_METRICS: u32 = 1 << 1;
#[allow(dead_code)]
const LJ_INGEST_SIGNAL_TRACES: u32 = 1 << 2;
#[allow(dead_code)]
const LJ_INGEST_SIGNAL_EVENTS: u32 = 1 << 3;

const LJ_INGEST_RECORD_TYPE_LOGS: u32 = 1;
const LJ_INGEST_RECORD_TYPE_METRICS: u32 = 2;
const LJ_INGEST_RECORD_TYPE_TRACES: u32 = 3;
const LJ_INGEST_RECORD_TYPE_EVENTS: u32 = 4;

#[allow(dead_code)]
const LJ_ATTR_STRING: i32 = 0;
const LJ_ATTR_INT: i32 = 1;
const LJ_ATTR_ARRAY: i32 = 2;

#[repr(C)]
struct LjAttribute {
    key: *const c_char,
    value: *const c_char,
    value_type: i32,
}

#[repr(C)]
struct LjLogRecord {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const LjAttribute,
    attributes_len: usize,
    // Extended OTel fields (NULL = legacy flat mode)
    event_name: *const c_char,
    service_name: *const c_char,
    scope_name: *const c_char,
    resource_attrs: *const LjAttribute,
    resource_attrs_len: usize,
    scope_attrs: *const LjAttribute,
    scope_attrs_len: usize,
}

/// Generic record delivered by multi-signal plugins (pre-encoded OTLP payload).
#[repr(C)]
struct LjIngestRecordV1 {
    struct_size: u32,
    record_type: u32, // LJ_INGEST_RECORD_TYPE_*
    timestamp_unix_ns: u64,
    payload: *const u8, // pre-encoded OTLP protobuf bytes
    payload_len: usize,
    flags: u32,
    reserved: [u64; 4],
}

/// Opaque plugin context. We never dereference it — just pass the pointer.
enum LjIngestPlugin {}

type CreateFn = unsafe extern "C" fn() -> *mut LjIngestPlugin;
type SetCallbackFn = unsafe extern "C" fn(*mut LjIngestPlugin, RecordCallback, *mut c_void);
type FeedFn = unsafe extern "C" fn(*mut LjIngestPlugin, *const u8, usize) -> c_int;
type FetchFn = unsafe extern "C" fn(*mut LjIngestPlugin) -> c_int;
type FreeFn = unsafe extern "C" fn(*mut LjIngestPlugin);
type LastErrorFn = unsafe extern "C" fn(*mut LjIngestPlugin) -> *const c_char;
type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);
type GenericRecordCallback = unsafe extern "C" fn(*mut c_void, *const LjIngestRecordV1);
type SetGenericCallbackFn = unsafe extern "C" fn(*mut LjIngestPlugin, GenericRecordCallback, *mut c_void);

// Plugin handle

/// Resolved symbols from a loaded ingest plugin.
struct PluginHandle {
    _lib: libloading::Library,
    create: CreateFn,
    set_callback: SetCallbackFn,
    feed: FeedFn,
    /// Active-source plugins export `lj_ingest_fetch`. If present, ljd calls
    /// it instead of accepting TCP connections and calling `lj_ingest_feed`.
    fetch: Option<FetchFn>,
    /// Multi-signal plugins export `lj_ingest_set_generic_callback`. If
    /// present, ljd calls it instead of `lj_ingest_set_callback`.
    set_generic_callback: Option<SetGenericCallbackFn>,
    /// Optional error message retrieval (`lj_ingest_last_error`).
    last_error: Option<LastErrorFn>,
    free: FreeFn,
}

impl PluginHandle {
    /// Loads the shared library at `path` and resolves required symbols.
    fn load(path: &Path) -> io::Result<Self> {
        // SAFETY: we trust the operator-provided .so path.
        let lib = unsafe { libloading::Library::new(path) }.map_err(|err| io::Error::other(format!("dlopen {}: {err}", path.display())))?;

        // SAFETY: symbol signatures must match the lj_ingest_* ABI.
        unsafe {
            let create: libloading::Symbol<CreateFn> =
                lib.get(b"lj_ingest_create\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_create: {err}")))?;
            let set_callback: libloading::Symbol<SetCallbackFn> =
                lib.get(b"lj_ingest_set_callback\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_set_callback: {err}")))?;
            let feed: libloading::Symbol<FeedFn> =
                lib.get(b"lj_ingest_feed\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_feed: {err}")))?;
            let fetch: Option<FetchFn> = lib.get::<FetchFn>(b"lj_ingest_fetch\0").ok().map(|sym| *sym);
            let set_generic_callback: Option<SetGenericCallbackFn> =
                lib.get::<SetGenericCallbackFn>(b"lj_ingest_set_generic_callback\0").ok().map(|sym| *sym);
            let last_error: Option<LastErrorFn> = lib.get::<LastErrorFn>(b"lj_ingest_last_error\0").ok().map(|sym| *sym);
            let free: libloading::Symbol<FreeFn> =
                lib.get(b"lj_ingest_free\0").map_err(|err| io::Error::other(format!("symbol lj_ingest_free: {err}")))?;

            Ok(Self { create: *create, set_callback: *set_callback, feed: *feed, fetch, set_generic_callback, last_error, free: *free, _lib: lib })
        }
    }

    /// Returns true if the plugin is an active source (exports `lj_ingest_fetch`).
    fn is_active(&self) -> bool {
        self.fetch.is_some()
    }
}

// Callback plumbing

/// Passed through the `void *user` pointer in the C callback.
struct CallbackCtx {
    spool: Arc<super::daemon::SharedSpool>,
    next_seq: Arc<AtomicU64>,
}

/// The C callback invoked by the plugin for each parsed record.
///
/// # Safety
///
/// `user` must be a valid `*mut CallbackCtx`. `record` must be a valid
/// `*const LjLogRecord` with all nested pointers valid for the call duration.
unsafe extern "C" fn on_record(user: *mut c_void, record: *const LjLogRecord) {
    let ctx = unsafe { &*(user as *const CallbackCtx) };
    let rec = unsafe { &*record };

    let body = if rec.body.is_null() { String::new() } else { unsafe { CStr::from_ptr(rec.body) }.to_string_lossy().into_owned() };

    let severity_text =
        if rec.severity_text.is_null() { None } else { Some(unsafe { CStr::from_ptr(rec.severity_text) }.to_string_lossy().into_owned()) };

    let attrs = unsafe { read_attrs(rec.attributes, rec.attributes_len) };

    let ts = if rec.timestamp_unix_ns != 0 {
        rec.timestamp_unix_ns
    } else {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
    };

    // Extended OTel fields (NULL = legacy flat mode).
    let event_name = unsafe { read_optional_str(rec.event_name) };
    let service_name = unsafe { read_optional_str(rec.service_name) };
    let scope_name = unsafe { read_optional_str(rec.scope_name) };
    let resource_attrs = unsafe { read_attrs(rec.resource_attrs, rec.resource_attrs_len) };
    let scope_attrs = unsafe { read_attrs(rec.scope_attrs, rec.scope_attrs_len) };

    let payload = build_otlp_payload(OtlpRecord {
        ts,
        severity: rec.severity_number,
        severity_text: severity_text.as_deref(),
        body: &body,
        attrs: &attrs,
        event_name: event_name.as_deref(),
        service_name: service_name.as_deref(),
        scope_name: scope_name.as_deref(),
        resource_attrs: &resource_attrs,
        scope_attrs: &scope_attrs,
    });

    let wire = WireRecord { record_type: logjet::RecordType::Logs, seq: ctx.next_seq.fetch_add(1, Ordering::Relaxed), ts_unix_ns: ts, payload };

    if let Err(err) = super::daemon::append_to_spool(&ctx.spool, wire) {
        eprintln!("ljd plugin callback spool error: {err}");
    }
}

/// The C callback invoked by the plugin for each generic (multi-signal) record.
///
/// # Safety
///
/// `user` must be a valid `*mut CallbackCtx`. `record` must be a valid
/// `*const LjIngestRecordV1` with payload pointer valid for the call duration.
unsafe extern "C" fn on_generic_record(user: *mut c_void, record: *const LjIngestRecordV1) {
    let ctx = unsafe { &*(user as *const CallbackCtx) };
    let rec = unsafe { &*record };

    let record_type = match rec.record_type {
        LJ_INGEST_RECORD_TYPE_LOGS => logjet::RecordType::Logs,
        LJ_INGEST_RECORD_TYPE_METRICS => logjet::RecordType::Metrics,
        LJ_INGEST_RECORD_TYPE_TRACES => logjet::RecordType::Traces,
        LJ_INGEST_RECORD_TYPE_EVENTS => logjet::RecordType::Events,
        _ => {
            eprintln!("ljd plugin callback unknown record type {}, defaulting to logs", rec.record_type);
            logjet::RecordType::Logs
        }
    };

    let payload = if rec.payload.is_null() || rec.payload_len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(rec.payload, rec.payload_len) }.to_vec()
    };

    let ts = if rec.timestamp_unix_ns != 0 {
        rec.timestamp_unix_ns
    } else {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
    };

    let seq = ctx.next_seq.fetch_add(1, Ordering::Relaxed);

    let wire = WireRecord { record_type, seq, ts_unix_ns: ts, payload };

    if let Err(err) = super::daemon::append_to_spool(&ctx.spool, wire) {
        eprintln!("ljd plugin callback spool error: {err}");
    }
}

/// Reads a NUL-terminated C string, returns None if the pointer is null.
unsafe fn read_optional_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() { None } else { Some(unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()) }
}

/// Reads an array of LjAttribute into owned (key, value, value_type) triples.
unsafe fn read_attrs(ptr: *const LjAttribute, len: usize) -> Vec<(String, String, i32)> {
    let mut out = Vec::new();
    if !ptr.is_null() && len > 0 {
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        for attr in slice {
            if attr.key.is_null() || attr.value.is_null() {
                continue;
            }
            let key = unsafe { CStr::from_ptr(attr.key) }.to_string_lossy().into_owned();
            let val = unsafe { CStr::from_ptr(attr.value) }.to_string_lossy().into_owned();
            out.push((key, val, attr.value_type));
        }
    }
    out
}

/// Input for OTLP payload construction.
pub(crate) struct OtlpRecord<'a> {
    pub ts: u64,
    pub severity: i32,
    pub severity_text: Option<&'a str>,
    pub body: &'a str,
    pub attrs: &'a [(String, String, i32)],
    // Extended (all None = legacy flat mode)
    pub event_name: Option<&'a str>,
    pub service_name: Option<&'a str>,
    pub scope_name: Option<&'a str>,
    pub resource_attrs: &'a [(String, String, i32)],
    pub scope_attrs: &'a [(String, String, i32)],
}

/// Encodes a single log record as an OTLP ExportLogsServiceRequest protobuf.
///
/// When `service_name` / `scope_name` / `event_name` are provided, builds a
/// spec-compliant OTLP structure with proper Resource and Scope. Otherwise
/// falls back to a minimal flat-attribute wrapper.
pub(crate) fn build_otlp_payload(rec: OtlpRecord<'_>) -> Vec<u8> {
    use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue};
    use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use prost::Message;

    let to_kv = |(k, v, t): &(String, String, i32)| KeyValue {
        key: k.clone(),
        value: Some(match *t {
            LJ_ATTR_INT => AnyValue { value: Some(Value::IntValue(v.parse::<i64>().unwrap_or(0))) },
            LJ_ATTR_ARRAY => AnyValue {
                value: Some(Value::ArrayValue(ArrayValue {
                    values: v.split(',').map(|s| AnyValue { value: Some(Value::StringValue(s.to_string())) }).collect(),
                })),
            },
            _ => AnyValue { value: Some(Value::StringValue(v.clone())) },
        }),
    };

    let record = LogRecord {
        time_unix_nano: rec.ts,
        observed_time_unix_nano: rec.ts,
        severity_number: rec.severity,
        severity_text: rec.severity_text.unwrap_or_default().to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(rec.body.to_string())) }),
        attributes: rec.attrs.iter().map(to_kv).collect(),
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: rec.event_name.unwrap_or_default().to_string(),
    };

    // Extended mode: proper Resource + Scope from plugin-provided fields.
    let has_extended = rec.service_name.is_some() || rec.scope_name.is_some();

    let mut resource_kv: Vec<KeyValue> = rec.resource_attrs.iter().map(to_kv).collect();
    if let Some(svc) = rec.service_name {
        resource_kv
            .insert(0, KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(svc.to_string())) }) });
    }

    let scope = InstrumentationScope {
        name: if has_extended { rec.scope_name.unwrap_or_default().to_string() } else { "lj-ingest-plugin".to_string() },
        version: String::new(),
        attributes: rec.scope_attrs.iter().map(to_kv).collect(),
        dropped_attributes_count: 0,
    };

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource { attributes: resource_kv, dropped_attributes_count: 0, entity_refs: Vec::new() }),
            scope_logs: vec![ScopeLogs { scope: Some(scope), log_records: vec![record], schema_url: String::new() }],
            schema_url: String::new(),
        }],
    };

    request.encode_to_vec()
}

// Public entry point

/// Runs the plugin ingest loop: loads the .so, then either calls
/// `lj_ingest_fetch` (active plugin) or binds TCP and feeds bytes (passive).
pub fn plugin_ingest_loop(
    bind_addr: &str, plugin_path: &Path, plugin_env: &[String], spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    // Set plugin-specific environment variables before loading.
    let prev_vars: Vec<_> = plugin_env
        .iter()
        .filter_map(|env| {
            let (k, v) = env.split_once('=')?;
            let prev = std::env::var(k).ok();
            unsafe { std::env::set_var(k, v) };
            Some((k.to_string(), prev))
        })
        .collect();

    let handle = Arc::new(PluginHandle::load(plugin_path)?);

    let result = if handle.is_active() {
        eprintln!("ljd ingest using active plugin {}", plugin_path.display());
        run_active_plugin(&handle, spool, next_seq)
    } else {
        let listener = TcpListener::bind(bind_addr)?;
        eprintln!("ljd ingest listening on {bind_addr} using passive plugin {}", plugin_path.display());
        let mut first = true;
        for stream in listener.incoming() {
            let stream = stream?;
            let peer = stream.peer_addr().ok();
            let handle = Arc::clone(&handle);
            let spool = Arc::clone(&spool);
            let next_seq = Arc::clone(&next_seq);

            thread::Builder::new().name("ljd-plugin-client".to_string()).spawn(move || {
                if let Err(err) = handle_plugin_client(stream, &handle, spool, next_seq) {
                    eprintln!("ljd plugin client error: {err}");
                }
                if let Some(peer) = peer {
                    eprintln!("ljd plugin client disconnected: {peer}");
                }
            })?;
            first = false;
        }
        #[allow(unreachable_code)]
        if first { Ok(()) } else { unreachable!() }
    };

    // Restore previous environment variables.
    for (k, prev) in prev_vars {
        match prev {
            Some(val) => unsafe { std::env::set_var(&k, val) },
            None => unsafe { std::env::remove_var(&k) },
        }
    }

    result
}

/// Handles a single TCP client through the plugin parser.
fn handle_plugin_client(
    stream: std::net::TcpStream, handle: &PluginHandle, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>,
) -> io::Result<()> {
    let ctx = Box::new(CallbackCtx { spool, next_seq });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    // SAFETY: we control the lifetime — ctx_ptr stays valid until we Box::from_raw below.
    let plugin_ctx = unsafe { (handle.create)() };
    if plugin_ctx.is_null() {
        // Reclaim the ctx box before returning.
        let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
        return Err(io::Error::other("lj_ingest_create returned NULL"));
    }
    unsafe { (handle.set_callback)(plugin_ctx, on_record, ctx_ptr) };
    if let Some(set_generic) = handle.set_generic_callback {
        unsafe { set_generic(plugin_ctx, on_generic_record, ctx_ptr) };
    }

    let mut reader = BufReader::new(stream);
    let mut buf = [0u8; 0x10_000];
    let result = loop {
        match reader.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => {
                let rc = unsafe { (handle.feed)(plugin_ctx, buf.as_ptr(), n) };
                if rc != 0 {
                    break Err(io::Error::other(format!("lj_ingest_feed returned error code {rc}")));
                }
            }
            Err(err) => break Err(err),
        }
    };

    unsafe { (handle.free)(plugin_ctx) };
    let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
    result
}

/// Runs an active-source plugin that owns its own I/O via `lj_ingest_fetch`.
fn run_active_plugin(handle: &PluginHandle, spool: Arc<super::daemon::SharedSpool>, next_seq: Arc<AtomicU64>) -> io::Result<()> {
    let fetch = handle.fetch.ok_or_else(|| io::Error::other("plugin has no lj_ingest_fetch"))?;

    let ctx = Box::new(CallbackCtx { spool, next_seq });
    let ctx_ptr = Box::into_raw(ctx) as *mut c_void;

    let plugin_ctx = unsafe { (handle.create)() };
    if plugin_ctx.is_null() {
        let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };
        return Err(io::Error::other("lj_ingest_create returned NULL"));
    }
    unsafe { (handle.set_callback)(plugin_ctx, on_record, ctx_ptr) };
    if let Some(set_generic) = handle.set_generic_callback {
        unsafe { set_generic(plugin_ctx, on_generic_record, ctx_ptr) };
    }

    let rc = unsafe { fetch(plugin_ctx) };

    if rc != 0
        && let Some(last_error_fn) = handle.last_error
    {
        let msg = unsafe { last_error_fn(plugin_ctx) };
        if !msg.is_null() {
            let msg_str = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
            eprintln!("ljd plugin error: {msg_str}");
        }
    }

    unsafe { (handle.free)(plugin_ctx) };
    let _ = unsafe { Box::from_raw(ctx_ptr as *mut CallbackCtx) };

    if rc != 0 {
        return Err(io::Error::other(format!("lj_ingest_fetch returned error code {rc}")));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/unit/plugin_utst.rs"]
mod plugin_utst;
