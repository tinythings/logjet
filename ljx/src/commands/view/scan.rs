use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
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

use super::types::{ActiveScan, DetailRecord, EntryMeta, FieldCatalog, ListRowSummary, SCAN_BATCH_SIZE, ScanUpdate, ViewOrder};
use crate::dataset::Dataset;
use crate::dataset_index::read_block_records;
use crate::error::{Error, Result};
use crate::input::InputHandle;

/// Scans the logjet file in the background to collect distinct severity texts and service names.
pub(super) fn scan_field_catalog(dataset: &Dataset, workers: usize) -> Result<FieldCatalog> {
    let paths = dataset.paths().map(|path| path.to_path_buf()).collect::<Vec<_>>();
    let worker_count = workers.max(1).min(paths.len().max(1));
    if worker_count == 1 {
        return scan_field_catalog_sequential(&paths);
    }

    let (tx, rx) = mpsc::channel();
    let paths = Arc::new(paths);
    let cursor = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let tx = tx.clone();
        let paths = Arc::clone(&paths);
        let cursor = Arc::clone(&cursor);
        handles.push(thread::spawn(move || {
            loop {
                let idx = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if idx >= paths.len() {
                    break;
                }
                let path = &paths[idx];
                let result = scan_field_catalog_file(path);
                let _ = tx.send(result);
            }
        }));
    }
    drop(tx);

    let mut severities = HashSet::new();
    let mut services = HashSet::new();
    for result in rx {
        let (file_services, file_severities) = result?;
        services.extend(file_services);
        severities.extend(file_severities);
    }
    for handle in handles {
        let _ = handle.join();
    }

    let mut severities: Vec<_> = severities.into_iter().collect();
    let mut services: Vec<_> = services.into_iter().collect();
    severities.sort();
    services.sort();
    Ok(FieldCatalog { severities, services })
}

fn scan_field_catalog_sequential(paths: &[PathBuf]) -> Result<FieldCatalog> {
    let mut severities = HashSet::new();
    let mut services = HashSet::new();

    for input in paths {
        let (file_services, file_severities) = scan_field_catalog_file(input)?;
        services.extend(file_services);
        severities.extend(file_severities);
    }

    let mut severities: Vec<_> = severities.into_iter().collect();
    let mut services: Vec<_> = services.into_iter().collect();
    severities.sort();
    services.sort();
    Ok(FieldCatalog { severities, services })
}

fn scan_field_catalog_file(path: &Path) -> Result<(HashSet<String>, HashSet<String>)> {
    let mut severities = HashSet::new();
    let mut services = HashSet::new();

    let handle = InputHandle::open(path)?;
    let mut reader = LogjetReader::new(handle.into_buf_reader());
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

    Ok((services, severities))
}

pub(super) fn scan_matches(
    dataset: &Dataset, order: ViewOrder, predicate: crate::predicate::RecordPredicate, spool: File, cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ScanUpdate>,
) -> Result<(u64, u64)> {
    match order {
        ViewOrder::Concat => scan_matches_concat(dataset, predicate, spool, cancel, tx),
        ViewOrder::MergeSeq | ViewOrder::MergeTs => scan_matches_merged(dataset, order, predicate, spool, cancel, tx),
    }
}

fn scan_matches_concat(
    dataset: &Dataset, predicate: crate::predicate::RecordPredicate, mut spool: File, cancel: Arc<AtomicBool>, tx: mpsc::Sender<ScanUpdate>,
) -> Result<(u64, u64)> {
    let mut tx_batch = Vec::with_capacity(SCAN_BATCH_SIZE);
    let mut scanned = 0u64;
    let mut matched = 0u64;

    for entry in dataset.entries() {
        let input_path = entry.path.as_path();
        if let Some(index) = &entry.index
            && !index.summary.may_match(&predicate)
        {
            continue;
        }
        if let Some(index) = &entry.index {
            let mut state = IndexedScanState {
                spool: &mut spool,
                cancel: &cancel,
                tx: &tx,
                tx_batch: &mut tx_batch,
                scanned: &mut scanned,
                matched: &mut matched,
            };
            scan_indexed_entry(entry, index, &predicate, &mut state)?;
            continue;
        }
        let input = InputHandle::open(input_path)?;
        let mut reader = LogjetReader::new(input.into_buf_reader());
        while !cancel.load(Ordering::Relaxed) {
            let Some(record) = reader.next_record()? else {
                break;
            };
            scanned = scanned.checked_add(1).ok_or(logjet::Error::NumericOverflow("view scanned"))?;

            if predicate.matches(&record) {
                tx_batch.push(write_spool_record(&mut spool, &record, input_path)?);
                matched = matched.checked_add(1).ok_or(logjet::Error::NumericOverflow("view matched"))?;
                if tx_batch.len() >= SCAN_BATCH_SIZE {
                    tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
                }
            }
        }
    }

    if !tx_batch.is_empty() {
        tx.send(ScanUpdate::Batch(tx_batch)).map_err(|err| Error::Usage(err.to_string()))?;
    }

    Ok((scanned, matched))
}

fn scan_matches_merged(
    dataset: &Dataset, order: ViewOrder, predicate: crate::predicate::RecordPredicate, mut spool: File, cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ScanUpdate>,
) -> Result<(u64, u64)> {
    let mut streams = dataset
        .entries()
        .iter()
        .filter(|entry| entry.index.as_ref().is_none_or(|index| index.summary.may_match(&predicate)))
        .enumerate()
        .map(|(idx, entry)| StreamState::open(idx, entry.path.as_path()))
        .collect::<Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::new();
    let mut tx_batch = Vec::with_capacity(SCAN_BATCH_SIZE);
    let mut scanned = 0u64;
    let mut matched = 0u64;
    let mut serial = 0u64;

    for stream in &mut streams {
        if let Some(item) = stream.advance(&predicate, &cancel, &mut scanned, order, &mut serial)? {
            heap.push(Reverse(item));
        }
    }

    while !cancel.load(Ordering::Relaxed) {
        let Some(Reverse(item)) = heap.pop() else {
            break;
        };
        let stream = &mut streams[item.stream_idx];
        let Some(record) = stream.pending.take() else {
            continue;
        };
        tx_batch.push(write_spool_record(&mut spool, &record, stream.path.as_path())?);
        matched = matched.checked_add(1).ok_or(logjet::Error::NumericOverflow("view matched"))?;
        if tx_batch.len() >= SCAN_BATCH_SIZE {
            tx.send(ScanUpdate::Batch(std::mem::take(&mut tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
        }
        if let Some(next) = stream.advance(&predicate, &cancel, &mut scanned, order, &mut serial)? {
            heap.push(Reverse(next));
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
                    tx_batch.push(write_spool_record(&mut spool, &record, input_path)?);
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

pub(crate) fn write_spool_record(file: &mut File, record: &OwnedRecord, source_path: &Path) -> Result<EntryMeta> {
    let offset = file.seek(SeekFrom::End(0))?;
    file.write_all(&[record.record_type as u8])?;
    file.write_all(&record.seq.to_le_bytes())?;
    file.write_all(&record.ts_unix_ns.to_le_bytes())?;
    let payload_len = u64::try_from(record.payload.len()).map_err(|_| logjet::Error::NumericOverflow("view payload_len"))?;
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&record.payload)?;
    file.flush()?;

    Ok(EntryMeta {
        offset,
        record_type: record.record_type,
        seq: record.seq,
        ts_unix_ns: record.ts_unix_ns,
        payload_len,
        source_path: source_path.to_path_buf(),
    })
}

pub(crate) fn read_spool_record(file: &mut File, meta: EntryMeta) -> Result<DetailRecord> {
    file.seek(SeekFrom::Start(meta.offset + 1 + 8 + 8 + 8))?;
    let mut payload = vec![0u8; meta.payload_len as usize];
    file.read_exact(&mut payload)?;
    Ok(DetailRecord { meta, payload })
}

type ViewReader = LogjetReader<std::io::BufReader<InputHandle>>;

struct StreamState {
    idx: usize,
    path: PathBuf,
    reader: ViewReader,
    pending: Option<OwnedRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MergeItem {
    key: (u64, u64, usize, u64),
    stream_idx: usize,
}

impl StreamState {
    fn open(idx: usize, path: &Path) -> Result<Self> {
        let input = InputHandle::open(path)?;
        Ok(Self { idx, path: path.to_path_buf(), reader: LogjetReader::new(input.into_buf_reader()), pending: None })
    }

    fn advance(
        &mut self, predicate: &crate::predicate::RecordPredicate, cancel: &Arc<AtomicBool>, scanned: &mut u64, order: ViewOrder, serial: &mut u64,
    ) -> Result<Option<MergeItem>> {
        while !cancel.load(Ordering::Relaxed) {
            let Some(record) = self.reader.next_record()? else {
                self.pending = None;
                return Ok(None);
            };
            *scanned = scanned.checked_add(1).ok_or(logjet::Error::NumericOverflow("view scanned"))?;
            if !predicate.matches(&record) {
                continue;
            }
            let item = MergeItem { key: merge_key(order, &record, self.idx, *serial), stream_idx: self.idx };
            *serial = serial.checked_add(1).ok_or(logjet::Error::NumericOverflow("view merge serial"))?;
            self.pending = Some(record);
            return Ok(Some(item));
        }
        Ok(None)
    }
}

fn merge_key(order: ViewOrder, record: &OwnedRecord, stream_idx: usize, serial: u64) -> (u64, u64, usize, u64) {
    match order {
        ViewOrder::Concat => (serial, 0, stream_idx, serial),
        ViewOrder::MergeSeq => (record.seq, record.ts_unix_ns, stream_idx, serial),
        ViewOrder::MergeTs => (record.ts_unix_ns, record.seq, stream_idx, serial),
    }
}

struct IndexedScanState<'a> {
    spool: &'a mut File,
    cancel: &'a Arc<AtomicBool>,
    tx: &'a mpsc::Sender<ScanUpdate>,
    tx_batch: &'a mut Vec<EntryMeta>,
    scanned: &'a mut u64,
    matched: &'a mut u64,
}

fn scan_indexed_entry(
    entry: &crate::dataset::DatasetEntry, index: &crate::dataset_index::DatasetIndex, predicate: &crate::predicate::RecordPredicate,
    state: &mut IndexedScanState<'_>,
) -> Result<()> {
    for block in &index.blocks {
        if state.cancel.load(Ordering::Relaxed) {
            break;
        }
        if !block.may_match(predicate) {
            continue;
        }
        for record in read_block_records(entry.path.as_path(), block)? {
            *state.scanned = (*state.scanned).checked_add(1).ok_or(logjet::Error::NumericOverflow("view scanned"))?;
            if !predicate.matches(&record) {
                continue;
            }
            state.tx_batch.push(write_spool_record(state.spool, &record, entry.path.as_path())?);
            *state.matched = (*state.matched).checked_add(1).ok_or(logjet::Error::NumericOverflow("view matched"))?;
            if state.tx_batch.len() >= SCAN_BATCH_SIZE {
                state.tx.send(ScanUpdate::Batch(std::mem::take(state.tx_batch))).map_err(|err| Error::Usage(err.to_string()))?;
            }
        }
    }
    Ok(())
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

pub(super) fn remember_summary(cache: &mut HashMap<usize, ListRowSummary>, order: &mut VecDeque<usize>, index: usize, summary: ListRowSummary) {
    cache.insert(index, summary);
    order.push_back(index);
    while order.len() > super::types::SUMMARY_CACHE_LIMIT {
        if let Some(old) = order.pop_front() {
            cache.remove(&old);
        }
    }
}

pub(crate) fn write_export_selection_to_temp_logjet(scan: &mut ActiveScan, entries: &[EntryMeta]) -> Result<PathBuf> {
    let temp_input = create_temp_path()?;
    let file = OpenOptions::new().write(true).create_new(true).open(&temp_input)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
    let mut block_last = None;
    for meta in entries {
        let detail = read_spool_record(&mut scan.spool_reader, meta.clone())?;
        push_preserving_view_order(&mut logjet, &mut block_last, detail.meta.record_type, detail.meta.seq, detail.meta.ts_unix_ns, &detail.payload)?;
    }
    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    Ok(temp_input)
}

pub(crate) fn push_preserving_view_order<W: Write>(
    logjet: &mut LogjetWriter<W>, block_last: &mut Option<(u64, u64)>, record_type: RecordType, seq: u64, ts_unix_ns: u64, payload: &[u8],
) -> Result<()> {
    if logjet.pending_bytes() > 0
        && let Some((last_seq, last_ts)) = *block_last
        && (seq < last_seq || ts_unix_ns < last_ts)
    {
        logjet.flush_block()?;
        *block_last = None;
    }

    logjet.push(record_type, seq, ts_unix_ns, payload)?;
    if logjet.pending_bytes() == 0 {
        *block_last = None;
    } else {
        *block_last = Some((seq, ts_unix_ns));
    }
    Ok(())
}
