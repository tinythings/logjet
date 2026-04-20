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
