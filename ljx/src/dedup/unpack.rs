//! Stage 0: crack OTLP batches into flat records.
//!
//! Reads all logjet records from a file, decodes `ExportLogsServiceRequest`
//! payloads, and flattens the nested protobuf hierarchy into a
//! `Vec<FlatRecord>`. Non-log records (metrics, traces) are collected
//! separately for pass-through.

use logjet::{LogjetReader, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use std::io::{Read, Seek};

use crate::dedup::flat_record::FlatRecord;
use crate::error::Result;

/// Result of unpacking a .logjet file.
pub struct UnpackResult {
    /// Flattened log records ready for dedup.
    pub records: Vec<FlatRecord>,
    /// Non-log records (metrics, traces) to pass through unchanged.
    pub passthrough: Vec<OwnedRecord>,
}

/// Read all records from a logjet reader, flatten OTLP log batches.
pub fn unpack<R: Read + Seek>(reader: &mut LogjetReader<R>) -> Result<UnpackResult> {
    let mut records = Vec::new();
    let mut passthrough = Vec::new();

    while let Some(rec) = reader.next_record()? {
        if rec.record_type != RecordType::Logs {
            passthrough.push(rec);
            continue;
        }
        let Ok(batch) = ExportLogsServiceRequest::decode(rec.payload.as_slice()) else {
            // Can't decode → treat as passthrough (don't silently drop).
            passthrough.push(rec);
            continue;
        };
        flatten_batch(&batch, &mut records);
    }
    Ok(UnpackResult { records, passthrough })
}

/// Walk ResourceLogs → ScopeLogs → LogRecord, emit one FlatRecord per log.
fn flatten_batch(batch: &ExportLogsServiceRequest, out: &mut Vec<FlatRecord>) {
    for rl in &batch.resource_logs {
        let service_name = rl
            .resource
            .as_ref()
            .and_then(|r| {
                r.attributes
                    .iter()
                    .find(|a| a.key == "service.name")
                    .and_then(|a| if let Some(AnyValue { value: Some(Value::StringValue(s)) }) = &a.value { Some(s.clone()) } else { None })
            })
            .unwrap_or_default();

        let resource_attrs = rl.resource.as_ref().map(|r| r.attributes.clone()).unwrap_or_default();

        for sl in &rl.scope_logs {
            let scope_name = sl.scope.as_ref().map(|s| s.name.clone()).unwrap_or_default();
            let scope_attrs = sl.scope.as_ref().map(|s| s.attributes.clone()).unwrap_or_default();

            for lr in &sl.log_records {
                let body = lr
                    .body
                    .as_ref()
                    .and_then(|b| if let Some(Value::StringValue(s)) = &b.value { Some(s.clone()) } else { None })
                    .unwrap_or_default();

                let (code_filepath, code_lineno) = extract_source_location(&lr.attributes);

                out.push(FlatRecord {
                    service_name: service_name.clone(),
                    severity_number: lr.severity_number,
                    severity_text: lr.severity_text.clone(),
                    scope_name: scope_name.clone(),
                    event_name: lr.event_name.clone(),
                    code_filepath,
                    code_lineno,
                    trace_id: lr.trace_id.clone(),
                    span_id: lr.span_id.clone(),
                    time_unix_nano: lr.time_unix_nano,
                    observed_time_unix_nano: lr.observed_time_unix_nano,
                    body,
                    resource_attrs: resource_attrs.clone(),
                    scope_attrs: scope_attrs.clone(),
                    record_attrs: lr.attributes.clone(),
                });
            }
        }
    }
}

/// Extract code.filepath and code.lineno from OTel log record attributes.
fn extract_source_location(attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue]) -> (Option<String>, Option<i64>) {
    let mut filepath = None;
    let mut lineno = None;
    for attr in attrs {
        match attr.key.as_str() {
            "code.filepath" => {
                if let Some(AnyValue { value: Some(Value::StringValue(s)) }) = &attr.value {
                    filepath = Some(s.clone());
                }
            }
            "code.lineno" => {
                if let Some(AnyValue { value: Some(Value::IntValue(n)) }) = &attr.value {
                    lineno = Some(*n);
                }
            }
            _ => {}
        }
    }
    (filepath, lineno)
}
