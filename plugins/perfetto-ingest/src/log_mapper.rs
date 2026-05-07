//! Maps Perfetto stats/errors to OTel log records.

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::sqlite_reader::{PerfettoDb, PerfettoFtraceEvent, PerfettoSchedSlice, PerfettoSlice, PerfettoSpuriousWakeup, PerfettoThreadState};
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

    // Emit thread_state entries as log records.
    let thread_states = db.read_thread_states()?;
    for ts_state in &thread_states {
        let ts = converter.to_realtime(ts_state.ts).ok().flatten().unwrap_or(0);
        if batch.is_empty() || ts < batch_min_ts {
            batch_min_ts = ts;
        }
        batch.push(thread_state_to_log(ts_state, converter));
        if batch.len() >= SLICES_PER_LOG_BATCH {
            flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);
        }
    }
    flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);

    // Emit ftrace_event entries as log records.
    let ftrace_events = db.read_ftrace_events()?;
    for ev in &ftrace_events {
        let ts = converter.to_realtime(ev.ts).ok().flatten().unwrap_or(0);
        if batch.is_empty() || ts < batch_min_ts {
            batch_min_ts = ts;
        }
        batch.push(ftrace_event_to_log(ev, converter));
        if batch.len() >= SLICES_PER_LOG_BATCH {
            flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);
        }
    }
    flush_log_batch(&mut batch, &mut batch_min_ts, emit, plugin);

    // Emit spurious_wakeup entries as log records.
    let wakeups = db.read_spurious_wakeups()?;
    for w in &wakeups {
        let ts = converter.to_realtime(w.ts).ok().flatten().unwrap_or(0);
        if batch.is_empty() || ts < batch_min_ts {
            batch_min_ts = ts;
        }
        batch.push(spurious_wakeup_to_log(w, converter));
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

fn thread_state_to_log(ts: &PerfettoThreadState, converter: &TimestampConverter) -> LogRecord {
    let t = converter.to_realtime(ts.ts).ok().flatten().unwrap_or(0);
    let state = ts.state.as_deref().unwrap_or("?");
    let dur_us = ts.dur as f64 / 1000.0;
    let mut body = format!("state={state} dur={dur_us:.1}us utid={}", ts.utid);
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
        attrs.push(KeyValue { key: "perfetto.ts.blocked_function".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(blocked.clone())) }) });
    }

    LogRecord {
        time_unix_nano: t, observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO, severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0, flags: 0,
        trace_id: Vec::new(), span_id: Vec::new(), event_name: String::new(),
    }
}

fn ftrace_event_to_log(ev: &PerfettoFtraceEvent, converter: &TimestampConverter) -> LogRecord {
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
    LogRecord {
        time_unix_nano: t, observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO, severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0, flags: 0,
        trace_id: Vec::new(), span_id: Vec::new(), event_name: String::new(),
    }
}

fn spurious_wakeup_to_log(w: &PerfettoSpuriousWakeup, converter: &TimestampConverter) -> LogRecord {
    let t = converter.to_realtime(w.ts).ok().flatten().unwrap_or(0);
    let body = format!("spurious_wakeup utid={}", w.utid.unwrap_or(-1));
    let mut attrs = vec![
        KeyValue { key: "perfetto.sw.id".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(w.id)) }) },
    ];
    if let Some(utid) = w.utid {
        attrs.push(KeyValue { key: "perfetto.sw.utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(utid)) }) });
    }
    if let Some(waker) = w.waker_utid {
        attrs.push(KeyValue { key: "perfetto.sw.waker_utid".to_string(), value: Some(AnyValue { value: Some(Value::IntValue(waker)) }) });
    }
    LogRecord {
        time_unix_nano: t, observed_time_unix_nano: t,
        severity_number: SEVERITY_INFO, severity_text: "INFO".to_string(),
        body: Some(AnyValue { value: Some(Value::StringValue(body)) }),
        attributes: attrs,
        dropped_attributes_count: 0, flags: 0,
        trace_id: Vec::new(), span_id: Vec::new(), event_name: String::new(),
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
