//! Stage 5: reconstruct OTLP batches from dedup groups, write .logjet.
//!
//! Each `DedupGroup` emits one OTLP `LogRecord` with dedup attributes
//! injected. Non-log passthrough records are written unchanged.

use std::io::Write;

use logjet::{LogjetWriter, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::dedup::{DedupGroup, DedupStats};
use crate::error::Result;

/// Write dedup groups + passthrough records to a .logjet file.
pub fn write<W: Write>(
    writer: &mut LogjetWriter<W>,
    groups: &[DedupGroup],
    passthrough: &[OwnedRecord],
    mode_label: &str,
) -> Result<DedupStats> {
    let total_records: u64 = groups.iter().map(|g| g.count).sum();
    let group_count = groups.len() as u64;

    // Build (ts, record_type, payload) tuples for all output records, sorted
    // by timestamp so the output is chronologically ordered. Fresh monotonic
    // seq avoids the writer's "sequence must be monotonic within block" rule.
    let mut emit_records: Vec<(u64, RecordType, Vec<u8>)> =
        Vec::with_capacity(groups.len() + passthrough.len());

    for group in groups {
        let payload = build_otlp_payload(group, mode_label);
        emit_records.push((group.first_seen_ns, RecordType::Logs, payload));
    }
    for rec in passthrough {
        emit_records.push((rec.ts_unix_ns, rec.record_type, rec.payload.clone()));
    }

    // Sort by timestamp for chronological output.
    emit_records.sort_by_key(|(ts, _, _)| *ts);

    for (seq, (ts, rt, payload)) in emit_records.iter().enumerate() {
        writer.push(*rt, seq as u64, *ts, payload)?;
    }

    Ok(DedupStats { total_records, group_count })
}

/// Build a single OTLP ExportLogsServiceRequest payload for one dedup group.
fn build_otlp_payload(group: &DedupGroup, mode_label: &str) -> Vec<u8> {
    let rep = &group.representative;
    let sig_hex = format!("{:016x}", group.signature);

    let mut attrs = rep.record_attrs.clone();
    push_attr_i64(&mut attrs, "dedup.count", group.count as i64);
    let effective_mode = if group.drain3_template.is_some() {
        "full/drain3"
    } else if mode_label == "full" {
        "full/canon"
    } else {
        mode_label
    };
    push_attr_str(&mut attrs, "dedup.mode", effective_mode);
    push_attr_str(&mut attrs, "dedup.signature", &sig_hex);
    push_attr_i64(&mut attrs, "dedup.first_seen_ns", group.first_seen_ns as i64);
    push_attr_i64(&mut attrs, "dedup.last_seen_ns", group.last_seen_ns as i64);
    push_attr_i64(
        &mut attrs,
        "dedup.time_span_ms",
        ((group.last_seen_ns.saturating_sub(group.first_seen_ns)) / 1_000_000) as i64,
    );
    push_attr_str(
        &mut attrs,
        "dedup.exemplar_trace_ids",
        &group.exemplar_trace_ids.join(","),
    );
    push_attr_str(
        &mut attrs,
        "dedup.exemplar_span_ids",
        &group.exemplar_span_ids.join(","),
    );
    if let Some(ref canon) = group.canonical_body {
        push_attr_str(&mut attrs, "dedup.canonical_body", canon);
    }
    if let Some(ref shape) = group.body_shape {
        push_attr_str(&mut attrs, "dedup.body_shape", shape);
    }
    if let Some(ref template) = group.drain3_template {
        push_attr_str(&mut attrs, "dedup.drain3_template", template);
    }
    if let Some(cid) = group.drain3_cluster_id {
        push_attr_i64(&mut attrs, "dedup.drain3_cluster_id", cid);
    }

    let lr = LogRecord {
        time_unix_nano: group.first_seen_ns,
        observed_time_unix_nano: group.last_seen_ns,
        severity_number: rep.severity_number,
        severity_text: rep.severity_text.clone(),
        body: Some(AnyValue {
            value: Some(Value::StringValue(rep.body.clone())),
        }),
        attributes: attrs,
        dropped_attributes_count: 0,
        flags: 0,
        trace_id: rep.trace_id.clone(),
        span_id: rep.span_id.clone(),
        event_name: rep.event_name.clone(),
    };

    let batch = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: rep.resource_attrs.clone(),
                dropped_attributes_count: 0,
                entity_refs: Vec::new(),
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: rep.scope_name.clone(),
                    version: String::new(),
                    attributes: rep.scope_attrs.clone(),
                    dropped_attributes_count: 0,
                }),
                log_records: vec![lr],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };
    batch.encode_to_vec()
}

fn push_attr_str(attrs: &mut Vec<KeyValue>, key: &str, val: &str) {
    attrs.push(KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::StringValue(val.to_string())),
        }),
    });
}

fn push_attr_i64(attrs: &mut Vec<KeyValue>, key: &str, val: i64) {
    attrs.push(KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(Value::IntValue(val)),
        }),
    });
}
