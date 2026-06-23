use std::ffi::{CStr, CString};
use std::ptr;
use std::time::Duration;

use opentelemetry_proto::tonic::common::v1::any_value::Value;

use super::{Backend, HttpEndpoint, LjLogRecord, Logger, build_batch_request};

fn test_logger(service: &str) -> Logger {
    Logger {
        backend: Backend::Http(HttpEndpoint {
            authority: "127.0.0.1:4318".to_string(),
            host_header: "127.0.0.1:4318".to_string(),
            path: "/v1/logs".to_string(),
        }),
        service_name: service.to_string(),
        timeout: Duration::from_millis(1000),
    }
}

fn record(ts: u64, body: &CStr, service: Option<&CStr>, scope: Option<&CStr>) -> LjLogRecord {
    LjLogRecord {
        timestamp_unix_ns: ts,
        severity_number: 9,
        severity_text: ptr::null(),
        body: body.as_ptr(),
        attributes: ptr::null(),
        attributes_len: 0,
        event_name: ptr::null(),
        service_name: service.map_or(ptr::null(), CStr::as_ptr),
        scope_name: scope.map_or(ptr::null(), CStr::as_ptr),
        resource_attrs: ptr::null(),
        resource_attrs_len: 0,
        scope_attrs: ptr::null(),
        scope_attrs_len: 0,
    }
}

#[test]
fn identical_records_collapse_into_one_scope() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let records = vec![record(1, &body, None, None), record(2, &body, None, None), record(3, &body, None, None)];

    let request = build_batch_request(&logger, &records).expect("batch request");

    assert_eq!(request.resource_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs[0].log_records.len(), 3);
}

#[test]
fn differing_scope_names_split_scopes() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let scope_a = CString::new("scope-a").unwrap();
    let scope_b = CString::new("scope-b").unwrap();
    let records = vec![record(1, &body, None, Some(&scope_a)), record(2, &body, None, Some(&scope_b))];

    let request = build_batch_request(&logger, &records).expect("batch request");

    assert_eq!(request.resource_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs.len(), 2);
}

#[test]
fn differing_service_names_split_resources() {
    let logger = test_logger("default-svc");
    let body = CString::new("hello").unwrap();
    let service_a = CString::new("svc-a").unwrap();
    let service_b = CString::new("svc-b").unwrap();
    let records = vec![record(1, &body, Some(&service_a), None), record(2, &body, Some(&service_b), None)];

    let request = build_batch_request(&logger, &records).expect("batch request");

    assert_eq!(request.resource_logs.len(), 2);
}

#[test]
fn injects_service_name_into_resource() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let records = vec![record(1, &body, None, None)];

    let request = build_batch_request(&logger, &records).expect("batch request");

    let attributes = &request.resource_logs[0].resource.as_ref().expect("resource").attributes;
    let service = attributes.iter().find(|kv| kv.key == "service.name").expect("service.name present");
    match service.value.as_ref().and_then(|value| value.value.as_ref()) {
        Some(Value::StringValue(name)) => assert_eq!(name, "svc"),
        other => panic!("unexpected service.name value: {other:?}"),
    }
}

#[test]
fn empty_slice_yields_no_resource_logs() {
    let logger = test_logger("svc");
    let request = build_batch_request(&logger, &[]).expect("batch request");
    assert!(request.resource_logs.is_empty());
}

#[test]
fn missing_body_is_an_error() {
    let logger = test_logger("svc");
    let mut bad = record(1, c"x", None, None);
    bad.body = ptr::null();
    let records = vec![bad];

    assert!(build_batch_request(&logger, &records).is_err());
}

#[test]
fn timestamps_are_preserved_per_record() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let records = vec![record(111, &body, None, None), record(222, &body, None, None)];

    let request = build_batch_request(&logger, &records).expect("batch request");

    let logs = &request.resource_logs[0].scope_logs[0].log_records;
    assert_eq!(logs[0].time_unix_nano, 111);
    assert_eq!(logs[1].time_unix_nano, 222);
}
