use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use opentelemetry_proto::tonic::common::v1::any_value::Value;

use super::{
    AsyncEngine, Backend, HttpClient, HttpEndpoint, HttpPool, LjAttribute, LjLogRecord, Logger,
    LJ_ATTR_ARRAY, LJ_ATTR_INT, LJ_ATTR_STRING,
    build_batch_request, build_request, normalise_grpc_endpoint, parse_http_endpoint,
    read_attrs, record_to_log, resolve_resource, resolve_scope, severity_text, triples_to_kvs,
};

fn test_logger(service: &str) -> Logger {
    let pool = std::sync::Arc::new(HttpPool {
        endpoint: HttpEndpoint { authority: "127.0.0.1:4318".to_string(), host_header: "127.0.0.1:4318".to_string(), path: "/v1/logs".to_string() },
        idle: Mutex::new(Vec::new()),
    });
    Logger {
        backend: Backend::Http(HttpClient { runtime: OnceLock::new(), engine: AsyncEngine::new(), pool }),
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

//
// build_request (single record)
//

#[test]
fn single_record_has_one_resource_logs_scope_logs_log_record() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None);

    let request = build_request(&logger, &rec).expect("build_request");

    assert_eq!(request.resource_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs[0].log_records.len(), 1);
}

#[test]
fn single_record_injects_service_name_in_resource() {
    let logger = test_logger("my-svc");
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None);

    let request = build_request(&logger, &rec).expect("build_request");

    let attributes = &request.resource_logs[0].resource.as_ref().expect("resource").attributes;
    let svc = attributes.iter().find(|kv| kv.key == "service.name").expect("service.name attribute");
    match svc.value.as_ref().and_then(|v| v.value.as_ref()) {
        Some(Value::StringValue(name)) => assert_eq!(name, "my-svc"),
        other => panic!("unexpected value: {other:?}"),
    }
}

#[test]
fn single_record_default_scope_name() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None);

    let request = build_request(&logger, &rec).expect("build_request");

    let scope = request.resource_logs[0].scope_logs[0].scope.as_ref().expect("scope");
    assert_eq!(scope.name, "liblogjet");
}

#[test]
fn single_record_explicit_scope_name() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let scope_name = CString::new("my-scope").unwrap();
    let mut rec = record(1, &body, None, None);
    rec.scope_name = scope_name.as_ptr();

    let request = build_request(&logger, &rec).expect("build_request");

    let scope = request.resource_logs[0].scope_logs[0].scope.as_ref().expect("scope");
    assert_eq!(scope.name, "my-scope");
}

#[test]
fn single_record_missing_body_is_error() {
    let logger = test_logger("svc");
    let mut rec = record(1, c"x", None, None);
    rec.body = ptr::null();

    assert!(build_request(&logger, &rec).is_err());
}

#[test]
fn single_record_zero_timestamp_is_replaced_with_now() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();
    let rec = record(0, &body, None, None);

    let request = build_request(&logger, &rec).expect("build_request");

    let ts = request.resource_logs[0].scope_logs[0].log_records[0].time_unix_nano;
    assert!(ts > 0);
}

//
// build_batch_request: resource / scope attribute partitioning
//

#[test]
fn differing_resource_attrs_split_resources() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();

    let key_a = CString::new("env").unwrap();
    let val_a = CString::new("prod").unwrap();
    let key_b = CString::new("env").unwrap();
    let val_b = CString::new("staging").unwrap();

    let attrs_a = [LjAttribute { key: key_a.as_ptr(), value: val_a.as_ptr(), value_type: LJ_ATTR_STRING }];
    let attrs_b = [LjAttribute { key: key_b.as_ptr(), value: val_b.as_ptr(), value_type: LJ_ATTR_STRING }];

    let mut rec_a = record(1, &body, None, None);
    rec_a.resource_attrs = attrs_a.as_ptr();
    rec_a.resource_attrs_len = attrs_a.len();

    let mut rec_b = record(2, &body, None, None);
    rec_b.resource_attrs = attrs_b.as_ptr();
    rec_b.resource_attrs_len = attrs_b.len();

    let request = build_batch_request(&logger, &[rec_a, rec_b]).expect("batch request");
    assert_eq!(request.resource_logs.len(), 2);
}

#[test]
fn differing_scope_attrs_split_scopes() {
    let logger = test_logger("svc");
    let body = CString::new("hello").unwrap();

    let key_a = CString::new("library").unwrap();
    let val_a = CString::new("fast").unwrap();
    let key_b = CString::new("library").unwrap();
    let val_b = CString::new("slow").unwrap();

    let attrs_a = [LjAttribute { key: key_a.as_ptr(), value: val_a.as_ptr(), value_type: LJ_ATTR_STRING }];
    let attrs_b = [LjAttribute { key: key_b.as_ptr(), value: val_b.as_ptr(), value_type: LJ_ATTR_STRING }];

    let mut rec_a = record(1, &body, None, None);
    rec_a.scope_attrs = attrs_a.as_ptr();
    rec_a.scope_attrs_len = attrs_a.len();

    let mut rec_b = record(2, &body, None, None);
    rec_b.scope_attrs = attrs_b.as_ptr();
    rec_b.scope_attrs_len = attrs_b.len();

    let request = build_batch_request(&logger, &[rec_a, rec_b]).expect("batch request");
    assert_eq!(request.resource_logs.len(), 1);
    assert_eq!(request.resource_logs[0].scope_logs.len(), 2);
}

#[test]
fn mixed_services_and_scopes_form_independent_groups() {
    let logger = test_logger("default-svc");
    let body = CString::new("hello").unwrap();
    let svc_a = CString::new("svc-a").unwrap();
    let svc_b = CString::new("svc-b").unwrap();
    let scp_a = CString::new("scope-a").unwrap();
    let scp_b = CString::new("scope-b").unwrap();

    // 2 services × 2 scopes = 4 groupings
    let records = vec![
        record(1, &body, Some(&svc_a), Some(&scp_a)),
        record(2, &body, Some(&svc_a), Some(&scp_b)),
        record(3, &body, Some(&svc_b), Some(&scp_a)),
        record(4, &body, Some(&svc_b), Some(&scp_b)),
    ];

    let request = build_batch_request(&logger, &records).expect("batch request");
    assert_eq!(request.resource_logs.len(), 2);

    let total_scopes: usize = request.resource_logs.iter().map(|rl| rl.scope_logs.len()).sum();
    assert_eq!(total_scopes, 4);

    let total_logs: usize = request.resource_logs.iter()
        .flat_map(|rl| rl.scope_logs.iter())
        .map(|sl| sl.log_records.len())
        .sum();
    assert_eq!(total_logs, 4);
}

//
// read_attrs / triples_to_kvs
//

#[test]
fn read_attrs_null_pointer_returns_empty() {
    let result = read_attrs(ptr::null(), 3).unwrap();
    assert!(result.is_empty());
}

#[test]
fn read_attrs_zero_len_returns_empty() {
    let attr = LjAttribute { key: c"k".as_ptr(), value: c"v".as_ptr(), value_type: LJ_ATTR_STRING };
    let result = read_attrs(&attr, 0).unwrap();
    assert!(result.is_empty());
}

#[test]
fn read_attrs_string_type() {
    let attrs = [
        LjAttribute { key: c"key".as_ptr(), value: c"val".as_ptr(), value_type: LJ_ATTR_STRING },
    ];
    let result = read_attrs(attrs.as_ptr(), attrs.len()).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], ("key".to_string(), LJ_ATTR_STRING, "val".to_string()));
}

#[test]
fn read_attrs_int_type() {
    let attrs = [
        LjAttribute { key: c"count".as_ptr(), value: c"42".as_ptr(), value_type: LJ_ATTR_INT },
    ];
    let result = read_attrs(attrs.as_ptr(), attrs.len()).unwrap();
    assert_eq!(result[0], ("count".to_string(), LJ_ATTR_INT, "42".to_string()));
}

#[test]
fn read_attrs_array_type() {
    let attrs = [
        LjAttribute { key: c"tags".as_ptr(), value: c"a, b, c".as_ptr(), value_type: LJ_ATTR_ARRAY },
    ];
    let result = read_attrs(attrs.as_ptr(), attrs.len()).unwrap();
    assert_eq!(result[0], ("tags".to_string(), LJ_ATTR_ARRAY, "a, b, c".to_string()));
}

#[test]
fn read_attrs_null_key_is_error() {
    let attrs = [
        LjAttribute { key: ptr::null(), value: c"v".as_ptr(), value_type: LJ_ATTR_STRING },
    ];
    assert!(read_attrs(attrs.as_ptr(), attrs.len()).is_err());
}

#[test]
fn triples_to_kvs_string_value() {
    let triples = vec![("key".to_string(), LJ_ATTR_STRING, "val".to_string())];
    let kvs = triples_to_kvs(&triples).unwrap();
    assert_eq!(kvs[0].key, "key");
    if let Some(ref v) = kvs[0].value {
        if let Some(Value::StringValue(s)) = &v.value {
            assert_eq!(s, "val");
            return;
        }
    }
    panic!("unexpected value");
}

#[test]
fn triples_to_kvs_int_value() {
    let triples = vec![("count".to_string(), LJ_ATTR_INT, "42".to_string())];
    let kvs = triples_to_kvs(&triples).unwrap();
    if let Some(ref v) = kvs[0].value {
        if let Some(Value::IntValue(i)) = &v.value {
            assert_eq!(*i, 42);
            return;
        }
    }
    panic!("unexpected value");
}

#[test]
fn triples_to_kvs_array_value() {
    let triples = vec![("tags".to_string(), LJ_ATTR_ARRAY, "a, b, c".to_string())];
    let kvs = triples_to_kvs(&triples).unwrap();
    if let Some(ref v) = kvs[0].value {
        if let Some(Value::ArrayValue(arr)) = &v.value {
            let items: Vec<&str> = arr.values.iter().filter_map(|av| match &av.value {
                Some(Value::StringValue(s)) => Some(s.as_str()),
                _ => None,
            }).collect();
            assert_eq!(items, vec!["a", "b", "c"]);
            return;
        }
    }
    panic!("unexpected value");
}

#[test]
fn triples_to_kvs_unknown_type_is_error() {
    let triples = vec![("key".to_string(), 99, "val".to_string())];
    assert!(triples_to_kvs(&triples).is_err());
}

//
// resolve_resource / resolve_scope
//

#[test]
fn resolve_resource_falls_back_to_logger_service_name() {
    let logger = test_logger("default-svc");
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None); // service_name = null

    let (key, attrs) = resolve_resource(&logger, &rec).unwrap();
    assert_eq!(key.0, "default-svc");
    let svc = attrs.iter().find(|kv| kv.key == "service.name").expect("service.name");
    if let Some(ref v) = svc.value {
        if let Some(Value::StringValue(s)) = &v.value {
            assert_eq!(s, "default-svc");
            return;
        }
    }
    panic!("unexpected service.name value");
}

#[test]
fn resolve_resource_empty_service_name_is_error() {
    let mut logger = test_logger("");
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None);

    // Records with null service_name that resolve to "" logger default.
    logger.service_name = String::new();
    assert!(resolve_resource(&logger, &rec).is_err());
}

#[test]
fn resolve_scope_defaults_to_liblogjet() {
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None); // scope_name = null

    let (key, scope) = resolve_scope(&rec).unwrap();
    assert_eq!(key.0, "liblogjet");
    assert_eq!(scope.name, "liblogjet");
}

//
// record_to_log
//

#[test]
fn record_to_log_defaults_severity_text_from_number() {
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None); // severity_text = null, severity_number = 9 = INFO

    let log = record_to_log(&rec).unwrap();
    assert_eq!(log.severity_text, "INFO");
}

#[test]
fn record_to_log_respects_explicit_severity_text() {
    let body = CString::new("hello").unwrap();
    let sev = CString::new("WARN").unwrap();
    let mut rec = record(1, &body, None, None);
    rec.severity_text = sev.as_ptr();

    let log = record_to_log(&rec).unwrap();
    assert_eq!(log.severity_text, "WARN");
}

#[test]
fn record_to_log_defaults_event_name_to_empty() {
    let body = CString::new("hello").unwrap();
    let rec = record(1, &body, None, None); // event_name = null

    let log = record_to_log(&rec).unwrap();
    assert_eq!(log.event_name, "");
}

#[test]
fn record_to_log_zero_timestamp_replaced_with_now() {
    let body = CString::new("hello").unwrap();
    let rec = record(0, &body, None, None);

    let log = record_to_log(&rec).unwrap();
    assert!(log.time_unix_nano > 0);
}

//
// severity_text
//

#[test]
fn severity_text_maps_correctly() {
    assert_eq!(severity_text(1), "TRACE");
    assert_eq!(severity_text(4), "TRACE");
    assert_eq!(severity_text(5), "DEBUG");
    assert_eq!(severity_text(9), "INFO");
    assert_eq!(severity_text(13), "WARN");
    assert_eq!(severity_text(17), "ERROR");
    assert_eq!(severity_text(21), "FATAL");
}

//
// parse_http_endpoint / normalise_grpc_endpoint
//

#[test]
fn parse_http_host_port_defaults_path() {
    let ep = parse_http_endpoint("127.0.0.1:4318").unwrap();
    assert_eq!(ep.authority, "127.0.0.1:4318");
    assert_eq!(ep.host_header, "127.0.0.1:4318");
    assert_eq!(ep.path, "/v1/logs");
}

#[test]
fn parse_http_with_custom_path() {
    let ep = parse_http_endpoint("127.0.0.1:4318/custom/path").unwrap();
    assert_eq!(ep.authority, "127.0.0.1:4318");
    assert_eq!(ep.path, "/custom/path");
}

#[test]
fn parse_http_scheme_is_stripped() {
    let ep = parse_http_endpoint("http://127.0.0.1:4318").unwrap();
    assert_eq!(ep.authority, "127.0.0.1:4318");
    assert_eq!(ep.path, "/v1/logs");
}

#[test]
fn parse_http_with_scheme_and_path() {
    let ep = parse_http_endpoint("http://127.0.0.1:4318/p").unwrap();
    assert_eq!(ep.authority, "127.0.0.1:4318");
    assert_eq!(ep.path, "/p");
}

#[test]
fn parse_https_is_rejected() {
    assert!(parse_http_endpoint("https://127.0.0.1:4318").is_err());
}

#[test]
fn parse_http_empty_is_error() {
    assert!(parse_http_endpoint("").is_err());
}

#[test]
fn normalise_grpc_adds_http_scheme() {
    assert_eq!(normalise_grpc_endpoint("127.0.0.1:4317"), "http://127.0.0.1:4317");
}

#[test]
fn normalise_grpc_preserves_existing_scheme() {
    assert_eq!(normalise_grpc_endpoint("http://127.0.0.1:4317"), "http://127.0.0.1:4317");
}
