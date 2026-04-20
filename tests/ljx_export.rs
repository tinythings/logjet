use std::fs::{self, File};
use std::io::{self, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use arrow_array::Array;
use logjet::{Codec, LogjetReader, LogjetWriter, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use prost::Message;

#[test]
fn ljx_exports_cpp_demo_to_parquet_and_preserves_rows() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-parquet")?;
    let input = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("logs").join("cpp-demo.logjet");
    let output = dir.path().join("cpp-demo.parquet");
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
    assert!(actual.iter().all(|row| row.body_kind == "string" || row.body_kind == "empty" || row.body_json.is_some()));
    Ok(())
}

#[test]
fn ljx_exports_empty_input_to_empty_parquet() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-empty")?;
    let input = dir.path().join("empty.logjet");
    let output = dir.path().join("empty.parquet");
    fs::write(&input, [])?;

    let export = run_ljx_export(&input, &output)?;
    if !export.status.success() {
        return Err(io::Error::other(format!("empty export failed: {}", String::from_utf8_lossy(&export.stderr))));
    }

    let actual = read_parquet_rows(&output)?;
    assert!(actual.is_empty());
    Ok(())
}

#[test]
fn ljx_fails_on_malformed_input_during_parquet_export() -> io::Result<()> {
    ensure_export_artifacts_exist()?;

    let dir = TestDir::new("ljx-export-malformed")?;
    let input = dir.path().join("broken.logjet");
    let output = dir.path().join("broken.parquet");
    fs::write(&input, b"definitely not a logjet stream")?;

    let export = run_ljx_export(&input, &output)?;
    assert!(!export.status.success());
    let stderr = String::from_utf8_lossy(&export.stderr);
    assert!(stderr.contains("failed reading") || stderr.contains("failed to") || stderr.contains("exporter"));
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

fn encode_logs_request(message: &str, service_name: Option<&str>) -> io::Result<Vec<u8>> {
    let resource_logs = ResourceLogs {
        resource: Some(opentelemetry_proto::tonic::resource::v1::Resource {
            attributes: service_name
                .map(|name| {
                    vec![KeyValue { key: "service.name".to_string(), value: Some(AnyValue { value: Some(Value::StringValue(name.to_string())) }) }]
                })
                .unwrap_or_default(),
            dropped_attributes_count: 0,
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

fn read_parquet_rows(path: &Path) -> io::Result<Vec<ParquetRow>> {
    let file = File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(io::Error::other)?;
    let mut reader = builder.with_batch_size(1024).build().map_err(io::Error::other)?;
    let mut rows = Vec::new();
    while let Some(batch) = reader.next() {
        let batch = batch.map_err(io::Error::other)?;
        let sequence = batch
            .column_by_name("sequence")
            .ok_or_else(|| io::Error::other("missing sequence column"))?
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .ok_or_else(|| io::Error::other("sequence column type mismatch"))?;
        let body_kind = batch
            .column_by_name("body_kind")
            .ok_or_else(|| io::Error::other("missing body_kind column"))?
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| io::Error::other("body_kind column type mismatch"))?;
        let body_string = batch
            .column_by_name("body_string")
            .ok_or_else(|| io::Error::other("missing body_string column"))?
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| io::Error::other("body_string column type mismatch"))?;
        let body_json = batch
            .column_by_name("body_json")
            .ok_or_else(|| io::Error::other("missing body_json column"))?
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| io::Error::other("body_json column type mismatch"))?;
        let service_name = batch
            .column_by_name("service_name")
            .ok_or_else(|| io::Error::other("missing service_name column"))?
            .as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| io::Error::other("service_name column type mismatch"))?;
        for row in 0..batch.num_rows() {
            rows.push(ParquetRow {
                sequence: sequence.value(row),
                body_kind: body_kind.value(row).to_string(),
                body_string: (!body_string.is_null(row)).then(|| body_string.value(row).to_string()),
                body_json: (!body_json.is_null(row)).then(|| body_json.value(row).to_string()),
                service_name: (!service_name.is_null(row)).then(|| service_name.value(row).to_string()),
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

struct ParquetRow {
    sequence: u64,
    body_kind: String,
    body_string: Option<String>,
    body_json: Option<String>,
    service_name: Option<String>,
}
