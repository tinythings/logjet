use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use logjet::{LogjetReader, LogjetWriter, OwnedRecord, RecordType, WriterConfig};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;

use super::types::{ActiveScan, DetailRecord, EntryMeta, FieldCatalog, SCAN_BATCH_SIZE, ScanUpdate};
use crate::error::{Error, Result};
use crate::input::InputHandle;

/// Scans the logjet file in the background to collect distinct severity texts and service names.
pub(super) fn scan_field_catalog(input: &Path) -> Result<FieldCatalog> {
    let handle = InputHandle::open(input)?;
    let mut reader = LogjetReader::new(handle.into_buf_reader());
    let mut severities = HashSet::new();
    let mut services = HashSet::new();

    while let Some(record) = reader.next_record()? {
        if record.record_type != RecordType::Logs {
            continue;
        }
        if let Ok(batch) = ExportLogsServiceRequest::decode(record.payload.as_slice()) {
            for rl in &batch.resource_logs {
                if let Some(res) = &rl.resource {
                    for attr in &res.attributes {
                        if attr.key == "service.name"
                            && let Some(AnyValue { value: Some(Value::StringValue(s)) }) = &attr.value
                        {
                            services.insert(s.clone());
                        }
                    }
                }
                for sl in &rl.scope_logs {
                    for lr in &sl.log_records {
                        if !lr.severity_text.is_empty() {
                            severities.insert(lr.severity_text.clone());
                        }
                    }
                }
            }
        }
    }

    let mut severities: Vec<_> = severities.into_iter().collect();
    let mut services: Vec<_> = services.into_iter().collect();
    severities.sort();
    services.sort();
    Ok(FieldCatalog { severities, services })
}

pub(super) fn scan_matches(
    input_path: &Path, predicate: crate::predicate::RecordPredicate, mut spool: File, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ScanUpdate>,
) -> Result<(u64, u64)> {
    let input = InputHandle::open(input_path)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut tx_batch = Vec::with_capacity(SCAN_BATCH_SIZE);
    let mut scanned = 0u64;
    let mut matched = 0u64;

    while !cancel.load(Ordering::Relaxed) {
        let Some(record) = reader.next_record()? else {
            break;
        };
        scanned = scanned.checked_add(1).ok_or(logjet::Error::NumericOverflow("view scanned"))?;

        if predicate.matches(&record) {
            tx_batch.push(write_spool_record(&mut spool, &record)?);
            matched = matched.checked_add(1).ok_or(logjet::Error::NumericOverflow("view matched"))?;
            if tx_batch.len() >= SCAN_BATCH_SIZE {
                tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
            }
        }
    }

    if !tx_batch.is_empty() {
        tx.send(ScanUpdate::Batch(tx_batch)).map_err(|err| Error::Usage(err.to_string()))?;
    }

    Ok((scanned, matched))
}

pub(super) fn follow_appended_matches(
    input_path: &Path, predicate: crate::predicate::RecordPredicate, mut spool: File, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ScanUpdate>,
) -> Result<()> {
    let mut input = InputHandle::open(input_path)?;
    input.seek(SeekFrom::End(0))?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut tx_batch = Vec::with_capacity(SCAN_BATCH_SIZE);

    while !cancel.load(Ordering::Relaxed) {
        match reader.next_record()? {
            Some(record) => {
                if predicate.matches(&record) {
                    tx_batch.push(write_spool_record(&mut spool, &record)?);
                    if tx_batch.len() >= SCAN_BATCH_SIZE {
                        tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
                    }
                }
            }
            None => {
                if !tx_batch.is_empty() {
                    tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }
    }

    if !tx_batch.is_empty() {
        tx.send(ScanUpdate::Batch(tx_batch)).map_err(|err| Error::Usage(err.to_string()))?;
    }

    Ok(())
}

pub(crate) fn write_spool_record(file: &mut File, record: &OwnedRecord) -> Result<EntryMeta> {
    let offset = file.seek(SeekFrom::End(0))?;
    file.write_all(&[record.record_type as u8])?;
    file.write_all(&record.seq.to_le_bytes())?;
    file.write_all(&record.ts_unix_ns.to_le_bytes())?;
    let payload_len = u64::try_from(record.payload.len()).map_err(|_| logjet::Error::NumericOverflow("view payload_len"))?;
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&record.payload)?;
    file.flush()?;

    Ok(EntryMeta { offset, record_type: record.record_type, seq: record.seq, ts_unix_ns: record.ts_unix_ns, payload_len })
}

pub(crate) fn read_spool_record(file: &mut File, meta: EntryMeta) -> Result<DetailRecord> {
    file.seek(SeekFrom::Start(meta.offset + 1 + 8 + 8 + 8))?;
    let mut payload = vec![0u8; meta.payload_len as usize];
    file.read_exact(&mut payload)?;
    Ok(DetailRecord { meta, payload })
}

pub(crate) fn create_temp_path() -> Result<PathBuf> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|err| Error::Usage(format!("system clock error: {err}")))?.as_nanos();
    for attempt in 0..1000u32 {
        let candidate = base.join(format!("ljx-view-{pid}-{nanos}-{attempt}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::Usage("unable to allocate a temporary view file".to_string()))
}

pub(crate) fn open_temp_spool_pair() -> Result<(PathBuf, File, File)> {
    let spool_path = create_temp_path()?;
    let spool_reader = OpenOptions::new().read(true).write(true).create_new(true).open(&spool_path)?;
    let spool_writer = OpenOptions::new().read(true).write(true).open(&spool_path)?;
    Ok((spool_path, spool_reader, spool_writer))
}

pub(super) fn remember_summary(cache: &mut HashMap<usize, String>, order: &mut VecDeque<usize>, index: usize, summary: String) {
    cache.insert(index, summary);
    order.push_back(index);
    while order.len() > super::types::SUMMARY_CACHE_LIMIT {
        if let Some(old) = order.pop_front() {
            cache.remove(&old);
        }
    }
}

pub(super) fn write_export_selection_to_temp_logjet(scan: &mut ActiveScan, entries: &[EntryMeta]) -> Result<PathBuf> {
    let temp_input = create_temp_path()?;
    let file = OpenOptions::new().write(true).create_new(true).open(&temp_input)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
    for meta in entries.iter().copied() {
        let detail = read_spool_record(&mut scan.spool_reader, meta)?;
        logjet.push(detail.meta.record_type, detail.meta.seq, detail.meta.ts_unix_ns, &detail.payload)?;
    }
    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    Ok(temp_input)
}
