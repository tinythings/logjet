//! Android logcat ingest plugin for ljd.
//!
//! Active-source plugin: exports `lj_ingest_fetch` which reads logcat
//! lines from stdin (pipe `adb logcat` into ljd) and delivers parsed
//! records through the callback.
//!
//! Parses the default `threadtime` format:
//!   `MM-DD HH:MM:SS.mmm  PID  TID LEVEL TAG     : message`
//! and the `brief` format:
//!   `L/TAG(PID): message`

use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{self, BufRead, BufReader};
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

// ── Severity constants ──────────────────────────────────────────────────────

const LJ_SEVERITY_TRACE: i32 = 1;
const LJ_SEVERITY_DEBUG: i32 = 5;
const LJ_SEVERITY_INFO: i32 = 9;
const LJ_SEVERITY_WARN: i32 = 13;
const LJ_SEVERITY_ERROR: i32 = 17;
const LJ_SEVERITY_FATAL: i32 = 21;

// ── Plugin context ──────────────────────────────────────────────────────────

pub struct LogcatPlugin {
    callback: Option<RecordCallback>,
    user: *mut c_void,
}

// ── Exported C ABI ──────────────────────────────────────────────────────────

/// Creates a new logcat parsing context.
#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_create() -> *mut LogcatPlugin {
    Box::into_raw(Box::new(LogcatPlugin { callback: None, user: std::ptr::null_mut() }))
}

/// Registers the record-delivery callback.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_set_callback(ctx: *mut LogcatPlugin, cb: RecordCallback, user: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.callback = Some(cb);
    ctx.user = user;
}

/// Passive feed — not used by this plugin but required by the ABI.
///
/// # Safety
///
/// Pointers must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_feed(_ctx: *mut LogcatPlugin, _data: *const u8, _len: usize) -> c_int {
    // Active plugin — feed is a no-op.
    0
}

/// Active source: reads logcat lines from stdin until EOF.
/// Pipe `adb logcat` into ljd to use this.
///
/// # Safety
///
/// `ctx` must be a valid pointer from `lj_ingest_create` with a callback set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_fetch(ctx: *mut LogcatPlugin) -> c_int {
    if ctx.is_null() {
        return -1;
    }
    let ctx = unsafe { &*ctx };
    let reader = BufReader::new(io::stdin().lock());
    for line in reader.lines() {
        match line {
            Ok(line) if line.is_empty() => continue,
            Ok(line) => emit_record(ctx, &line),
            Err(_) => break,
        }
    }
    0
}

/// Destroys the plugin context. Accepts NULL.
///
/// # Safety
///
/// `ctx` must be null or a valid pointer that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_free(ctx: *mut LogcatPlugin) {
    if ctx.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(ctx) };
}

// ── Logcat parsing ──────────────────────────────────────────────────────────

struct Parsed<'a> {
    severity: i32,
    severity_text: &'static str,
    pid: &'a str,
    tid: &'a str,
    tag: &'a str,
    body: &'a str,
}

/// Parses a logcat `threadtime` line:
///   `06-11 22:14:15.123  1234  5678 I MyApp   : some message`
/// Falls back to `brief` format:
///   `I/MyApp(1234): message`
fn parse_logcat(line: &str) -> Parsed<'_> {
    // Try threadtime: date(5) space time(12) spaces pid spaces tid space level space tag colon-space message
    if let Some(parsed) = try_threadtime(line) {
        return parsed;
    }
    if let Some(parsed) = try_brief(line) {
        return parsed;
    }
    // Unparseable — raw body.
    Parsed { severity: LJ_SEVERITY_INFO, severity_text: "INFO", pid: "", tid: "", tag: "", body: line }
}

/// Tries to parse `threadtime` format.
fn try_threadtime(line: &str) -> Option<Parsed<'_>> {
    // Minimum: "MM-DD HH:MM:SS.mmm" = 18 chars + fields after
    if line.len() < 25 {
        return None;
    }
    let bytes = line.as_bytes();
    // "06-11 22:14:15.123" — dash at [2], space at [5], colon at [8]
    if bytes[2] != b'-' || bytes[5] != b' ' || bytes[8] != b':' {
        return None;
    }

    // Skip "MM-DD HH:MM:SS.mmm" — find the rest after the timestamp.
    // The timestamp is always 18 chars, followed by spaces.
    let rest = line.get(18..)?.trim_start();

    // rest = "PID  TID LEVEL TAG     : message"
    let (pid, rest) = split_first_token(rest)?;
    let rest = rest.trim_start();
    let (tid, rest) = split_first_token(rest)?;
    let rest = rest.trim_start();

    // Next char is the level letter.
    let level_char = rest.as_bytes().first().copied()?;
    let rest = rest.get(1..)?.trim_start();

    // rest = "TAG     : message"
    let (tag, body) = if let Some(colon_pos) = rest.find(": ") { (rest[..colon_pos].trim(), &rest[colon_pos + 2..]) } else { (rest.trim(), "") };

    let (severity, severity_text) = map_logcat_level(level_char);
    Some(Parsed { severity, severity_text, pid, tid, tag, body })
}

/// Tries to parse `brief` format: `I/MyApp(1234): message`
fn try_brief(line: &str) -> Option<Parsed<'_>> {
    let bytes = line.as_bytes();
    if bytes.len() < 4 || bytes[1] != b'/' {
        return None;
    }
    let level_char = bytes[0];
    let rest = &line[2..];

    let (tag_pid, body) =
        if let Some(colon_pos) = rest.find("): ") { (&rest[..colon_pos + 1], rest[colon_pos + 3..].trim_start()) } else { (rest, "") };

    // tag_pid = "MyApp(1234)" or "MyApp( 1234)"
    let (tag, pid) = if let Some(paren) = tag_pid.find('(') {
        let tag = &tag_pid[..paren];
        let pid = tag_pid[paren + 1..].trim_end_matches(')').trim();
        (tag, pid)
    } else {
        (tag_pid, "")
    };

    let (severity, severity_text) = map_logcat_level(level_char);
    Some(Parsed { severity, severity_text, pid, tid: "", tag, body })
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace)?;
    Some((&s[..end], &s[end..]))
}

/// Maps logcat single-char level to OTel severity.
fn map_logcat_level(ch: u8) -> (i32, &'static str) {
    match ch {
        b'V' => (LJ_SEVERITY_TRACE, "TRACE"),
        b'D' => (LJ_SEVERITY_DEBUG, "DEBUG"),
        b'I' => (LJ_SEVERITY_INFO, "INFO"),
        b'W' => (LJ_SEVERITY_WARN, "WARN"),
        b'E' => (LJ_SEVERITY_ERROR, "ERROR"),
        b'F' | b'A' => (LJ_SEVERITY_FATAL, "FATAL"),
        b'S' => (LJ_SEVERITY_FATAL, "FATAL"),
        _ => (LJ_SEVERITY_INFO, "INFO"),
    }
}

// ── Record emission ─────────────────────────────────────────────────────────

fn emit_record(ctx: &LogcatPlugin, line: &str) {
    let Some(cb) = ctx.callback else { return };

    let parsed = parse_logcat(line);

    let body_c = cstring_lossy(parsed.body);
    let severity_text_c = cstring_lossy(parsed.severity_text);

    let tag_key = cstring_lossy("logcat.tag");
    let tag_val = cstring_lossy(parsed.tag);
    let pid_key = cstring_lossy("logcat.pid");
    let pid_val = cstring_lossy(parsed.pid);
    let tid_key = cstring_lossy("logcat.tid");
    let tid_val = cstring_lossy(parsed.tid);

    let attrs = [
        LjAttribute { key: tag_key.as_ptr(), value: tag_val.as_ptr() },
        LjAttribute { key: pid_key.as_ptr(), value: pid_val.as_ptr() },
        LjAttribute { key: tid_key.as_ptr(), value: tid_val.as_ptr() },
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

fn cstring_lossy(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_else(|_| CString::new("?").expect("static"))
}

#[cfg(test)]
#[path = "../tests/unit/tests.rs"]
mod tests;
