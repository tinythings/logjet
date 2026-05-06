//! Maps Perfetto slice/flow/process/thread data to OTel spans.
//!
//! Reads slices from the exported SQLite DB, joins with track/thread/process
//! metadata, builds OTel spans with proper IDs and attributes, encodes them as
//! `ExportTraceServiceRequest` protobuf batches, and streams them through the
//! generic record callback.

#![allow(dead_code)]

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use opentelemetry_proto::tonic::trace::v1::status::StatusCode;
use prost::Message;

use crate::sqlite_reader::{PerfettoDb, PerfettoSlice};
use crate::timestamp::TimestampConverter;

/// Maximum spans per OTLP export batch.
const SPANS_PER_BATCH: usize = 200;

/// Context gathered from the DB for span construction.
struct TraceContext {
    /// Thread name by utid.
    thread_names: HashMap<i64, String>,
    /// Process name by upid.
    process_names: HashMap<i64, String>,
    /// upid for each thread utid.
    thread_process: HashMap<i64, i64>,
}

impl TraceContext {
    fn build(db: &PerfettoDb) -> Result<Self, String> {
        let threads = db.read_threads()?;
        let _tracks = db.read_tracks()?;
        let processes = db.read_processes()?;

        let thread_names: HashMap<i64, String> = threads
            .iter()
            .filter_map(|t| t.name.clone().map(|n| (t.utid, n)))
            .collect();
        let process_names: HashMap<i64, String> = processes
            .iter()
            .filter_map(|p| p.name.clone().map(|n| (p.upid, n)))
            .collect();
        let thread_process: HashMap<i64, i64> = threads.iter().filter_map(|t| t.upid.map(|u| (t.utid, u))).collect();

        Ok(Self { thread_names, process_names, thread_process })
    }
}

/// Maps all slices in the DB to OTel spans and streams them through `emit`.
pub fn map_traces(
    db: &PerfettoDb,
    converter: &TimestampConverter,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    let ctx = TraceContext::build(db)?;
    let slices = db.read_slices()?;

    if slices.is_empty() {
        return Ok(());
    }

    // Build a constant trace_id for this trace file.
    let trace_id = make_trace_id();

    let mut batch: Vec<Span> = Vec::with_capacity(SPANS_PER_BATCH);
    let mut batch_min_ts: Option<u64> = None;

    for slice in &slices {
        let span = build_span(slice, &trace_id, &ctx, converter)?;
        let span_ts = match converter.to_realtime(slice.ts)? {
            Some(ts) => ts,
            None => continue, // BestEffort: skip spans without realtime
        };

        if batch_min_ts.is_none() || span_ts < batch_min_ts.unwrap() {
            batch_min_ts = Some(span_ts);
        }

        batch.push(span);

        if batch.len() >= SPANS_PER_BATCH {
            flush_batch(&mut batch, &mut batch_min_ts, &trace_id, emit, plugin)?;
        }
    }

    flush_batch(&mut batch, &mut batch_min_ts, &trace_id, emit, plugin)?;
    Ok(())
}

fn build_span(
    slice: &PerfettoSlice,
    trace_id: &[u8; 16],
    _ctx: &TraceContext,
    converter: &TimestampConverter,
) -> Result<Span, String> {
    let start_time = converter.to_realtime(slice.ts)?.unwrap_or(0);
    let end_time = converter.to_realtime(slice.ts.saturating_add(slice.dur))?.unwrap_or_else(|| start_time.saturating_add(slice.dur.max(0) as u64));

    let span_id = make_span_id(slice.id);
    let parent_span_id = slice.parent_id.map(make_span_id).unwrap_or_default();

    let name = slice.name.clone().unwrap_or_else(|| format!("slice-{}", slice.id));

    let mut attrs = vec![
        key_value("perfetto.slice.id", any_string(&slice.id.to_string())),
        key_value("perfetto.slice.ts", any_string(&slice.ts.to_string())),
        key_value("perfetto.slice.dur_ns", any_string(&slice.dur.to_string())),
        key_value("perfetto.slice.track_id", any_string(&slice.track_id.to_string())),
    ];

    // Attach thread/process context via track_id lookup.
    // We don't have track_id → utid mapping directly in slices, but we can add
    // a note. For now, attach the slice depth.
    attrs.push(key_value("perfetto.slice.depth", any_int(slice.depth as i64)));

    let status = Status { code: StatusCode::Unset as i32, message: String::new() };

    Ok(Span {
        trace_id: trace_id.to_vec(),
        span_id: span_id.to_vec(),
        trace_state: String::new(),
        parent_span_id: parent_span_id.to_vec(),
        name,
        kind: SpanKind::Internal as i32,
        start_time_unix_nano: start_time,
        end_time_unix_nano: end_time,
        attributes: attrs,
        dropped_attributes_count: 0,
        events: Vec::new(),
        dropped_events_count: 0,
        links: Vec::new(),
        dropped_links_count: 0,
        status: Some(status),
        flags: 0,
    })
}

fn flush_batch(
    batch: &mut Vec<Span>,
    batch_min_ts: &mut Option<u64>,
    _trace_id: &[u8; 16],
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]),
    plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }

    let spans = std::mem::replace(batch, Vec::with_capacity(SPANS_PER_BATCH));
    let ts = batch_min_ts.unwrap_or(0);
    *batch_min_ts = None;

    let resource = Resource {
        attributes: vec![
            key_value("service.name", any_string("perfetto")),
        ],
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let scope_spans = ScopeSpans {
        scope: Some(opentelemetry_proto::tonic::common::v1::InstrumentationScope {
            name: "perfetto-ingest".to_string(),
            version: String::new(),
            attributes: Vec::new(),
            dropped_attributes_count: 0,
        }),
        spans,
        schema_url: String::new(),
    };

    let request = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource),
            scope_spans: vec![scope_spans],
            schema_url: String::new(),
        }],
    };

    let payload = request.encode_to_vec();
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_TRACES, ts, &payload) };

    Ok(())
}

// ID generation

fn make_trace_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    id[..8].copy_from_slice(&ts.to_le_bytes());
    id[8..16].copy_from_slice(&(std::process::id() as u64).to_le_bytes());
    id
}

fn make_span_id(slice_id: i64) -> [u8; 8] {
    let mut id = [0u8; 8];
    id.copy_from_slice(&slice_id.to_le_bytes());
    id
}

// Attribute helpers

fn key_value(key: &str, value: AnyValue) -> KeyValue {
    KeyValue { key: key.to_string(), value: Some(value) }
}

fn any_string(s: &str) -> AnyValue {
    AnyValue { value: Some(Value::StringValue(s.to_string())) }
}

fn any_int(v: i64) -> AnyValue {
    AnyValue { value: Some(Value::IntValue(v)) }
}
