use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::Array;
use logjet::{Codec, LogjetReader, LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::metric::Data as MetricData;
use opentelemetry_proto::tonic::metrics::v1::{Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use prost::Message;

#[test]
fn ljx_exports_cpp_demo_to_parquet_and_preserves_rows() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-parquet")?;
    let input = dir.path().join("cpp-demo.logjet");
    let output = dir.path().join("cpp-demo.parquet");
    write_cpp_demo_fixture(&input)?;
    let expected = decode_expected_rows(&input)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("ljx export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual.iter().map(|row| row.sequence).collect::<Vec<_>>(), expected.iter().map(|row| row.sequence).collect::<Vec<_>>());
    assert_eq!(
        actual.iter().filter_map(|row| row.body_string.clone()).collect::<Vec<_>>(),
        expected.iter().filter_map(|row| row.body_string.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        actual.iter().filter_map(|row| row.service_name.clone()).collect::<Vec<_>>(),
        expected.iter().filter_map(|row| row.service_name.clone()).collect::<Vec<_>>()
    );
    assert!(
        actual.iter().all(|row| row.body_kind == Some("string".to_string()) || row.body_kind == Some("empty".to_string()) || row.body_json.is_some())
    );
    Ok(())
}

#[test]
fn ljx_exports_empty_input_to_empty_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-empty")?;
    let input = dir.path().join("empty.logjet");
    let output = dir.path().join("empty.parquet");
    write_empty_logjet_fixture(&input)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("empty export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert!(actual.is_empty());
    Ok(())
}

#[test]
fn ljx_exports_unrecoverable_garbage_input_to_empty_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-malformed")?;
    let input = dir.path().join("broken.logjet");
    let output = dir.path().join("broken.parquet");
    fs::write(&input, b"definitely not a logjet stream")?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("garbage export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert!(actual.is_empty());
    Ok(())
}

#[test]
fn ljx_exports_large_generated_input_to_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-large")?;
    let input = dir.path().join("large.logjet");
    let output = dir.path().join("large.parquet");
    write_large_logjet_fixture(&input, 5000)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("large export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert_eq!(actual.len(), 5000);
    assert_eq!(actual.first().and_then(|row| row.body_string.as_deref()), Some("large-row-0"));
    assert_eq!(actual.last().and_then(|row| row.body_string.as_deref()), Some("large-row-4999"));
    Ok(())
}

#[test]
fn ljx_exports_metrics_to_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-metrics")?;
    let input = dir.path().join("metrics.logjet");
    let output = dir.path().join("metrics.parquet");
    write_metrics_fixture(&input)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("metrics export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert_eq!(actual.len(), 3, "expected 3 metric datapoint rows (one per metric type)");

    // First row: gauge
    let gauge = &actual[0];
    assert_eq!(gauge.signal_type, Some("metrics".to_string()));
    assert_eq!(gauge.metric_name, Some("cpu_usage".to_string()));
    assert_eq!(gauge.metric_type, Some("Gauge".to_string()));
    assert!(gauge.metric_value_number.is_some());
    assert_eq!(gauge.service_name, Some("metrics-service".to_string()));

    // Second row: sum
    let sum = &actual[1];
    assert_eq!(sum.metric_name, Some("request_count".to_string()));
    assert_eq!(sum.metric_type, Some("Sum".to_string()));
    assert_eq!(sum.is_monotonic, Some(true));
    assert!(sum.metric_value_number.is_some());

    // Third row: histogram
    let hist = &actual[2];
    assert_eq!(hist.metric_name, Some("latency".to_string()));
    assert_eq!(hist.metric_type, Some("Histogram".to_string()));
    assert!(hist.metric_value_count.is_some());

    Ok(())
}

#[test]
fn ljx_exports_traces_to_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-traces")?;
    let input = dir.path().join("traces.logjet");
    let output = dir.path().join("traces.parquet");
    write_traces_fixture(&input)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("traces export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert_eq!(actual.len(), 2, "expected 2 span rows");

    let span1 = &actual[0];
    assert_eq!(span1.signal_type, Some("traces".to_string()));
    assert_eq!(span1.span_name, Some("GET /api/users".to_string()));
    assert_eq!(span1.span_kind, Some("Server".to_string()));
    assert!(span1.trace_id.is_some());
    assert!(span1.span_id.is_some());
    assert!(span1.duration_ns.is_some());
    assert_eq!(span1.service_name, Some("trace-service".to_string()));

    let span2 = &actual[1];
    assert_eq!(span2.span_name, Some("query_database".to_string()));
    assert_eq!(span2.span_kind, Some("Client".to_string()));
    assert!(span2.parent_span_id.is_some());

    Ok(())
}

#[test]
fn ljx_exports_mixed_signals_to_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-mixed")?;
    let input = dir.path().join("mixed.logjet");
    let output = dir.path().join("mixed.parquet");
    write_mixed_fixture(&input)?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("mixed export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert_eq!(actual.len(), 6, "expected 6 rows: 1 log + 3 metrics + 2 traces");

    let logs = actual.iter().filter(|r| r.signal_type == Some("logs".to_string())).collect::<Vec<_>>();
    let metrics = actual.iter().filter(|r| r.signal_type == Some("metrics".to_string())).collect::<Vec<_>>();
    let traces = actual.iter().filter(|r| r.signal_type == Some("traces".to_string())).collect::<Vec<_>>();

    assert_eq!(logs.len(), 1);
    assert_eq!(metrics.len(), 3);
    assert_eq!(traces.len(), 2);

    // Verify logs row has metrics/traces columns null
    let log_row = logs[0];
    assert!(log_row.metric_name.is_none());
    assert!(log_row.span_name.is_none());
    assert!(log_row.body_string.is_some());

    // Verify metrics row has logs/traces columns null
    let metric_row = metrics[0];
    assert!(metric_row.body_string.is_none());
    assert!(metric_row.span_name.is_none());
    assert!(metric_row.metric_name.is_some());

    // Verify traces row has logs/metrics columns null
    let trace_row = traces[0];
    assert!(trace_row.body_string.is_none());
    assert!(trace_row.metric_name.is_none());
    assert!(trace_row.span_name.is_some());

    Ok(())
}

fn ensure_export_artifacts_exist() -> io::Result<()> {
    for path in [ljx_bin(), parquet_plugin_bin()] {
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "missing test artifact {}. build it first with: cargo build -p ljx -p ljx-parquet-exporter",
                path.display()
            )));
        }
    }
    Ok(())
}

fn run_ljx_export(input: &Path, output: &Path) -> io::Result<Output> {
    Command::new(ljx_bin())
        .env("LJX_EXPORTER_PATH", parquet_plugin_bin())
        .arg("--export")
        .arg("parquet")
        .arg(input)
        .arg("-o")
        .arg(output)
        .arg("--force")
        .output()
}

fn decode_expected_rows(path: &Path) -> io::Result<Vec<ExpectedRow>> {
    let file = File::open(path)?;
    let mut reader = LogjetReader::new(BufReader::new(file));
    let mut rows = Vec::new();
    while let Some(record) = reader.next_record().map_err(io::Error::other)? {
        let batch =
            ExportLogsServiceRequest::decode(record.payload.as_slice()).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        for resource_logs in batch.resource_logs {
            let service_name = resource_logs.resource.as_ref().and_then(|resource| find_attr_string(&resource.attributes, "service.name"));
            for scope_logs in resource_logs.scope_logs {
                for log_record in scope_logs.log_records {
                    rows.push(ExpectedRow {
                        sequence: record.seq,
                        body_string: log_record.body.as_ref().and_then(string_body),
                        service_name: service_name.clone(),
                    });
                }
            }
        }
    }
    Ok(rows)
}

fn write_large_logjet_fixture(path: &Path, count: u64) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = LogjetWriter::with_config(file, WriterConfig { codec: Codec::Lz4, ..WriterConfig::default() });
    for i in 0..count {
        let payload = encode_logs_request(&format!("large-row-{i}"), Some("ljx-export-it"))?;
        writer.push(RecordType::Logs, i + 1, 1_700_000_000_000_000_000 + i, &payload).map_err(io::Error::other)?;
    }
    let mut file = writer.into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn write_cpp_demo_fixture(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = LogjetWriter::with_config(file, WriterConfig { codec: Codec::Lz4, ..WriterConfig::default() });
    for i in 0..25u64 {
        let payload = encode_logs_request(&format!("cpp-demo-row-{i}"), Some("hello-cpp"))?;
        writer.push(RecordType::Logs, i + 1, 1_700_000_000_000_000_000 + i, &payload).map_err(io::Error::other)?;
    }
    let mut file = writer.into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn write_empty_logjet_fixture(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let mut file = LogjetWriter::new(file).into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn write_metrics_fixture(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = LogjetWriter::with_config(file, WriterConfig { codec: Codec::Lz4, ..WriterConfig::default() });
    let payload = encode_metrics_request(Some("metrics-service"))?;
    writer.push(RecordType::Metrics, 1, 1_700_000_000_000_000_000, &payload).map_err(io::Error::other)?;
    let mut file = writer.into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn write_traces_fixture(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = LogjetWriter::with_config(file, WriterConfig { codec: Codec::Lz4, ..WriterConfig::default() });
    let payload = encode_traces_request(Some("trace-service"))?;
    writer.push(RecordType::Traces, 1, 1_700_000_000_000_000_000, &payload).map_err(io::Error::other)?;
    let mut file = writer.into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn write_mixed_fixture(path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = LogjetWriter::with_config(file, WriterConfig { codec: Codec::Lz4, ..WriterConfig::default() });

    let logs_payload = encode_logs_request("mixed-log", Some("mixed-service"))?;
    writer.push(RecordType::Logs, 1, 1_700_000_000_000_000_000, &logs_payload).map_err(io::Error::other)?;

    let metrics_payload = encode_metrics_request(Some("mixed-service"))?;
    writer.push(RecordType::Metrics, 2, 1_700_000_000_000_000_001, &metrics_payload).map_err(io::Error::other)?;

    let traces_payload = encode_traces_request(Some("mixed-service"))?;
    writer.push(RecordType::Traces, 3, 1_700_000_000_000_000_002, &traces_payload).map_err(io::Error::other)?;

    let mut file = writer.into_inner().map_err(io::Error::other)?;
    file.flush()?;
    Ok(())
}

fn encode_logs_request(message: &str, service_name: Option<&str>) -> io::Result<Vec<u8>> {
    let resource_logs = ResourceLogs {
        resource: Some(opentelemetry_proto::tonic::resource::v1::Resource {
            attributes: service_name
                .map(|name| {
                    vec![KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) }]
                })
                .unwrap_or_default(),
            dropped_attributes_count: 0,
            entity_refs: Vec::new(),
        }),
        scope_logs: vec![ScopeLogs {
            scope: None,
            log_records: vec![LogRecord {
                time_unix_nano: 1_700_000_000_000_000_000,
                observed_time_unix_nano: 1_700_000_000_000_000_000,
                severity_number: 9,
                severity_text: "INFO".to_string(),
                body: Some(AnyValue { value: Some(Value::StringValue(message.to_string())) }),
                attributes: Vec::new(),
                dropped_attributes_count: 0,
                flags: 0,
                trace_id: Vec::new(),
                span_id: Vec::new(),
                event_name: String::new(),
            }],
            schema_url: String::new(),
        }],
        schema_url: String::new(),
    };
    Ok(ExportLogsServiceRequest { resource_logs: vec![resource_logs] }.encode_to_vec())
}

fn encode_metrics_request(service_name: Option<&str>) -> io::Result<Vec<u8>> {
    let resource = opentelemetry_proto::tonic::resource::v1::Resource {
        attributes: service_name
            .map(|name| {
                vec![KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) }]
            })
            .unwrap_or_default(),
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let metrics = vec![
        Metric {
            name: "cpu_usage".to_string(),
            description: "CPU usage percentage".to_string(),
            unit: "%".to_string(),
            data: Some(MetricData::Gauge(Gauge {
                data_points: vec![NumberDataPoint {
                    time_unix_nano: 1_700_000_000_000_000_000,
                    start_time_unix_nano: 0,
                    value: Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(42.5)),
                    attributes: vec![],
                    exemplars: vec![],
                    flags: 0,
                }],
            })),
            metadata: vec![],
        },
        Metric {
            name: "request_count".to_string(),
            description: "Total request count".to_string(),
            unit: "1".to_string(),
            data: Some(MetricData::Sum(opentelemetry_proto::tonic::metrics::v1::Sum {
                data_points: vec![NumberDataPoint {
                    time_unix_nano: 1_700_000_000_000_000_000,
                    start_time_unix_nano: 1_600_000_000_000_000_000,
                    value: Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(100)),
                    attributes: vec![],
                    exemplars: vec![],
                    flags: 0,
                }],
                aggregation_temporality: 2,
                is_monotonic: true,
            })),
            metadata: vec![],
        },
        Metric {
            name: "latency".to_string(),
            description: "Request latency".to_string(),
            unit: "ms".to_string(),
            data: Some(MetricData::Histogram(opentelemetry_proto::tonic::metrics::v1::Histogram {
                data_points: vec![opentelemetry_proto::tonic::metrics::v1::HistogramDataPoint {
                    time_unix_nano: 1_700_000_000_000_000_000,
                    start_time_unix_nano: 1_600_000_000_000_000_000,
                    count: 50,
                    sum: Some(250.0),
                    bucket_counts: vec![],
                    explicit_bounds: vec![],
                    attributes: vec![],
                    exemplars: vec![],
                    flags: 0,
                    max: None,
                    min: None,
                }],
                aggregation_temporality: 2,
            })),
            metadata: vec![],
        },
    ];

    let resource_metrics = ResourceMetrics {
        resource: Some(resource),
        scope_metrics: vec![ScopeMetrics { scope: None, metrics, schema_url: String::new() }],
        schema_url: String::new(),
    };

    Ok(ExportMetricsServiceRequest { resource_metrics: vec![resource_metrics] }.encode_to_vec())
}

fn encode_traces_request(service_name: Option<&str>) -> io::Result<Vec<u8>> {
    let resource = opentelemetry_proto::tonic::resource::v1::Resource {
        attributes: service_name
            .map(|name| {
                vec![KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) }]
            })
            .unwrap_or_default(),
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let spans = vec![
        Span {
            trace_id: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10],
            span_id: vec![0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18],
            parent_span_id: vec![],
            trace_state: String::new(),
            name: "GET /api/users".to_string(),
            kind: 2, // Server
            start_time_unix_nano: 1_700_000_000_000_000_000,
            end_time_unix_nano: 1_700_000_000_000_001_000,
            attributes: vec![],
            dropped_attributes_count: 0,
            events: vec![],
            dropped_events_count: 0,
            links: vec![],
            dropped_links_count: 0,
            status: Some(Status { code: 1, message: String::new() }),
            flags: 0,
        },
        Span {
            trace_id: vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10],
            span_id: vec![0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28],
            parent_span_id: vec![0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18],
            trace_state: String::new(),
            name: "query_database".to_string(),
            kind: 3, // Client
            start_time_unix_nano: 1_700_000_000_000_000_100,
            end_time_unix_nano: 1_700_000_000_000_000_800,
            attributes: vec![],
            dropped_attributes_count: 0,
            events: vec![],
            dropped_events_count: 0,
            links: vec![],
            dropped_links_count: 0,
            status: Some(Status { code: 0, message: String::new() }),
            flags: 0,
        },
    ];

    let resource_spans = ResourceSpans {
        resource: Some(resource),
        scope_spans: vec![ScopeSpans { scope: None, spans, schema_url: String::new() }],
        schema_url: String::new(),
    };

    Ok(ExportTraceServiceRequest { resource_spans: vec![resource_spans] }.encode_to_vec())
}

fn read_parquet_rows(path: &Path) -> io::Result<Vec<ParquetRow>> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(io::Error::other)?;
    let reader = builder.with_batch_size(1024).build().map_err(io::Error::other)?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(io::Error::other)?;

        macro_rules! col {
            ($name:expr, $ty:ty) => {
                batch
                    .column_by_name($name)
                    .ok_or_else(|| io::Error::other(format!("missing {} column", $name)))?
                    .as_any()
                    .downcast_ref::<$ty>()
                    .ok_or_else(|| io::Error::other(format!("{} column type mismatch", $name)))?
            };
        }

        macro_rules! opt_string {
            ($col:expr, $row:expr) => {
                if $col.is_null($row) { None } else { Some($col.value($row).to_string()) }
            };
        }

        macro_rules! opt_u64 {
            ($col:expr, $row:expr) => {
                if $col.is_null($row) { None } else { Some($col.value($row)) }
            };
        }

        macro_rules! opt_f64 {
            ($col:expr, $row:expr) => {
                if $col.is_null($row) { None } else { Some($col.value($row)) }
            };
        }

        macro_rules! opt_bool {
            ($col:expr, $row:expr) => {
                if $col.is_null($row) { None } else { Some($col.value($row)) }
            };
        }

        let sequence = col!("sequence", arrow_array::UInt64Array);
        let signal_type = col!("signal_type", arrow_array::StringArray);
        let body_kind = col!("body_kind", arrow_array::StringArray);
        let body_string = col!("body_string", arrow_array::StringArray);
        let body_json = col!("body_json", arrow_array::StringArray);
        let service_name = col!("service_name", arrow_array::StringArray);
        let metric_name = col!("metric_name", arrow_array::StringArray);
        let metric_type = col!("metric_type", arrow_array::StringArray);
        let metric_value_number = col!("metric_value_number", arrow_array::Float64Array);
        let metric_value_count = col!("metric_value_count", arrow_array::UInt64Array);
        let is_monotonic = col!("is_monotonic", arrow_array::BooleanArray);
        let span_name = col!("span_name", arrow_array::StringArray);
        let span_kind = col!("span_kind", arrow_array::StringArray);
        let trace_id = col!("trace_id", arrow_array::StringArray);
        let span_id = col!("span_id", arrow_array::StringArray);
        let parent_span_id = col!("parent_span_id", arrow_array::StringArray);
        let duration_ns = col!("duration_ns", arrow_array::UInt64Array);

        for row in 0..batch.num_rows() {
            rows.push(ParquetRow {
                sequence: sequence.value(row),
                signal_type: opt_string!(signal_type, row),
                body_kind: opt_string!(body_kind, row),
                body_string: opt_string!(body_string, row),
                body_json: opt_string!(body_json, row),
                service_name: opt_string!(service_name, row),
                metric_name: opt_string!(metric_name, row),
                metric_type: opt_string!(metric_type, row),
                metric_value_number: opt_f64!(metric_value_number, row),
                metric_value_count: opt_u64!(metric_value_count, row),
                is_monotonic: opt_bool!(is_monotonic, row),
                span_name: opt_string!(span_name, row),
                span_kind: opt_string!(span_kind, row),
                trace_id: opt_string!(trace_id, row),
                span_id: opt_string!(span_id, row),
                parent_span_id: opt_string!(parent_span_id, row),
                duration_ns: opt_u64!(duration_ns, row),
            });
        }
    }
    Ok(rows)
}

fn find_attr_string(attrs: &[KeyValue], key: &str) -> Option<String> {
    attrs.iter().find(|attr| attr.key == key).and_then(|attr| match attr.value.as_ref()?.value.as_ref()? {
        Value::StringValue(text) if !text.is_empty() => Some(text.clone()),
        _ => None,
    })
}

fn string_body(value: &AnyValue) -> Option<String> {
    match value.value.as_ref()? {
        Value::StringValue(text) => Some(text.clone()),
        _ => None,
    }
}

fn ljx_bin() -> PathBuf {
    target_dir().join("debug").join(binary_name("ljx"))
}

fn parquet_plugin_bin() -> PathBuf {
    target_dir().join("debug").join(shared_library_name("ljx_parquet_exporter"))
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"))
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn shared_library_name(stem: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(label: &str) -> io::Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path = std::env::temp_dir().join(format!("logjet-{label}-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ExpectedRow {
    sequence: u64,
    body_string: Option<String>,
    service_name: Option<String>,
}

#[derive(Debug)]
struct ParquetRow {
    sequence: u64,
    signal_type: Option<String>,
    body_kind: Option<String>,
    body_string: Option<String>,
    body_json: Option<String>,
    service_name: Option<String>,
    metric_name: Option<String>,
    metric_type: Option<String>,
    metric_value_number: Option<f64>,
    metric_value_count: Option<u64>,
    is_monotonic: Option<bool>,
    span_name: Option<String>,
    span_kind: Option<String>,
    trace_id: Option<String>,
    span_id: Option<String>,
    parent_span_id: Option<String>,
    duration_ns: Option<u64>,
}
