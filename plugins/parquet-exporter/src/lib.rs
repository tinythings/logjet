//! Parquet exporter plugin for `ljx`.
//!
//! This crate implements the stable `liblogjet::export` C ABI directly.
//! The host streams raw logjet records into `write_record`, and the plugin
//! emits a Parquet file through the host-provided write/flush callbacks.

use std::ffi::{c_char, c_void};
use std::io::{self, Write};
use std::sync::Arc;

use arrow_array::builder::{Int32Builder, StringBuilder, UInt32Builder, UInt64Builder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use liblogjet::export::{
    LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST, LJX_EXPORT_CAP_RECORD_LOGS, LJX_EXPORT_CAP_STREAMING, LJX_EXPORT_STATUS_BAD_ARG,
    LJX_EXPORT_STATUS_ERROR, LJX_EXPORT_STATUS_IO, LJX_EXPORT_STATUS_OK, LJX_EXPORT_STATUS_UNSUPPORTED, LJX_EXPORTER_ABI_MAJOR,
    LJX_EXPORTER_ABI_MINOR, LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST, LJX_RECORD_TYPE_LOGS, LjxAbiBytes, LjxAbiString, LjxExportHostV1,
    LjxExportInitV1, LjxExportRecordV1, LjxExporterCtx, LjxExporterDescriptorV1,
};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use parquet::arrow::arrow_writer::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use prost::Message;
use serde_json::{Map as JsonMap, Value as JsonValue};

const PLUGIN_API_VERSION: u32 = 1;
const DEFAULT_ROW_GROUP_ROWS: usize = 8_192;
const DEFAULT_COMPRESSION: &str = "zstd";

struct ExporterDescriptor(LjxExporterDescriptorV1);

unsafe impl Sync for ExporterDescriptor {}

static PARQUET_EXPORTER_DESCRIPTOR: ExporterDescriptor = ExporterDescriptor(LjxExporterDescriptorV1 {
    struct_size: std::mem::size_of::<LjxExporterDescriptorV1>() as u32,
    abi_major: LJX_EXPORTER_ABI_MAJOR,
    abi_minor: LJX_EXPORTER_ABI_MINOR,
    plugin_api_version: PLUGIN_API_VERSION,
    capabilities: LJX_EXPORT_CAP_STREAMING | LJX_EXPORT_CAP_RECORD_LOGS | LJX_EXPORT_CAP_PAYLOAD_OTLP_EXPORT_LOGS_REQUEST,
    format_name: LjxAbiString::from_static("parquet"),
    display_name: LjxAbiString::from_static("Parquet"),
    default_extension: LjxAbiString::from_static("parquet"),
    create: parquet_exporter_create,
    write_record: parquet_exporter_write_record,
    finish: parquet_exporter_finish,
    last_error: parquet_exporter_last_error,
    free: parquet_exporter_free,
    reserved: [0; 6],
});

/// Returns the stable exporter descriptor for this plugin.
#[unsafe(no_mangle)]
pub extern "C" fn ljx_exporter_descriptor_v1() -> *const LjxExporterDescriptorV1 {
    &PARQUET_EXPORTER_DESCRIPTOR.0
}

struct ParquetConfig {
    row_group_rows: usize,
    compression: Compression,
}

struct HostSink {
    user: *mut c_void,
    write: unsafe extern "C" fn(user: *mut c_void, data: *const u8, len: usize) -> i32,
    flush: Option<unsafe extern "C" fn(user: *mut c_void) -> i32>,
}

unsafe impl Send for HostSink {}

impl Write for HostSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let status = unsafe { (self.write)(self.user, buf.as_ptr(), buf.len()) };
        match status {
            LJX_EXPORT_STATUS_OK => Ok(buf.len()),
            LJX_EXPORT_STATUS_IO => Err(io::Error::other("host callback write failed")),
            other => Err(io::Error::other(format!("host callback write returned status {other}"))),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.flush {
            Some(flush) => match unsafe { flush(self.user) } {
                LJX_EXPORT_STATUS_OK => Ok(()),
                LJX_EXPORT_STATUS_IO => Err(io::Error::other("host callback flush failed")),
                other => Err(io::Error::other(format!("host callback flush returned status {other}"))),
            },
            None => Ok(()),
        }
    }
}

struct ParquetExporter {
    cfg: ParquetConfig,
    schema: SchemaRef,
    writer: ArrowWriter<HostSink>,
    rows: RowBuffer,
    finished: bool,
    last_error: String,
}

impl ParquetExporter {
    fn create(host: &LjxExportHostV1, init: Option<&LjxExportInitV1>) -> Result<Self, String> {
        validate_host(host)?;
        let cfg = ParquetConfig::from_init(init)?;
        let schema = schema();
        let sink = HostSink { user: host.user, write: host.write, flush: host.flush };
        let props = WriterProperties::builder().set_compression(cfg.compression).set_max_row_group_row_count(Some(cfg.row_group_rows)).build();
        let writer =
            ArrowWriter::try_new(sink, Arc::clone(&schema), Some(props)).map_err(|err| format!("failed to initialise parquet writer: {err}"))?;
        Ok(Self { cfg, schema, writer, rows: RowBuffer::default(), finished: false, last_error: String::new() })
    }

    fn write_record(&mut self, record: &LjxExportRecordV1) -> i32 {
        if self.finished {
            return self.fail_status(LJX_EXPORT_STATUS_ERROR, "finish already succeeded; further writes are rejected");
        }
        if record.struct_size < std::mem::size_of::<LjxExportRecordV1>() as u32 {
            return self.fail_status(
                LJX_EXPORT_STATUS_BAD_ARG,
                format!("record struct_size {} is smaller than host ABI expects {}", record.struct_size, std::mem::size_of::<LjxExportRecordV1>()),
            );
        }
        if record.payload.ptr.is_null() && record.payload.len != 0 {
            return self.fail_status(LJX_EXPORT_STATUS_BAD_ARG, "record payload pointer is null but len is non-zero");
        }
        if record.record_type != LJX_RECORD_TYPE_LOGS {
            return self.fail_status(
                LJX_EXPORT_STATUS_UNSUPPORTED,
                format!("unsupported record type {}; parquet exporter currently supports logs only", record.record_type),
            );
        }
        if record.payload_kind != LJX_PAYLOAD_KIND_OTLP_EXPORT_LOGS_REQUEST {
            return self.fail_status(
                LJX_EXPORT_STATUS_UNSUPPORTED,
                format!("unsupported payload kind {}; parquet exporter currently supports OTLP ExportLogsServiceRequest only", record.payload_kind),
            );
        }

        let payload = match abi_bytes(record.payload) {
            Ok(payload) => payload,
            Err(err) => return self.fail_status(LJX_EXPORT_STATUS_BAD_ARG, err),
        };
        let request = match ExportLogsServiceRequest::decode(payload) {
            Ok(request) => request,
            Err(err) => return self.fail_status(LJX_EXPORT_STATUS_ERROR, format!("failed to decode OTLP logs payload at seq {}: {err}", record.seq)),
        };

        for resource_logs in &request.resource_logs {
            let resource_attrs = resource_logs.resource.as_ref().map(|resource| resource.attributes.as_slice()).unwrap_or(&[]);
            let service_name = find_attr_string(resource_attrs, "service.name");
            let resource_attributes_json = attrs_to_json(resource_attrs);
            for scope_logs in &resource_logs.scope_logs {
                let scope = scope_logs.scope.as_ref();
                let scope_name = scope.and_then(|scope| non_empty(scope.name.as_str()));
                let scope_version = scope.and_then(|scope| non_empty(scope.version.as_str()));
                let scope_attributes_json = scope.and_then(|scope| attrs_to_json(scope.attributes.as_slice()));
                for log_record in &scope_logs.log_records {
                    let row = ParquetRow {
                        sequence: record.seq,
                        timestamp_unix_ns: record.timestamp_unix_ns,
                        observed_timestamp_unix_ns: zero_is_none(log_record.observed_time_unix_nano),
                        trace_id: bytes_to_lower_hex(&log_record.trace_id),
                        span_id: bytes_to_lower_hex(&log_record.span_id),
                        trace_flags: zero_is_none_u32(log_record.flags),
                        severity_number: (log_record.severity_number != 0).then_some(log_record.severity_number),
                        severity_text: non_empty_owned(&log_record.severity_text),
                        body_kind: body_kind(log_record.body.as_ref()),
                        body_string: body_string(log_record.body.as_ref()),
                        body_json: body_json(log_record.body.as_ref()),
                        service_name: service_name.clone(),
                        scope_name: scope_name.map(str::to_owned),
                        scope_version: scope_version.map(str::to_owned),
                        resource_attributes_json: resource_attributes_json.clone(),
                        scope_attributes_json: scope_attributes_json.clone(),
                        log_attributes_json: attrs_to_json(&log_record.attributes),
                        event_name: non_empty_owned(&log_record.event_name),
                    };
                    self.rows.push(row);
                }
            }
        }

        if self.rows.len() >= self.cfg.row_group_rows
            && let Err(err) = self.flush_rows()
        {
            return self.fail_status(status_for_message(&err), err);
        }
        LJX_EXPORT_STATUS_OK
    }

    fn finish(&mut self) -> i32 {
        if self.finished {
            return self.fail_status(LJX_EXPORT_STATUS_ERROR, "finish already called");
        }
        if let Err(err) = self.flush_rows() {
            return self.fail_status(status_for_message(&err), err);
        }
        if let Err(err) = self.writer.finish() {
            return self.fail_status(status_for_message(&err.to_string()), format!("failed to finish parquet writer: {err}"));
        }
        if let Err(err) = self.writer.sync() {
            return self.fail_status(LJX_EXPORT_STATUS_IO, format!("failed to flush host output after parquet finish: {err}"));
        }
        self.finished = true;
        self.last_error.clear();
        LJX_EXPORT_STATUS_OK
    }

    fn flush_rows(&mut self) -> Result<(), String> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let batch = self.rows.drain_into_batch(Arc::clone(&self.schema))?;
        self.writer.write(&batch).map_err(|err| format!("failed to write parquet batch: {err}"))?;
        if self.writer.in_progress_rows() >= self.cfg.row_group_rows {
            self.writer.flush().map_err(|err| format!("failed to flush parquet row group: {err}"))?;
            self.writer.sync().map_err(|err| format!("failed to flush host output: {err}"))?;
        }
        Ok(())
    }

    fn fail_status(&mut self, status: i32, message: impl Into<String>) -> i32 {
        self.last_error = message.into();
        status
    }
}

impl ParquetConfig {
    /// Supported option keys:
    /// - `output.row-group-rows` => positive integer row-group target
    /// - `output.compression` => `zstd` (default) or `uncompressed`
    fn from_init(init: Option<&LjxExportInitV1>) -> Result<Self, String> {
        let mut row_group_rows = DEFAULT_ROW_GROUP_ROWS;
        let mut compression = parse_compression(DEFAULT_COMPRESSION)?;

        let Some(init) = init else {
            return Ok(Self { row_group_rows, compression });
        };
        if init.struct_size < std::mem::size_of::<LjxExportInitV1>() as u32 {
            return Err(format!(
                "init struct_size {} is smaller than plugin ABI expects {}",
                init.struct_size,
                std::mem::size_of::<LjxExportInitV1>()
            ));
        }
        if init.options.is_null() && init.options_len != 0 {
            return Err("init options pointer is null but options_len is non-zero".to_string());
        }
        if init.options_len == 0 {
            return Ok(Self { row_group_rows, compression });
        }

        let options = unsafe { std::slice::from_raw_parts(init.options, init.options_len) };
        for option in options {
            let key = abi_string(option.key)?;
            let value = abi_string(option.value)?;
            match key.as_str() {
                "output.row-group-rows" => {
                    row_group_rows = value.parse::<usize>().map_err(|err| format!("invalid output.row-group-rows `{value}`: {err}"))?;
                    if row_group_rows == 0 {
                        return Err("output.row-group-rows must be greater than zero".to_string());
                    }
                }
                "output.compression" => {
                    compression = parse_compression(value.as_str())?;
                }
                other => {
                    return Err(format!("unsupported parquet exporter option `{other}`"));
                }
            }
        }

        Ok(Self { row_group_rows, compression })
    }
}

#[derive(Default)]
struct RowBuffer {
    sequence: Vec<u64>,
    timestamp_unix_ns: Vec<u64>,
    observed_timestamp_unix_ns: Vec<Option<u64>>,
    trace_id: Vec<Option<String>>,
    span_id: Vec<Option<String>>,
    trace_flags: Vec<Option<u32>>,
    severity_number: Vec<Option<i32>>,
    severity_text: Vec<Option<String>>,
    body_kind: Vec<String>,
    body_string: Vec<Option<String>>,
    body_json: Vec<Option<String>>,
    service_name: Vec<Option<String>>,
    scope_name: Vec<Option<String>>,
    scope_version: Vec<Option<String>>,
    resource_attributes_json: Vec<Option<String>>,
    scope_attributes_json: Vec<Option<String>>,
    log_attributes_json: Vec<Option<String>>,
    event_name: Vec<Option<String>>,
}

struct ParquetRow {
    sequence: u64,
    timestamp_unix_ns: u64,
    observed_timestamp_unix_ns: Option<u64>,
    trace_id: Option<String>,
    span_id: Option<String>,
    trace_flags: Option<u32>,
    severity_number: Option<i32>,
    severity_text: Option<String>,
    body_kind: String,
    body_string: Option<String>,
    body_json: Option<String>,
    service_name: Option<String>,
    scope_name: Option<String>,
    scope_version: Option<String>,
    resource_attributes_json: Option<String>,
    scope_attributes_json: Option<String>,
    log_attributes_json: Option<String>,
    event_name: Option<String>,
}

impl RowBuffer {
    fn push(&mut self, row: ParquetRow) {
        self.sequence.push(row.sequence);
        self.timestamp_unix_ns.push(row.timestamp_unix_ns);
        self.observed_timestamp_unix_ns.push(row.observed_timestamp_unix_ns);
        self.trace_id.push(row.trace_id);
        self.span_id.push(row.span_id);
        self.trace_flags.push(row.trace_flags);
        self.severity_number.push(row.severity_number);
        self.severity_text.push(row.severity_text);
        self.body_kind.push(row.body_kind);
        self.body_string.push(row.body_string);
        self.body_json.push(row.body_json);
        self.service_name.push(row.service_name);
        self.scope_name.push(row.scope_name);
        self.scope_version.push(row.scope_version);
        self.resource_attributes_json.push(row.resource_attributes_json);
        self.scope_attributes_json.push(row.scope_attributes_json);
        self.log_attributes_json.push(row.log_attributes_json);
        self.event_name.push(row.event_name);
    }

    fn len(&self) -> usize {
        self.sequence.len()
    }

    fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    fn drain_into_batch(&mut self, schema: SchemaRef) -> Result<RecordBatch, String> {
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(u64_array(std::mem::take(&mut self.sequence))),
            Arc::new(u64_array(std::mem::take(&mut self.timestamp_unix_ns))),
            Arc::new(opt_u64_array(std::mem::take(&mut self.observed_timestamp_unix_ns))),
            Arc::new(opt_string_array(std::mem::take(&mut self.trace_id))),
            Arc::new(opt_string_array(std::mem::take(&mut self.span_id))),
            Arc::new(opt_u32_array(std::mem::take(&mut self.trace_flags))),
            Arc::new(opt_i32_array(std::mem::take(&mut self.severity_number))),
            Arc::new(opt_string_array(std::mem::take(&mut self.severity_text))),
            Arc::new(string_array(std::mem::take(&mut self.body_kind))),
            Arc::new(opt_string_array(std::mem::take(&mut self.body_string))),
            Arc::new(opt_string_array(std::mem::take(&mut self.body_json))),
            Arc::new(opt_string_array(std::mem::take(&mut self.service_name))),
            Arc::new(opt_string_array(std::mem::take(&mut self.scope_name))),
            Arc::new(opt_string_array(std::mem::take(&mut self.scope_version))),
            Arc::new(opt_string_array(std::mem::take(&mut self.resource_attributes_json))),
            Arc::new(opt_string_array(std::mem::take(&mut self.scope_attributes_json))),
            Arc::new(opt_string_array(std::mem::take(&mut self.log_attributes_json))),
            Arc::new(opt_string_array(std::mem::take(&mut self.event_name))),
        ];
        RecordBatch::try_new(schema, arrays).map_err(|err| format!("failed to build parquet batch: {err}"))
    }
}

fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("sequence", DataType::UInt64, false),
        Field::new("timestamp_unix_ns", DataType::UInt64, false),
        Field::new("observed_timestamp_unix_ns", DataType::UInt64, true),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
        Field::new("trace_flags", DataType::UInt32, true),
        Field::new("severity_number", DataType::Int32, true),
        Field::new("severity_text", DataType::Utf8, true),
        Field::new("body_kind", DataType::Utf8, false),
        // `body_string` preserves the common string fast-path while `body_json`
        // keeps non-string AnyValue bodies in a stable bounded representation.
        Field::new("body_string", DataType::Utf8, true),
        Field::new("body_json", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, true),
        Field::new("scope_name", DataType::Utf8, true),
        Field::new("scope_version", DataType::Utf8, true),
        // Resource, scope, and log attributes stay separate JSON text columns to
        // avoid unbounded top-level column growth while preserving OTel meaning.
        Field::new("resource_attributes_json", DataType::Utf8, true),
        Field::new("scope_attributes_json", DataType::Utf8, true),
        Field::new("log_attributes_json", DataType::Utf8, true),
        Field::new("event_name", DataType::Utf8, true),
    ]))
}

fn validate_host(host: &LjxExportHostV1) -> Result<(), String> {
    if host.struct_size < std::mem::size_of::<LjxExportHostV1>() as u32 {
        return Err(format!("host struct_size {} is smaller than plugin ABI expects {}", host.struct_size, std::mem::size_of::<LjxExportHostV1>()));
    }
    Ok(())
}

fn parse_compression(value: &str) -> Result<Compression, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "zstd" => Ok(Compression::ZSTD(Default::default())),
        "uncompressed" | "none" => Ok(Compression::UNCOMPRESSED),
        other => Err(format!("unsupported output.compression `{other}`; supported values: zstd, uncompressed")),
    }
}

fn abi_string(value: LjxAbiString) -> Result<String, String> {
    if value.ptr.is_null() {
        return if value.len == 0 { Ok(String::new()) } else { Err("ABI string pointer is null but len is non-zero".to_string()) };
    }
    let bytes = unsafe { std::slice::from_raw_parts(value.ptr.cast::<u8>(), value.len) };
    let text = std::str::from_utf8(bytes).map_err(|err| format!("ABI string is not valid UTF-8: {err}"))?;
    Ok(text.to_owned())
}

fn abi_bytes<'a>(value: LjxAbiBytes) -> Result<&'a [u8], String> {
    if value.ptr.is_null() {
        return if value.len == 0 { Ok(&[]) } else { Err("ABI byte slice pointer is null but len is non-zero".to_string()) };
    }
    Ok(unsafe { std::slice::from_raw_parts(value.ptr, value.len) })
}

fn status_for_message(message: &str) -> i32 {
    if message.contains("host callback") || message.contains("flush host output") { LJX_EXPORT_STATUS_IO } else { LJX_EXPORT_STATUS_ERROR }
}

fn zero_is_none(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

fn zero_is_none_u32(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn non_empty_owned(value: &str) -> Option<String> {
    non_empty(value).map(str::to_owned)
}

fn bytes_to_lower_hex(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(nibble_to_hex(byte >> 4));
        out.push(nibble_to_hex(byte & 0x0f));
    }
    Some(out)
}

fn nibble_to_hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + (value - 10)) as char,
    }
}

fn body_kind(value: Option<&AnyValue>) -> String {
    match value.and_then(|value| value.value.as_ref()) {
        None => "empty",
        Some(Value::StringValue(_)) => "string",
        Some(Value::BoolValue(_)) => "bool",
        Some(Value::IntValue(_)) => "int",
        Some(Value::DoubleValue(_)) => "double",
        Some(Value::BytesValue(_)) => "bytes",
        Some(Value::ArrayValue(_)) => "array",
        Some(Value::KvlistValue(_)) => "kvlist",
    }
    .to_string()
}

fn body_string(value: Option<&AnyValue>) -> Option<String> {
    match value.and_then(|value| value.value.as_ref()) {
        Some(Value::StringValue(text)) => Some(text.clone()),
        _ => None,
    }
}

fn body_json(value: Option<&AnyValue>) -> Option<String> {
    let json = any_value_to_json(value?)?;
    Some(json.to_string())
}

fn any_value_to_json(value: &AnyValue) -> Option<JsonValue> {
    match value.value.as_ref()? {
        Value::StringValue(text) => Some(JsonValue::String(text.clone())),
        Value::BoolValue(flag) => Some(JsonValue::Bool(*flag)),
        Value::IntValue(number) => Some(JsonValue::Number((*number).into())),
        Value::DoubleValue(number) => serde_json::Number::from_f64(*number).map(JsonValue::Number),
        Value::BytesValue(bytes) => Some(JsonValue::String(bytes_to_lower_hex(bytes).unwrap_or_default())),
        Value::ArrayValue(array) => Some(JsonValue::Array(array.values.iter().filter_map(any_value_to_json).collect())),
        Value::KvlistValue(map) => {
            let mut entries = map
                .values
                .iter()
                .filter_map(|kv| kv.value.as_ref().and_then(any_value_to_json).map(|value| (kv.key.clone(), value)))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut out = JsonMap::new();
            for (key, value) in entries {
                out.insert(key, value);
            }
            Some(JsonValue::Object(out))
        }
    }
}

fn attrs_to_json(attrs: &[KeyValue]) -> Option<String> {
    if attrs.is_empty() {
        return None;
    }
    let mut pairs =
        attrs.iter().filter_map(|attr| attr.value.as_ref().and_then(any_value_to_json).map(|value| (attr.key.clone(), value))).collect::<Vec<_>>();
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_by(|left, right| left.0.cmp(&right.0));
    let mut out = JsonMap::new();
    for (key, value) in pairs {
        out.insert(key, value);
    }
    Some(JsonValue::Object(out).to_string())
}

fn find_attr_string(attrs: &[KeyValue], key: &str) -> Option<String> {
    attrs.iter().find(|attr| attr.key == key).and_then(|attr| match attr.value.as_ref()?.value.as_ref()? {
        Value::StringValue(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
}

fn u64_array(values: Vec<u64>) -> arrow_array::UInt64Array {
    let mut builder = UInt64Builder::new();
    for value in values {
        builder.append_value(value);
    }
    builder.finish()
}

fn opt_u64_array(values: Vec<Option<u64>>) -> arrow_array::UInt64Array {
    let mut builder = UInt64Builder::new();
    for value in values {
        match value {
            Some(value) => builder.append_value(value),
            None => builder.append_null(),
        }
    }
    builder.finish()
}

fn opt_u32_array(values: Vec<Option<u32>>) -> arrow_array::UInt32Array {
    let mut builder = UInt32Builder::new();
    for value in values {
        match value {
            Some(value) => builder.append_value(value),
            None => builder.append_null(),
        }
    }
    builder.finish()
}

fn opt_i32_array(values: Vec<Option<i32>>) -> arrow_array::Int32Array {
    let mut builder = Int32Builder::new();
    for value in values {
        match value {
            Some(value) => builder.append_value(value),
            None => builder.append_null(),
        }
    }
    builder.finish()
}

fn string_array(values: Vec<String>) -> arrow_array::StringArray {
    let mut builder = StringBuilder::new();
    for value in values {
        builder.append_value(value);
    }
    builder.finish()
}

fn opt_string_array(values: Vec<Option<String>>) -> arrow_array::StringArray {
    let mut builder = StringBuilder::new();
    for value in values {
        match value {
            Some(value) => builder.append_value(value),
            None => builder.append_null(),
        }
    }
    builder.finish()
}

fn ctx_mut<'a>(ctx: *mut LjxExporterCtx) -> Result<&'a mut ParquetExporter, i32> {
    if ctx.is_null() {
        return Err(LJX_EXPORT_STATUS_BAD_ARG);
    }
    Ok(unsafe { &mut *(ctx.cast::<ParquetExporter>()) })
}

fn ctx_ref<'a>(ctx: *mut LjxExporterCtx) -> Option<&'a ParquetExporter> {
    (!ctx.is_null()).then(|| unsafe { &*(ctx.cast::<ParquetExporter>()) })
}

/// Creates a new Parquet exporter instance.
///
/// # Safety
///
/// `host` must point to a valid `LjxExportHostV1` for the duration of the call.
/// If non-null, `init` must point to a valid `LjxExportInitV1` for the duration
/// of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parquet_exporter_create(host: *const LjxExportHostV1, init: *const LjxExportInitV1) -> *mut LjxExporterCtx {
    let Some(host) = (!host.is_null()).then(|| unsafe { &*host }) else {
        return std::ptr::null_mut();
    };
    let init = (!init.is_null()).then(|| unsafe { &*init });
    match ParquetExporter::create(host, init) {
        Ok(ctx) => Box::into_raw(Box::new(ctx)).cast::<LjxExporterCtx>(),
        Err(err) => {
            eprintln!("ljx parquet exporter: create failed: {err}");
            std::ptr::null_mut()
        }
    }
}

/// Pushes one logjet record into the Parquet exporter.
///
/// # Safety
///
/// `ctx` must be a valid exporter context previously returned by
/// `parquet_exporter_create`. `record` must point to a valid
/// `LjxExportRecordV1` for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn parquet_exporter_write_record(ctx: *mut LjxExporterCtx, record: *const LjxExportRecordV1) -> i32 {
    let Some(record) = (!record.is_null()).then(|| unsafe { &*record }) else {
        return LJX_EXPORT_STATUS_BAD_ARG;
    };
    match ctx_mut(ctx) {
        Ok(ctx) => ctx.write_record(record),
        Err(status) => status,
    }
}

/// Finalises the Parquet stream and flushes host output.
#[unsafe(no_mangle)]
pub extern "C" fn parquet_exporter_finish(ctx: *mut LjxExporterCtx) -> i32 {
    match ctx_mut(ctx) {
        Ok(ctx) => ctx.finish(),
        Err(status) => status,
    }
}

/// Returns the last plugin-side error string.
#[unsafe(no_mangle)]
pub extern "C" fn parquet_exporter_last_error(ctx: *mut LjxExporterCtx) -> LjxAbiString {
    let Some(ctx) = ctx_ref(ctx) else {
        return LjxAbiString { ptr: std::ptr::null::<c_char>(), len: 0 };
    };
    if ctx.last_error.is_empty() {
        LjxAbiString { ptr: std::ptr::null::<c_char>(), len: 0 }
    } else {
        LjxAbiString { ptr: ctx.last_error.as_ptr().cast::<c_char>(), len: ctx.last_error.len() }
    }
}

/// Releases a Parquet exporter instance. Accepts null.
#[unsafe(no_mangle)]
pub extern "C" fn parquet_exporter_free(ctx: *mut LjxExporterCtx) {
    if ctx.is_null() {
        return;
    }
    let _ = unsafe { Box::from_raw(ctx.cast::<ParquetExporter>()) };
}
