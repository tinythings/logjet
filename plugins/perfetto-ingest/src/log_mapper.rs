//! Maps Perfetto stats/errors to OTel log records.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::sqlite_reader::{PerfettoDb, PerfettoSchedSlice, PerfettoSlice};
use crate::timestamp::TimestampConverter;

const SEVERITY_INFO: i32 = 9;
const SLICES_PER_LOG_BATCH: usize = 1;

pub fn map_logs(
    db: &PerfettoDb,
    converter: &TimestampConverter,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    // Emit a per-slice log for each slice (readable in ljx view).
    let slices = db.read_slices()?;
    let mut batch: Vec<LogRecord> = Vec::with_capacity(SLICES_PER_LOG_BATCH);
    let mut batch_min_ts: u64 = 0;

    for slice in &slices {
        let ts = converter.to_realtime(slice.ts).ok().flatten().unwrap_or(0);

        if batch.is_empty() || ts < batch_min_ts {
            batch_min_ts = ts;
        }

        batch.push(slice_to_log(slice, converter));

        if batch.len() >= SLICES_PER_LOG_BATCH {
            flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);
        }
    }
    flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);

    // Emit sched_slice entries as log records.
    let sched_slices = db.read_sched_slices()?;
    for s in &sched_slices {
        let ts = converter.to_realtime(s.ts).ok().flatten().unwrap_or(0);
        if batch.is_empty() || ts < batch_min_ts {
            batch_min_ts = ts;
        }
        batch.push(sched_slice_to_log(s, converter));
        if batch.len() >= SLICES_PER_LOG_BATCH {
            flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);
        }
    }
    flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);

    // Emit a summary log.
    let threads = db.read_threads()?;
    let processes = db.read_processes()?;
    emit_summary(slices.len(), threads.len(), processes.len(), emit, plugin);

    Ok(())
}

fn slice_to_log(slice: &PerfettoSlice, converter: &TimestampConverter) -> LogRecord {
    let ts = converter.to_realtime(slice.ts).ok().flatten().unwrap_or(0);
    let dur_us = slice.dur as f64 / 1000.0;
    let name = slice.name.as_deref().unwrap_or("(unnamed)");

    let body = format!("{name}  dur={dur_us:.1}us  depth={}", slice.depth);

    LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue {
                key: "perfetto.slice.id".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(slice.id)) }),
            },
            KeyValue {
                key: "perfetto.slice.name".to_string(),
                value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }),
            },
            KeyValue {
                key: "perfetto.slice.dur_ns".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(slice.dur)) }),
            },
            KeyValue {
                key: "perfetto.slice.depth".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(slice.depth as i64)) }),
            },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    }
}

fn sched_slice_to_log(s: &PerfettoSchedSlice, converter: &TimestampConverter) -> LogRecord {
    let ts = converter.to_realtime(s.ts).ok().flatten().unwrap_or(0);
    let end = s.end_state.as_deref().unwrap_or("?");
    let dur_ns = s.dur as u64;
    let dur_us = dur_ns as f64 / 1000.0;
    let body = format!("cpu={} state={end} utid={}  dur={dur_us:.1}us", s.cpu, s.utid);

    LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue {
                key: "perfetto.sched.id".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(s.id)) }),
            },
            KeyValue {
                key: "perfetto.sched.cpu".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(s.cpu)) }),
            },
            KeyValue {
                key: "perfetto.sched.dur_ns".to_string(),
                value: Some(AnyValue { value: Some(Value::IntValue(s.dur)) }),
            },
            KeyValue {
                key: "perfetto.sched.end_state".to_string(),
                value: Some(AnyValue { value: Some(Value::StringValue(end.to_string())) }),
            },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    }
}

fn flush_log_batch(
    batch: &mut Vec<LogRecord>,
    batch_min_ts: &mut u64,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) {
    if batch.is_empty() {
        return;
    }

    let records = std::mem::replace(batch, Vec::with_capacity(SLICES_PER_LOG_BATCH));
    let ts = *batch_min_ts;
    *batch_min_ts = 0;

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("perfetto".to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "perfetto-ingest".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let payload = request.encode_to_vec();
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_LOGS, ts, &payload) };
}

fn emit_summary(
    count_slices: usize,
    count_threads: usize,
    count_processes: usize,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) {
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

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue { value: Some(Value::StringValue("perfetto".to_string())) }),
                }],
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: "perfetto-ingest".to_string(),
                    version: String::new(),
                    attributes: Vec::new(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![record],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let payload = request.encode_to_vec();
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_LOGS, now_ns, &payload) };
}
