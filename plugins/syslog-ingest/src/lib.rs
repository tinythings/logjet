//! Syslog ingest plugin for ljd.
//!
//! Implements the `lj_ingest_*` C ABI defined in `liblogjet.h`.
//! Parses RFC 3164 and RFC 5424 syslog messages from a raw TCP byte stream,
//! splits on newlines, and calls back with `lj_log_record` for each message.

use std::ffi::{CString, c_char, c_int, c_void};
use std::time::{SystemTime, UNIX_EPOCH};

// ── C ABI types (must match liblogjet.h exactly) ────────────────────────────

#[repr(C)]
pub struct LjAttribute {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
pub struct LjLogRecord {
    timestamp_unix_ns: u64,
    severity_number: i32,
    severity_text: *const c_char,
    body: *const c_char,
    attributes: *const LjAttribute,
    attributes_len: usize,
}

pub type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);

// ── Severity constants matching liblogjet.h ─────────────────────────────────

const LJ_SEVERITY_TRACE: i32 = 1;
const LJ_SEVERITY_DEBUG: i32 = 5;
const LJ_SEVERITY_INFO: i32 = 9;
const LJ_SEVERITY_WARN: i32 = 13;
const LJ_SEVERITY_ERROR: i32 = 17;
const LJ_SEVERITY_FATAL: i32 = 21;

// ── Plugin context ──────────────────────────────────────────────────────────

/// Parsing context accumulates partial lines from the TCP stream.
pub struct SyslogPlugin {
    buf: Vec<u8>,
    callback: Option<RecordCallback>,
    user: *mut c_void,
}

// ── Exported C ABI ──────────────────────────────────────────────────────────

/// Creates a new syslog parsing context.
#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_create() -> *mut SyslogPlugin {
    Box::into_raw(Box::new(SyslogPlugin { buf: Vec::with_capacity(8192), callback: None, user: std::ptr::null_mut() }))
}

/// Registers the record-delivery callback.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_set_callback(ctx: *mut SyslogPlugin, cb: RecordCallback, user: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.callback = Some(cb);
    ctx.user = user;
}

/// Feeds raw bytes from the TCP stream into the syslog parser.
/// Splits on `\n`, parses each complete line, fires the callback.
/// Returns 0 on success.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create`.
/// `data` must point to at least `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_feed(ctx: *mut SyslogPlugin, data: *const u8, len: usize) -> c_int {
    if ctx.is_null() || data.is_null() {
        return -1;
    }
    let ctx = unsafe { &mut *ctx };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };

    ctx.buf.extend_from_slice(bytes);

    while let Some(pos) = ctx.buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = ctx.buf.drain(..=pos).collect();
        let trimmed = strip_trailing_whitespace(&line);
        if !trimmed.is_empty() {
            emit_record(ctx, trimmed);
        }
    }

    // Guard against a single line exceeding a sane limit (256 KiB).
    if ctx.buf.len() > 256 * 1024 {
        ctx.buf.clear();
    }

    0
}

/// Destroys the plugin context. Accepts NULL.
///
/// # Safety
///
/// `ctx` must be null or a valid pointer from `lj_ingest_create` that
/// has not already been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_free(ctx: *mut SyslogPlugin) {
    if ctx.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(ctx) };
}

// ── Syslog parsing ─────────────────────────────────────────────────────────

/// Parsed fields from a syslog line.
struct Parsed<'a> {
    severity: i32,
    severity_text: &'static str,
    facility_text: &'static str,
    hostname: &'a str,
    app_name: &'a str,
    body: &'a str,
}

/// Parses a single syslog line (RFC 3164 / RFC 5424 priority prefix).
fn parse_syslog(line: &str) -> Parsed<'_> {
    if let Some(rest) = line.strip_prefix('<')
        && let Some(end) = rest.find('>')
        && let Ok(pri) = rest[..end].parse::<u32>()
    {
        let after_pri = &rest[end + 1..];
        let facility = pri >> 3;
        let syslog_severity = pri & 0x07;
        let (otel_severity, severity_text) = map_syslog_severity(syslog_severity);
        let facility_text = facility_name(facility);

        // Skip RFC 5424 version digit if present (e.g. "1 ")
        let msg = if after_pri.starts_with(|c: char| c.is_ascii_digit()) { after_pri.get(2..).unwrap_or(after_pri) } else { after_pri };

        let (hostname, app_name, body) = extract_header_fields(msg);
        return Parsed { severity: otel_severity, severity_text, facility_text, hostname, app_name, body };
    }

    // Fallback: unparseable line, treat as INFO with raw body.
    Parsed { severity: LJ_SEVERITY_INFO, severity_text: "INFO", facility_text: "user", hostname: "", app_name: "", body: line }
}

/// Extracts hostname, app_name, and body from the message portion after the PRI.
fn extract_header_fields(msg: &str) -> (&str, &str, &str) {
    let parts: Vec<&str> = msg.splitn(4, ' ').collect();
    // RFC 3164: "Mon DD HH:MM:SS hostname app: msg"
    if parts.len() >= 4 && looks_like_month(parts[0]) {
        let rest = parts[3];
        let (hostname, after_host) = rest.split_once(' ').unwrap_or((rest, ""));
        let (app_name, body) = split_app_body(after_host);
        (hostname, app_name, body)
    } else if parts.len() >= 3 {
        // RFC 5424 or non-standard. First token = hostname, rest = app: body.
        let rest_after_host = msg.splitn(3, ' ').nth(2).unwrap_or("");
        let (app, bod) = split_app_body(rest_after_host);
        (parts[0], app, bod)
    } else {
        ("", "", msg)
    }
}

/// Splits "app_name: body" or "app_name[pid]: body" into (app_name, body).
fn split_app_body(s: &str) -> (&str, &str) {
    if let Some(colon_pos) = s.find(':') {
        let app_raw = &s[..colon_pos];
        let app = app_raw.split('[').next().unwrap_or(app_raw).trim();
        let body = s[colon_pos + 1..].trim_start();
        (app, body)
    } else {
        ("", s)
    }
}

fn looks_like_month(s: &str) -> bool {
    matches!(s, "Jan" | "Feb" | "Mar" | "Apr" | "May" | "Jun" | "Jul" | "Aug" | "Sep" | "Oct" | "Nov" | "Dec")
}

/// Maps syslog severity (0=emerg .. 7=debug) to OTel severity number + text.
fn map_syslog_severity(syslog_sev: u32) -> (i32, &'static str) {
    match syslog_sev {
        0 => (LJ_SEVERITY_FATAL, "FATAL"),
        1 => (LJ_SEVERITY_FATAL, "FATAL"),
        2 => (LJ_SEVERITY_FATAL, "FATAL"),
        3 => (LJ_SEVERITY_ERROR, "ERROR"),
        4 => (LJ_SEVERITY_WARN, "WARN"),
        5 => (LJ_SEVERITY_INFO, "INFO"),
        6 => (LJ_SEVERITY_INFO, "INFO"),
        7 => (LJ_SEVERITY_DEBUG, "DEBUG"),
        _ => (LJ_SEVERITY_TRACE, "TRACE"),
    }
}

/// Maps facility number to a human name (RFC 5424 §6.2.1).
fn facility_name(facility: u32) -> &'static str {
    const TABLE: [&str; 24] = [
        "kern", "user", "mail", "daemon", "auth", "syslog", "lpr", "news", "uucp", "cron", "authpriv", "ftp", "ntp", "audit", "console", "clock",
        "local0", "local1", "local2", "local3", "local4", "local5", "local6", "local7",
    ];
    TABLE.get(facility as usize).copied().unwrap_or("unknown")
}

// ── Record emission ─────────────────────────────────────────────────────────

/// Emits a parsed syslog record through the C callback.
fn emit_record(ctx: &SyslogPlugin, line: &[u8]) {
    let Some(cb) = ctx.callback else { return };

    let line_str = String::from_utf8_lossy(line);
    let parsed = parse_syslog(&line_str);

    let body_c = cstring_lossy(parsed.body);
    let severity_text_c = cstring_lossy(parsed.severity_text);

    let facility_key = cstring_lossy("syslog.facility");
    let facility_val = cstring_lossy(parsed.facility_text);
    let hostname_key = cstring_lossy("syslog.hostname");
    let hostname_val = cstring_lossy(parsed.hostname);
    let appname_key = cstring_lossy("syslog.appname");
    let appname_val = cstring_lossy(parsed.app_name);

    let attrs = [
        LjAttribute { key: facility_key.as_ptr(), value: facility_val.as_ptr() },
        LjAttribute { key: hostname_key.as_ptr(), value: hostname_val.as_ptr() },
        LjAttribute { key: appname_key.as_ptr(), value: appname_val.as_ptr() },
    ];

    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;

    let record = LjLogRecord {
        timestamp_unix_ns: ts,
        severity_number: parsed.severity,
        severity_text: severity_text_c.as_ptr(),
        body: body_c.as_ptr(),
        attributes: attrs.as_ptr(),
        attributes_len: attrs.len(),
    };

    unsafe { cb(ctx.user, &record) };
}

fn strip_trailing_whitespace(data: &[u8]) -> &[u8] {
    let end = data.iter().rposition(|&b| !b.is_ascii_whitespace()).map(|i| i + 1).unwrap_or(0);
    &data[..end]
}

fn cstring_lossy(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_else(|_| CString::new("?").expect("static"))
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
