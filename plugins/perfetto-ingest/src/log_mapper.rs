//! Maps Perfetto stats/errors to OTel log records.

#![allow(dead_code)]

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::sqlite_reader::PerfettoDb;

/// OTel severity numbers mapped from Perfetto contexts.
const SEVERITY_INFO: i32 = 9;
const SEVERITY_ERROR: i32 = 17;

pub fn map_logs(
    db: &PerfettoDb,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    let slices = db.read_slices()?;
    let threads = db.read_threads()?;
    let processes = db.read_processes()?;

    let count_slices = slices.len();
    let count_threads = threads.len();
    let count_processes = processes.len();

    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let body = format!(
        "Perfetto trace analysis complete: {} slices, {} threads, {} processes",
        count_slices, count_threads, count_processes
    );

    let record = LogRecord {
        time_unix_nano: now_ns,
        observed_time_unix_nano: now_ns,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue {
                key: "perfetto.slices".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(count_slices as i64)) }),
            },
            KeyValue {
                key: "perfetto.threads".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(count_threads as i64)) }),
            },
            KeyValue {
                key: "perfetto.processes".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(count_processes as i64)) }),
            },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    };

    let resource = Resource {
        attributes: vec![KeyValue {
            key: "service.name".to_string(),
            value: Some(AnyValue { value: Some(Value::StringValue("perfetto".to_string())) }),
        }],
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let scope = InstrumentationScope {
        name: "perfetto-ingest".to_string(),
        version: String::new(),
        attributes: Vec::new(),
        dropped_attributes_count: 0,
    };

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource),
            scope_logs: vec![ScopeLogs {
                scope: Some(scope),
                log_records: vec![record],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let payload = request.encode_to_vec();
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_LOGS, now_ns, &payload) };

    Ok(())
}
