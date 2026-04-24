use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};

use logjet::{LogjetReader, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use serde::Serialize;

use crate::cli::DiscoverArgs;
use crate::dataset::{Dataset, DatasetEntry};
use crate::error::{Error, Result};
use crate::input::InputHandle;
use crate::predicate::RecordPredicate;

const FORMAT_VERSION: u32 = 1;

pub fn run(args: DiscoverArgs) -> Result<()> {
    run_inner(args).map_err(machine_error)
}

fn run_inner(args: DiscoverArgs) -> Result<()> {
    if args.limit == Some(0) {
        return Err(Error::Usage("--limit must be greater than zero".to_string()));
    }
    let mut predicate = args.predicate.build()?;
    let service_filter = string_filter(args.services)?;
    let severity_filter = string_filter(args.severities)?;
    predicate.field_filter.services = service_filter.clone();
    predicate.field_filter.severities = severity_filter.clone();

    let dataset = Dataset::from_inputs(&args.inputs)?;
    let entries = paged_entries(&dataset, args.offset, args.limit)?;
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut summary = DiscoverySummary::default();
    let mut file_rows = Vec::new();

    for entry in &entries {
        let file = scan_entry(entry, &predicate, service_filter.as_ref(), severity_filter.as_ref())?;
        summary.merge(&file);
        if args.ndjson {
            write_json_line(&mut out, &NdjsonRow::File { format_version: FORMAT_VERSION, row: &file.row })?;
        }
        file_rows.push(file.row);
    }

    let top_services = top_counts(&summary.services, args.top_services);
    let severity_breakdown = all_counts(&summary.severities);
    let response = DiscoverResponse {
        ok: true,
        command: "discover",
        format_version: FORMAT_VERSION,
        input: InputSummary {
            files_total: dataset.len(),
            offset: args.offset,
            limit: args.limit,
            files_scanned: entries.len(),
            next_offset: next_offset(args.offset, entries.len(), dataset.len()),
        },
        summary: SummaryJson {
            records_scanned: summary.records_scanned,
            records_matched: summary.records_matched,
            log_events: summary.log_events,
            time_span_unix_ns: TimeSpanJson { first: summary.first_ts_unix_ns, last: summary.last_ts_unix_ns },
            top_services,
            severity_breakdown,
        },
        files: file_rows,
    };

    if args.ndjson {
        write_json_line(&mut out, &NdjsonRow::Summary { response: &response })?;
    } else {
        serde_json::to_writer_pretty(&mut out, &response).map_err(json_ser_error)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    Ok(())
}

fn paged_entries(dataset: &Dataset, offset: usize, limit: Option<usize>) -> Result<Vec<&DatasetEntry>> {
    if offset > dataset.len() {
        return Err(Error::Usage(format!("--offset {offset} is past the end of the {} file manifest", dataset.len())));
    }
    let take = limit.unwrap_or(usize::MAX);
    Ok(dataset.entries().iter().skip(offset).take(take).collect())
}

fn scan_entry(
    entry: &DatasetEntry, predicate: &RecordPredicate, service_filter: Option<&HashSet<String>>, severity_filter: Option<&HashSet<String>>,
) -> Result<FileScan> {
    let path = entry.path.as_path();
    let mut row = FileSummaryJson {
        path: path.display().to_string(),
        size: entry.size,
        modified_unix_ns: entry.modified_ns,
        source_sequence_span: SequenceSpanJson { first: entry.first_seq, last: entry.last_seq },
        source_time_span_unix_ns: TimeSpanJson { first: entry.first_ts_unix_ns, last: entry.last_ts_unix_ns },
        records_scanned: 0,
        records_matched: 0,
        log_events: 0,
        time_span_unix_ns: TimeSpanJson { first: None, last: None },
        skipped_by_index: false,
    };

    if let Some(index) = &entry.index
        && !index.summary.may_match(predicate)
    {
        row.skipped_by_index = true;
        return Ok(FileScan { row, services: BTreeMap::new(), severities: BTreeMap::new() });
    }

    let input = InputHandle::open(path)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut services = BTreeMap::new();
    let mut severities = BTreeMap::new();
    while let Some(record) = reader.next_record()? {
        row.records_scanned = row.records_scanned.checked_add(1).ok_or(logjet::Error::NumericOverflow("discover records_scanned"))?;
        if !predicate.matches(&record) {
            continue;
        }
        row.records_matched = row.records_matched.checked_add(1).ok_or(logjet::Error::NumericOverflow("discover records_matched"))?;
        update_span(&mut row.time_span_unix_ns, record.ts_unix_ns);
        let log_summary = summarise_matching_log_events(&record, service_filter, severity_filter)?;
        row.log_events = row.log_events.checked_add(log_summary.count).ok_or(logjet::Error::NumericOverflow("discover log_events"))?;
        merge_counts(&mut services, log_summary.services);
        merge_counts(&mut severities, log_summary.severities);
    }

    Ok(FileScan { row, services, severities })
}

fn summarise_matching_log_events(
    record: &OwnedRecord, service_filter: Option<&HashSet<String>>, severity_filter: Option<&HashSet<String>>,
) -> Result<LogEventSummary> {
    if record.record_type != RecordType::Logs {
        return Ok(LogEventSummary::default());
    }
    let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) else {
        return Ok(LogEventSummary::default());
    };
    let mut summary = LogEventSummary::default();
    for resource_logs in &batch.resource_logs {
        let service = service_name(resource_logs);
        if let Some(allowed) = service_filter
            && !service.is_some_and(|value| allowed.contains(value))
        {
            continue;
        }
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                if let Some(allowed) = severity_filter
                    && !allowed.contains(&log_record.severity_text)
                {
                    continue;
                }
                summary.count = summary.count.checked_add(1).ok_or(logjet::Error::NumericOverflow("discover log_events"))?;
                if let Some(service) = service {
                    increment_count(&mut summary.services, service);
                }
                let severity = if log_record.severity_text.is_empty() { "<unset>" } else { &log_record.severity_text };
                increment_count(&mut summary.severities, severity);
            }
        }
    }
    Ok(summary)
}

fn service_name(resource_logs: &opentelemetry_proto::tonic::logs::v1::ResourceLogs) -> Option<&str> {
    let resource = resource_logs.resource.as_ref()?;
    resource.attributes.iter().find_map(|attr| {
        if attr.key != "service.name" {
            return None;
        }
        match &attr.value {
            Some(AnyValue { value: Some(Value::StringValue(value)) }) => Some(value.as_str()),
            _ => None,
        }
    })
}

#[derive(Default)]
struct DiscoverySummary {
    records_scanned: u64,
    records_matched: u64,
    log_events: u64,
    first_ts_unix_ns: Option<u64>,
    last_ts_unix_ns: Option<u64>,
    services: BTreeMap<String, u64>,
    severities: BTreeMap<String, u64>,
}

impl DiscoverySummary {
    fn merge(&mut self, file: &FileScan) {
        let row = &file.row;
        self.records_scanned += row.records_scanned;
        self.records_matched += row.records_matched;
        self.log_events += row.log_events;
        merge_span(&mut self.first_ts_unix_ns, &mut self.last_ts_unix_ns, &row.time_span_unix_ns);
        merge_counts(&mut self.services, file.services.clone());
        merge_counts(&mut self.severities, file.severities.clone());
    }
}

struct FileScan {
    row: FileSummaryJson,
    services: BTreeMap<String, u64>,
    severities: BTreeMap<String, u64>,
}

#[derive(Default)]
struct LogEventSummary {
    count: u64,
    services: BTreeMap<String, u64>,
    severities: BTreeMap<String, u64>,
}

fn update_span(span: &mut TimeSpanJson, ts_unix_ns: u64) {
    span.first = Some(span.first.map_or(ts_unix_ns, |current| current.min(ts_unix_ns)));
    span.last = Some(span.last.map_or(ts_unix_ns, |current| current.max(ts_unix_ns)));
}

fn merge_span(first: &mut Option<u64>, last: &mut Option<u64>, span: &TimeSpanJson) {
    if let Some(value) = span.first {
        *first = Some(first.map_or(value, |current| current.min(value)));
    }
    if let Some(value) = span.last {
        *last = Some(last.map_or(value, |current| current.max(value)));
    }
}

fn increment_count(counts: &mut BTreeMap<String, u64>, value: &str) {
    *counts.entry(value.to_string()).or_insert(0) += 1;
}

fn merge_counts(target: &mut BTreeMap<String, u64>, source: BTreeMap<String, u64>) {
    for (value, count) in source {
        *target.entry(value).or_insert(0) += count;
    }
}

fn top_counts(counts: &BTreeMap<String, u64>, limit: usize) -> Vec<CountJson> {
    let mut rows = counts.iter().map(|(value, count)| CountJson { value: value.clone(), count: *count }).collect::<Vec<_>>();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    rows.truncate(limit);
    rows
}

fn all_counts(counts: &BTreeMap<String, u64>) -> Vec<CountJson> {
    counts.iter().map(|(value, count)| CountJson { value: value.clone(), count: *count }).collect()
}

fn string_filter(values: Vec<String>) -> Result<Option<HashSet<String>>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut set = HashSet::new();
    for value in values {
        if value.is_empty() {
            return Err(Error::Usage("empty service/severity filter values are not allowed".to_string()));
        }
        set.insert(value);
    }
    Ok(Some(set))
}

fn next_offset(offset: usize, scanned: usize, total: usize) -> Option<usize> {
    let next = offset.saturating_add(scanned);
    if next < total { Some(next) } else { None }
}

fn write_json_line<T: Serialize>(out: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *out, value).map_err(json_ser_error)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn json_ser_error(err: serde_json::Error) -> Error {
    Error::Usage(format!("failed to serialise discovery JSON: {err}"))
}

fn machine_error(err: Error) -> Error {
    match err {
        Error::JsonUsage { .. } => err,
        other => Error::JsonUsage { code: "discover_failed", message: other.to_string() },
    }
}

#[derive(Serialize)]
struct DiscoverResponse {
    ok: bool,
    command: &'static str,
    format_version: u32,
    input: InputSummary,
    summary: SummaryJson,
    files: Vec<FileSummaryJson>,
}

#[derive(Serialize)]
struct InputSummary {
    files_total: usize,
    offset: usize,
    limit: Option<usize>,
    files_scanned: usize,
    next_offset: Option<usize>,
}

#[derive(Serialize)]
struct SummaryJson {
    records_scanned: u64,
    records_matched: u64,
    log_events: u64,
    time_span_unix_ns: TimeSpanJson,
    top_services: Vec<CountJson>,
    severity_breakdown: Vec<CountJson>,
}

#[derive(Debug, Clone, Serialize)]
struct FileSummaryJson {
    path: String,
    size: u64,
    modified_unix_ns: Option<u64>,
    source_sequence_span: SequenceSpanJson,
    source_time_span_unix_ns: TimeSpanJson,
    records_scanned: u64,
    records_matched: u64,
    log_events: u64,
    time_span_unix_ns: TimeSpanJson,
    skipped_by_index: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct TimeSpanJson {
    first: Option<u64>,
    last: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct SequenceSpanJson {
    first: Option<u64>,
    last: Option<u64>,
}

#[derive(Serialize)]
struct CountJson {
    value: String,
    count: u64,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum NdjsonRow<'a> {
    #[serde(rename = "file")]
    File { format_version: u32, row: &'a FileSummaryJson },
    #[serde(rename = "summary")]
    Summary { response: &'a DiscoverResponse },
}

#[cfg(test)]
#[path = "../../tests/unit/commands/discover_ut.rs"]
mod discover_ut;
