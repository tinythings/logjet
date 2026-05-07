//! Maps Perfetto stats/errors to OTel log records.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::sqlite_reader::{
    PerfettoDb, PerfettoFtraceEvent, PerfettoInstant, PerfettoSchedSlice, PerfettoSlice, PerfettoSpuriousWakeup, PerfettoThreadState,
};
use crate::timestamp::TimestampConverter;

const SEVERITY_INFO: i32 = 9;

fn dur_str(ns: i64) -> String {
    if ns < 0 { "running".to_string() } else { format!("{:.1}us", ns as f64 / 1000.0) }
}

pub fn map_logs(
    db: &PerfettoDb, converter: &TimestampConverter, emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    let mut all: Vec<LogRecord> = Vec::new();

    for slice in &db.read_slices()? {
        if let Some(rec) = maybe_slice_to_log(slice, converter) {
            all.push(rec);
        }
    }
    for s in &db.read_sched_slices()? {
        if let Some(rec) = maybe_sched_slice_to_log(s, converter) {
            all.push(rec);
        }
    }
    for ts in &db.read_thread_states()? {
        if let Some(rec) = maybe_thread_state_to_log(ts, converter) {
            all.push(rec);
        }
    }
    for ev in &db.read_ftrace_events()? {
        if let Some(rec) = maybe_ftrace_event_to_log(ev, converter) {
            all.push(rec);
        }
    }
    for w in &db.read_spurious_wakeups()? {
        if let Some(rec) = maybe_spurious_wakeup_to_log(w, converter) {
            all.push(rec);
        }
    }
    for inst in &db.read_instants()? {
        if let Some(rec) = maybe_instant_to_log(inst, converter) {
            all.push(rec);
        }
    }

    all.sort_by_key(|r| r.time_unix_nano);

    for rec in &all {
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
                    log_records: vec![rec.clone()],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        let payload = request.encode_to_vec();
        unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_LOGS, rec.time_unix_nano, &payload) };
    }

    let threads = db.read_threads()?;
    let processes = db.read_processes()?;
    let max_ts = all.last().map(|r| r.time_unix_nano.saturating_add(1)).unwrap_or(0);
    emit_summary(db.read_slices()?.len(), threads.len(), processes.len(), emit, plugin, max_ts);

    Ok(())
}

fn maybe_slice_to_log(slice: &PerfettoSlice, converter: &TimestampConverter) -> Option<LogRecord> {
    let ts = converter.to_realtime(slice.ts).ok().flatten().unwrap_or(0);
    let dur = dur_str(slice.dur);
    let name = slice.name.as_deref().unwrap_or("(unnamed)");
    let body = format!("{name}  dur={dur}  depth={}", slice.depth);
    Some(LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue { key: "perfetto.slice.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(slice.id)) }) },
            KeyValue { key: "perfetto.slice.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) },
            KeyValue { key: "perfetto.slice.dur_ns".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(slice.dur)) }) },
            KeyValue { key: "perfetto.slice.depth".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(slice.depth as i64)) }) },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn maybe_sched_slice_to_log(s: &PerfettoSchedSlice, converter: &TimestampConverter) -> Option<LogRecord> {
    let ts = converter.to_realtime(s.ts).ok().flatten().unwrap_or(0);
    let end = s.end_state.as_deref().unwrap_or("?");
    let dur = dur_str(s.dur);
    let body = format!("cpu={} state={end} utid={}  dur={dur}", s.cpu, s.utid);
    Some(LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue { key: "perfetto.sched.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(s.id)) }) },
            KeyValue { key: "perfetto.sched.cpu".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(s.cpu)) }) },
            KeyValue { key: "perfetto.sched.dur_ns".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(s.dur)) }) },
            KeyValue { key: "perfetto.sched.end_state".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(end.to_string())) }) },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn maybe_thread_state_to_log(ts: &PerfettoThreadState, converter: &TimestampConverter) -> Option<LogRecord> {
    let t = converter.to_realtime(ts.ts).ok().flatten().unwrap_or(0);
    let state = ts.state.as_deref().unwrap_or("?");
    let dur = dur_str(ts.dur);
    let mut body = format!("state={state} dur={dur} utid={}", ts.utid);
    if let Some(cpu) = ts.cpu {
        body.push_str(&format!(" cpu={cpu}"));
    }
    if ts.io_wait == Some(true) {
        body.push_str(" io_wait");
    }
    if let Some(ref blocked) = ts.blocked_function {
        body.push_str(&format!(" blocked={blocked}"));
    }
    if let Some(waker) = ts.waker_utid {
        body.push_str(&format!(" waker={waker}"));
    }

    let mut attrs = vec![
        KeyValue { key: "perfetto.ts.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(ts.id)) }) },
        KeyValue { key: "perfetto.ts.state".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(state.to_string())) }) },
        KeyValue { key: "perfetto.ts.dur_ns".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(ts.dur)) }) },
        KeyValue { key: "perfetto.ts.utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(ts.utid)) }) },
    ];
    if let Some(cpu) = ts.cpu {
        attrs.push(KeyValue { key: "perfetto.ts.cpu".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(cpu)) }) });
    }
    if let Some(io) = ts.io_wait {
        attrs.push(KeyValue { key: "perfetto.ts.io_wait".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(i64::from(io))) }) });
    }
    if let Some(ref blocked) = ts.blocked_function {
        attrs.push(KeyValue {
            key: "perfetto.ts.blocked_function".to_string(),
            value: Some(AnyValue { value: Some(Value::StringValue(blocked.clone())) }),
        });
    }

    Some(LogRecord {
        time_unix_nano: t,
        observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn maybe_ftrace_event_to_log(ev: &PerfettoFtraceEvent, converter: &TimestampConverter) -> Option<LogRecord> {
    let t = converter.to_realtime(ev.ts).ok().flatten().unwrap_or(0);
    let name = ev.name.as_deref().unwrap_or("?");
    let cpu = ev.cpu.unwrap_or(-1);
    let body = format!("{name} cpu={cpu}");
    let mut attrs = vec![
        KeyValue { key: "perfetto.ftrace.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(ev.id)) }) },
        KeyValue { key: "perfetto.ftrace.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) },
        KeyValue { key: "perfetto.ftrace.cpu".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(cpu)) }) },
    ];
    if let Some(utid) = ev.utid {
        attrs.push(KeyValue { key: "perfetto.ftrace.utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(utid)) }) });
    }
    Some(LogRecord {
        time_unix_nano: t,
        observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn maybe_spurious_wakeup_to_log(w: &PerfettoSpuriousWakeup, converter: &TimestampConverter) -> Option<LogRecord> {
    let t = converter.to_realtime(w.ts).ok().flatten().unwrap_or(0);
    let body = format!("spurious_wakeup utid={}", w.utid.unwrap_or(-1));
    let mut attrs = vec![KeyValue { key: "perfetto.sw.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(w.id)) }) }];
    if let Some(utid) = w.utid {
        attrs.push(KeyValue { key: "perfetto.sw.utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(utid)) }) });
    }
    if let Some(waker) = w.waker_utid {
        attrs.push(KeyValue { key: "perfetto.sw.waker_utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(waker)) }) });
    }
    Some(LogRecord {
        time_unix_nano: t,
        observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn maybe_instant_to_log(inst: &PerfettoInstant, converter: &TimestampConverter) -> Option<LogRecord> {
    let t = converter.to_realtime(inst.ts).ok().flatten().unwrap_or(0);
    let name = inst.name.as_deref().unwrap_or("?");
    let body = name.to_string();
    Some(LogRecord {
        time_unix_nano: t,
        observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue { key: "perfetto.instant.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) },
            KeyValue { key: "perfetto.instant.track_id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(inst.track_id)) }) },
        ],
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: Vec::new(),
        span_id: Vec::new(),
        event_name: String::new(),
    })
}

fn emit_summary(
    count_slices: usize, count_threads: usize, count_processes: usize,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]), plugin: &crate::PerfettoPlugin, ts: u64,
) {
    let body = format!("Perfetto trace analysis complete: {} slices, {} threads, {} processes", count_slices, count_threads, count_processes);
    let record = LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: vec![
            KeyValue { key: "perfetto.slices".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(count_slices as i64)) }) },
            KeyValue { key: "perfetto.threads".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(count_threads as i64)) }) },
            KeyValue { key: "perfetto.processes".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(count_processes as i64)) }) },
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
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_LOGS, ts, &payload) };
}
