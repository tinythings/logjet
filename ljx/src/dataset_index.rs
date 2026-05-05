use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use xxhash_rust::xxh3::xxh3_64;

use logjet::{BLOCK_HEADER_EXT_LEN, BLOCK_HEADER_FIXED_LEN, BlockHeader, BlockHeaderExt, OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::predicate::RecordPredicate;

const INDEX_VERSION: u32 = 1;
const SUMMARY_LIMIT: usize = 16;

#[derive(Debug, Clone)]
pub(crate) struct DatasetIndex {
    pub(crate) summary: IndexSummary,
    pub(crate) blocks: Vec<IndexBlock>,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexSummary {
    pub(crate) size: u64,
    pub(crate) modified_ns: Option<u64>,
    pub(crate) first_seq: Option<u64>,
    pub(crate) last_seq: Option<u64>,
    pub(crate) first_ts_unix_ns: Option<u64>,
    pub(crate) last_ts_unix_ns: Option<u64>,
    pub(crate) record_types: u8,
    pub(crate) services: Vec<String>,
    pub(crate) services_complete: bool,
    pub(crate) severities: Vec<String>,
    pub(crate) severities_complete: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct IndexBlock {
    pub(crate) offset: u64,
    pub(crate) len: u64,
    pub(crate) first_seq: Option<u64>,
    pub(crate) last_seq: Option<u64>,
    pub(crate) first_ts_unix_ns: Option<u64>,
    pub(crate) last_ts_unix_ns: Option<u64>,
    pub(crate) record_types: u8,
    pub(crate) services: Vec<String>,
    pub(crate) services_complete: bool,
    pub(crate) severities: Vec<String>,
    pub(crate) severities_complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskIndex {
    version: u32,
    source_path: String,
    source_size: u64,
    source_modified_ns: Option<u64>,
    summary: DiskSummary,
    blocks: Vec<DiskBlock>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskSummary {
    first_seq: Option<u64>,
    last_seq: Option<u64>,
    first_ts_unix_ns: Option<u64>,
    last_ts_unix_ns: Option<u64>,
    record_types: u8,
    services: Vec<String>,
    services_complete: bool,
    severities: Vec<String>,
    severities_complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskBlock {
    offset: u64,
    len: u64,
    first_seq: Option<u64>,
    last_seq: Option<u64>,
    first_ts_unix_ns: Option<u64>,
    last_ts_unix_ns: Option<u64>,
    record_types: u8,
    services: Vec<String>,
    services_complete: bool,
    severities: Vec<String>,
    severities_complete: bool,
}

pub(crate) fn load_or_build(path: &Path, size: u64, modified_ns: Option<u64>) -> Option<DatasetIndex> {
    let sidecar_path = sidecar_path(path);
    if let Some(index) = load_fresh(path, size, modified_ns, &sidecar_path) {
        return Some(index);
    }
    let index = build(path, size, modified_ns).ok()?;
    let _ = persist(path, &sidecar_path, &index);
    Some(index)
}

pub(crate) fn sidecar_path(path: &Path) -> PathBuf {
    let cache_dir = cache_root_dir().unwrap_or_else(|| PathBuf::from("."));
    let hash = xxh3_64(path.to_string_lossy().as_bytes());
    let file_name = format!("{:016x}.ljxidx", hash);
    cache_dir.join(file_name)
}

fn cache_root_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    })?;
    let dir = base.join("ljx");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir)
}

impl IndexSummary {
    pub(crate) fn may_match(&self, predicate: &RecordPredicate) -> bool {
        if let Some(kind) = predicate.record_type_filter()
            && self.record_types & record_type_bit(kind) == 0
        {
            return false;
        }
        if let Some(min) = predicate.seq_min_filter()
            && let Some(last) = self.last_seq
            && last < min
        {
            return false;
        }
        if let Some(max) = predicate.seq_max_filter()
            && let Some(first) = self.first_seq
            && first > max
        {
            return false;
        }
        if let Some(min) = predicate.ts_min_filter()
            && let Some(last) = self.last_ts_unix_ns
            && last < min
        {
            return false;
        }
        if let Some(max) = predicate.ts_max_filter()
            && let Some(first) = self.first_ts_unix_ns
            && first > max
        {
            return false;
        }
        if let Some(services) = predicate.service_filter()
            && self.services_complete
            && !services.iter().any(|svc| self.services.iter().any(|v| v == svc))
        {
            return false;
        }
        if let Some(severities) = predicate.severity_filter()
            && self.severities_complete
            && !severities.iter().any(|sev| self.severities.iter().any(|v| v == sev))
        {
            return false;
        }
        true
    }
}

impl IndexBlock {
    pub(crate) fn may_match(&self, predicate: &RecordPredicate) -> bool {
        IndexSummary {
            size: self.len,
            modified_ns: None,
            first_seq: self.first_seq,
            last_seq: self.last_seq,
            first_ts_unix_ns: self.first_ts_unix_ns,
            last_ts_unix_ns: self.last_ts_unix_ns,
            record_types: self.record_types,
            services: self.services.clone(),
            services_complete: self.services_complete,
            severities: self.severities.clone(),
            severities_complete: self.severities_complete,
        }
        .may_match(predicate)
    }
}

fn load_fresh(path: &Path, size: u64, modified_ns: Option<u64>, sidecar_path: &Path) -> Option<DatasetIndex> {
    let bytes = std::fs::read(sidecar_path).ok()?;
    let disk: DiskIndex = serde_json::from_slice(&bytes).ok()?;
    if disk.version != INDEX_VERSION || disk.source_path != path.display().to_string() || disk.source_size != size || disk.source_modified_ns != modified_ns {
        return None;
    }
    Some(from_disk(disk))
}

fn from_disk(disk: DiskIndex) -> DatasetIndex {
    DatasetIndex {
        summary: IndexSummary {
            size: disk.source_size,
            modified_ns: disk.source_modified_ns,
            first_seq: disk.summary.first_seq,
            last_seq: disk.summary.last_seq,
            first_ts_unix_ns: disk.summary.first_ts_unix_ns,
            last_ts_unix_ns: disk.summary.last_ts_unix_ns,
            record_types: disk.summary.record_types,
            services: disk.summary.services,
            services_complete: disk.summary.services_complete,
            severities: disk.summary.severities,
            severities_complete: disk.summary.severities_complete,
        },
        blocks: disk
            .blocks
            .into_iter()
            .map(|block| IndexBlock {
                offset: block.offset,
                len: block.len,
                first_seq: block.first_seq,
                last_seq: block.last_seq,
                first_ts_unix_ns: block.first_ts_unix_ns,
                last_ts_unix_ns: block.last_ts_unix_ns,
                record_types: block.record_types,
                services: block.services,
                services_complete: block.services_complete,
                severities: block.severities,
                severities_complete: block.severities_complete,
            })
            .collect(),
    }
}

fn build(path: &Path, size: u64, modified_ns: Option<u64>) -> Result<DatasetIndex> {
    let mut file = File::open(path)?;
    let mut blocks = Vec::new();
    let mut summary = SummaryBuilder::default();
    let mut offset = 0u64;

    while offset < size {
        let Some(block) = read_block_summary(&mut file, offset)? else {
            break;
        };
        summary.merge(&block);
        offset = block.offset + block.len;
        blocks.push(block);
    }

    if blocks.is_empty() {
        return Err(logjet::Error::InvalidHeader("missing block sync").into());
    }
    Ok(DatasetIndex { summary: summary.finish(size, modified_ns), blocks })
}

fn persist(path: &Path, sidecar_path: &Path, index: &DatasetIndex) -> Result<()> {
    let disk = DiskIndex {
        version: INDEX_VERSION,
        source_path: path.display().to_string(),
        source_size: index.summary.size,
        source_modified_ns: index.summary.modified_ns,
        summary: DiskSummary {
            first_seq: index.summary.first_seq,
            last_seq: index.summary.last_seq,
            first_ts_unix_ns: index.summary.first_ts_unix_ns,
            last_ts_unix_ns: index.summary.last_ts_unix_ns,
            record_types: index.summary.record_types,
            services: index.summary.services.clone(),
            services_complete: index.summary.services_complete,
            severities: index.summary.severities.clone(),
            severities_complete: index.summary.severities_complete,
        },
        blocks: index
            .blocks
            .iter()
            .map(|block| DiskBlock {
                offset: block.offset,
                len: block.len,
                first_seq: block.first_seq,
                last_seq: block.last_seq,
                first_ts_unix_ns: block.first_ts_unix_ns,
                last_ts_unix_ns: block.last_ts_unix_ns,
                record_types: block.record_types,
                services: block.services.clone(),
                services_complete: block.services_complete,
                severities: block.severities.clone(),
                severities_complete: block.severities_complete,
            })
            .collect(),
    };
    let mut out = File::create(sidecar_path)?;
    out.write_all(&serde_json::to_vec_pretty(&disk).map_err(std::io::Error::other)?)?;
    out.flush()?;
    Ok(())
}

fn read_block_summary(file: &mut File, offset: u64) -> Result<Option<IndexBlock>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut sync = [0u8; 8];
    if file.read(&mut sync)? == 0 {
        return Ok(None);
    }
    if sync != logjet::DEFAULT_SYNC_MARKER {
        return Ok(None);
    }

    let mut fixed = [0u8; BLOCK_HEADER_FIXED_LEN];
    file.read_exact(&mut fixed)?;
    let header = BlockHeader::decode(&fixed)?;
    let ext_len = usize::from(header.header_len).saturating_sub(BLOCK_HEADER_FIXED_LEN);
    let mut ext_bytes = vec![0u8; ext_len];
    file.read_exact(&mut ext_bytes)?;
    let ext = BlockHeaderExt::decode(&ext_bytes[..BLOCK_HEADER_EXT_LEN])?;
    let mut compressed = vec![0u8; header.compressed_len as usize];
    file.read_exact(&mut compressed)?;
    let mut crc = [0u8; 4];
    file.read_exact(&mut crc)?;
    let mut payload = Vec::with_capacity(header.uncompressed_len as usize);
    header.codec.decompress(&compressed, header.uncompressed_len as usize, &mut payload)?;

    let len = u64::from(8u16) + u64::from(header.header_len) + u64::from(header.compressed_len) + 4;
    let mut summary = SummaryBuilder::default();
    parse_block_payload(&payload, ext, &mut |record| summary.push(record))?;

    let summary = summary.finish(len, None);
    Ok(Some(IndexBlock {
        offset,
        len,
        first_seq: summary.first_seq,
        last_seq: summary.last_seq,
        first_ts_unix_ns: summary.first_ts_unix_ns,
        last_ts_unix_ns: summary.last_ts_unix_ns,
        record_types: summary.record_types,
        services: summary.services,
        services_complete: summary.services_complete,
        severities: summary.severities,
        severities_complete: summary.severities_complete,
    }))
}

fn parse_block_payload(payload: &[u8], ext: BlockHeaderExt, f: &mut impl FnMut(OwnedRecord)) -> Result<()> {
    let mut cursor = 0usize;
    while cursor < payload.len() {
        let record_type = RecordType::from_u8(payload[cursor])?;
        cursor += 1;
        let seq = ext.base_seq + decode_varint(payload, &mut cursor)?;
        let ts_unix_ns = ext.base_ts_unix_ns + decode_varint(payload, &mut cursor)?;
        let payload_len = usize::try_from(decode_varint(payload, &mut cursor)?).map_err(|_| logjet::Error::NumericOverflow("payload_len"))?;
        let end = cursor.checked_add(payload_len).ok_or(logjet::Error::NumericOverflow("record payload end"))?;
        let record = OwnedRecord { record_type, seq, ts_unix_ns, payload: payload[cursor..end].to_vec() };
        cursor = end;
        f(record);
    }
    Ok(())
}

fn decode_varint(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while *cursor < bytes.len() {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(logjet::Error::Truncated("varint").into())
}

struct SummaryBuilder {
    first_seq: Option<u64>,
    last_seq: Option<u64>,
    first_ts_unix_ns: Option<u64>,
    last_ts_unix_ns: Option<u64>,
    record_types: u8,
    services: HashSet<String>,
    services_complete: bool,
    severities: HashSet<String>,
    severities_complete: bool,
}

impl Default for SummaryBuilder {
    fn default() -> Self {
        Self {
            first_seq: None,
            last_seq: None,
            first_ts_unix_ns: None,
            last_ts_unix_ns: None,
            record_types: 0,
            services: HashSet::new(),
            services_complete: true,
            severities: HashSet::new(),
            severities_complete: true,
        }
    }
}

impl SummaryBuilder {
    fn push(&mut self, record: OwnedRecord) {
        self.first_seq.get_or_insert(record.seq);
        self.last_seq = Some(record.seq);
        self.first_ts_unix_ns.get_or_insert(record.ts_unix_ns);
        self.last_ts_unix_ns = Some(record.ts_unix_ns);
        self.record_types |= record_type_bit(record.record_type);
        if record.record_type == RecordType::Logs {
            collect_log_summaries(&record.payload, &mut self.services, &mut self.services_complete, &mut self.severities, &mut self.severities_complete);
        }
    }

    fn merge(&mut self, block: &IndexBlock) {
        self.first_seq = self.first_seq.or(block.first_seq);
        self.last_seq = block.last_seq.or(self.last_seq);
        self.first_ts_unix_ns = self.first_ts_unix_ns.or(block.first_ts_unix_ns);
        self.last_ts_unix_ns = block.last_ts_unix_ns.or(self.last_ts_unix_ns);
        self.record_types |= block.record_types;
        merge_set(&mut self.services, &mut self.services_complete, &block.services, block.services_complete);
        merge_set(&mut self.severities, &mut self.severities_complete, &block.severities, block.severities_complete);
    }

    fn finish(self, size: u64, modified_ns: Option<u64>) -> IndexSummary {
        let mut services = self.services.into_iter().collect::<Vec<_>>();
        let mut severities = self.severities.into_iter().collect::<Vec<_>>();
        services.sort();
        severities.sort();
        IndexSummary {
            size,
            modified_ns,
            first_seq: self.first_seq,
            last_seq: self.last_seq,
            first_ts_unix_ns: self.first_ts_unix_ns,
            last_ts_unix_ns: self.last_ts_unix_ns,
            record_types: self.record_types,
            services,
            services_complete: self.services_complete,
            severities,
            severities_complete: self.severities_complete,
        }
    }
}

fn collect_log_summaries(
    payload: &[u8], services: &mut HashSet<String>, services_complete: &mut bool, severities: &mut HashSet<String>, severities_complete: &mut bool,
) {
    let Ok(batch) = ExportLogsServiceRequest::decode(payload) else {
        return;
    };
    for rl in &batch.resource_logs {
        if let Some(resource) = &rl.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(AnyValue { value: Some(Value::StringValue(value)) }) = &attr.value
                {
                    insert_capped(services, services_complete, value);
                }
            }
        }
        for sl in &rl.scope_logs {
            for lr in &sl.log_records {
                if !lr.severity_text.is_empty() {
                    insert_capped(severities, severities_complete, &lr.severity_text);
                }
            }
        }
    }
}

fn insert_capped(set: &mut HashSet<String>, complete: &mut bool, value: &str) {
    if !*complete {
        return;
    }
    if set.len() >= SUMMARY_LIMIT && !set.contains(value) {
        *complete = false;
        return;
    }
    set.insert(value.to_string());
}

fn merge_set(set: &mut HashSet<String>, complete: &mut bool, values: &[String], values_complete: bool) {
    if !*complete {
        return;
    }
    if !values_complete {
        *complete = false;
        return;
    }
    for value in values {
        if set.len() >= SUMMARY_LIMIT && !set.contains(value) {
            *complete = false;
            return;
        }
        set.insert(value.clone());
    }
}

fn record_type_bit(kind: RecordType) -> u8 {
    match kind {
        RecordType::Logs => 1,
        RecordType::Metrics => 2,
        RecordType::Traces => 4,
    }
}

pub(crate) fn read_block_records(path: &Path, block: &IndexBlock) -> Result<Vec<OwnedRecord>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(block.offset))?;
    let mut sync = [0u8; 8];
    file.read_exact(&mut sync)?;
    let mut fixed = [0u8; BLOCK_HEADER_FIXED_LEN];
    file.read_exact(&mut fixed)?;
    let header = BlockHeader::decode(&fixed)?;
    let ext_len = usize::from(header.header_len).saturating_sub(BLOCK_HEADER_FIXED_LEN);
    let mut ext_bytes = vec![0u8; ext_len];
    file.read_exact(&mut ext_bytes)?;
    let ext = BlockHeaderExt::decode(&ext_bytes[..BLOCK_HEADER_EXT_LEN])?;
    let mut compressed = vec![0u8; header.compressed_len as usize];
    file.read_exact(&mut compressed)?;
    let mut crc = [0u8; 4];
    file.read_exact(&mut crc)?;
    let mut payload = Vec::with_capacity(header.uncompressed_len as usize);
    header.codec.decompress(&compressed, header.uncompressed_len as usize, &mut payload)?;
    let mut out = Vec::new();
    parse_block_payload(&payload, ext, &mut |record| out.push(record))?;
    Ok(out)
}

#[cfg(test)]
#[path = "../tests/unit/dataset_index_ut.rs"]
mod dataset_index_ut;
