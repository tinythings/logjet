//! Stress-test ingest plugin for ljd.
//!
//! Active-source plugin (exports `lj_ingest_fetch`) that generates thousands
//! of records with variable payload sizes,
//! many attributes, rapid-fire delivery. No external dependencies — pure fake data.

use std::ffi::{CString, c_char, c_int, c_void};

// ── C ABI types (must match liblogjet.h) ────────────────────────────────────

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

type RecordCallback = unsafe extern "C" fn(*mut c_void, *const LjLogRecord);

// ── Plugin context ──────────────────────────────────────────────────────────

pub struct StressPlugin {
    callback: Option<RecordCallback>,
    user: *mut c_void,
}

// ── Exported C ABI ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn lj_ingest_create() -> *mut StressPlugin {
    Box::into_raw(Box::new(StressPlugin { callback: None, user: std::ptr::null_mut() }))
}

/// # Safety
///
/// `ctx` must be valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_set_callback(ctx: *mut StressPlugin, cb: RecordCallback, user: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    ctx.callback = Some(cb);
    ctx.user = user;
}

/// Passive feed — unused but required by ABI.
///
/// # Safety
///
/// `_ctx` must be either null or a valid plugin pointer created by
/// `lj_ingest_create`. `_data` and `_len` are ignored by this plugin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_feed(_ctx: *mut StressPlugin, _data: *const u8, _len: usize) -> c_int {
    0
}

/// Active source — generates 25,000 records with varied payload sizes.
///
/// # Safety
///
/// `ctx` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_fetch(ctx: *mut StressPlugin) -> c_int {
    if ctx.is_null() {
        return -1;
    }
    let ctx = unsafe { &*ctx };
    let Some(cb) = ctx.callback else {
        return -1;
    };

    let count = 25_000u64;

    for i in 0..count {
        emit_record(ctx, cb, i);
    }

    eprintln!("lj-stress-ingest: emitted {count} records");
    0
}

/// # Safety
///
/// `ctx` must be valid or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lj_ingest_free(ctx: *mut StressPlugin) {
    if ctx.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(ctx) };
}

// ── Record generation ───────────────────────────────────────────────────────

fn emit_record(ctx: &StressPlugin, cb: RecordCallback, seq: u64) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;

    // Vary body size — realistic distribution.
    let body_size = match seq % 20 {
        0 => 4000, // large JSON-like (~4KB records)
        1 => 3000,
        2..=5 => 500, // medium
        6..=9 => 200, // smallish
        _ => 80,      // tiny (most common)
    };
    let body = make_body(seq, body_size);
    let body_c = cstring_lossy(&body);

    let (severity, sev_text) = match seq % 5 {
        0 => (5i32, "DEBUG"),
        1 => (9, "INFO"),
        2 => (9, "INFO"),
        3 => (13, "WARN"),
        _ => (17, "ERROR"),
    };
    let sev_c = cstring_lossy(sev_text);

    // 5 attributes — realistic count.
    let svc_key = cstring_lossy("service.name");
    let svc_val = cstring_lossy(&format!("vendor-harman-tuner-service{}", seq % 3));
    let scope_key = cstring_lossy("scope.name");
    let scope_val = cstring_lossy(&format!("TooMany_RUDI_GATEWAY_VIWI_RUDISVC_EnergyFlowState{}", seq % 7));
    let mt_key = cstring_lossy("stress.msg_type");
    let mt_val = cstring_lossy("12");
    let pnr_key = cstring_lossy("stress.record_nr");
    let pnr_val = cstring_lossy(&format!("{}", seq * 1000 + 208502));
    let ts_key = cstring_lossy("stress.origin_ts_ns");
    let ts_val = cstring_lossy(&format!("{}", 959727165693860u64 + seq));

    let attrs = [
        LjAttribute { key: svc_key.as_ptr(), value: svc_val.as_ptr() },
        LjAttribute { key: scope_key.as_ptr(), value: scope_val.as_ptr() },
        LjAttribute { key: mt_key.as_ptr(), value: mt_val.as_ptr() },
        LjAttribute { key: pnr_key.as_ptr(), value: pnr_val.as_ptr() },
        LjAttribute { key: ts_key.as_ptr(), value: ts_val.as_ptr() },
    ];

    let record = LjLogRecord {
        timestamp_unix_ns: now,
        severity_number: severity,
        severity_text: sev_c.as_ptr(),
        body: body_c.as_ptr(),
        attributes: attrs.as_ptr(),
        attributes_len: attrs.len(),
    };

    unsafe { cb(ctx.user, &record) };
}

/// Generates a body string of the given size with realistic-looking content.
fn make_body(seq: u64, size: usize) -> String {
    if size < 100 {
        return format!("[{seq}:{}:0] short log message padding={:0>width$}", seq % 2000, seq, width = size.saturating_sub(50));
    }

    let mut s = format!(
        "[{pid}:{tid}:{seq}] handler_station_CGStationHandler_updateStation: \
         name=StationName(value=Absolut GERMANY, scrolling=false), airLogoId=null, \
         ptyCode=10, relatedContent={{DabSidExtEnsImpl(rawValue={raw})}}, \
         selector=MediumLevelStationSelector(lowLevelStationSelector=SidExtEns(\
         primaryIdentifier=DabSidExtEnsImpl(rawValue={raw}), \
         secondaryIdentifiers={{DabEnsembleImpl(rawValue=180064), ServiceEccImpl(rawValue=224)}})), \
         DabFrequencyImpl(rawValue=180064), mcc=262, freqBasedHybridVisuals=null, \
         airLogoFileId=FileId(filename=884372_-1998220465.webp, partition=AIRLOGOS)",
        pid = 4834 + seq % 100,
        tid = 5428 + seq % 200,
        raw = 1234549346844u64 + seq,
    );

    // Pad or truncate to target size.
    while s.len() < size {
        s.push_str(", extra_padding_data=true");
    }
    s.truncate(size);
    s
}

fn cstring_lossy(s: &str) -> CString {
    let clean: String = s.chars().map(|c| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c }).collect();
    CString::new(clean).unwrap_or_else(|_| CString::new("?").expect("static"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[derive(Debug)]
    struct CapturedRecord {
        severity: i32,
        severity_text: String,
        body: String,
        attrs: Vec<(String, String)>,
    }

    unsafe extern "C" fn capture_record(user: *mut c_void, record: *const LjLogRecord) {
        let captured = unsafe { &mut *(user as *mut Vec<CapturedRecord>) };
        let record = unsafe { &*record };

        let severity_text = if record.severity_text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(record.severity_text) }.to_string_lossy().into_owned()
        };
        let body = if record.body.is_null() { String::new() } else { unsafe { CStr::from_ptr(record.body) }.to_string_lossy().into_owned() };

        let attrs = if record.attributes.is_null() || record.attributes_len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(record.attributes, record.attributes_len) }
                .iter()
                .map(|attr| {
                    let key = unsafe { CStr::from_ptr(attr.key) }.to_string_lossy().into_owned();
                    let value = unsafe { CStr::from_ptr(attr.value) }.to_string_lossy().into_owned();
                    (key, value)
                })
                .collect()
        };

        captured.push(CapturedRecord { severity: record.severity_number, severity_text, body, attrs });
    }

    #[test]
    fn stress_plugin_fetch_emits_consistent_records() {
        let plugin = lj_ingest_create();
        assert!(!plugin.is_null());

        let mut captured: Vec<CapturedRecord> = Vec::new();
        let captured_ptr = &mut captured as *mut Vec<CapturedRecord> as *mut c_void;

        unsafe {
            lj_ingest_set_callback(plugin, capture_record, captured_ptr);
            assert_eq!(lj_ingest_fetch(plugin), 0);
            lj_ingest_free(plugin);
        }

        assert_eq!(captured.len(), 25_000);

        for (index, record) in captured.iter().enumerate().step_by(257) {
            assert!(record.severity > 0, "record {index} severity should stay valid");
            assert!(!record.severity_text.is_empty(), "record {index} severity text should stay valid");
            assert!(!record.body.is_empty(), "record {index} body should not be empty");
            assert_eq!(record.attrs.len(), 5, "record {index} should carry 5 attrs");

            let keys = record.attrs.iter().map(|(key, _value)| key.as_str()).collect::<Vec<_>>();
            assert_eq!(
                keys,
                vec!["service.name", "scope.name", "stress.msg_type", "stress.record_nr", "stress.origin_ts_ns"],
                "record {index} attribute keys changed unexpectedly"
            );
            assert!(
                record.body.contains("handler_station_CGStationHandler_updateStation") || record.body.contains("short log message"),
                "record {index} body looks garbled: {}",
                record.body
            );
        }
    }
}
