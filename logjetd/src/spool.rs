use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, ErrorKind};
use std::path::{Path, PathBuf};

use logjet::{LogjetReader, LogjetWriter, OwnedRecord, ReaderConfig};

use crate::config::{BufferConfig, BufferLimit, FileConfig, StorageConfig};
use crate::protocol::{WireRecord, write_record};

#[derive(Debug)]
pub enum Spool {
    Buffer(BufferSpool),
    File(FileSpool),
}

#[derive(Debug)]
pub struct BufferSpool {
    limit: BufferLimit,
    keep_messages: usize,
    records: VecDeque<WireRecord>,
    tail_bytes: usize,
}

#[derive(Debug)]
pub struct FileSpool {
    dir: PathBuf,
    base_stem: String,
    active_segment_id: u64,
    active_segment_path: PathBuf,
    active_writer: LogjetWriter<File>,
    active_size_bytes: u64,
    segment_target_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub id: u64,
    pub path: PathBuf,
}

impl Spool {
    pub fn open(config: StorageConfig) -> io::Result<Self> {
        match config {
            StorageConfig::Buffer(config) => Ok(Self::Buffer(BufferSpool::new(config))),
            StorageConfig::File(config) => Ok(Self::File(FileSpool::open(config)?)),
        }
    }

    pub fn append(&mut self, record: WireRecord) -> io::Result<()> {
        match self {
            Self::Buffer(spool) => {
                spool.append(record);
                Ok(())
            }
            Self::File(spool) => spool.append(record),
        }
    }

    pub fn replay_since<W: io::Write>(&self, writer: &mut W, last_seq: &mut u64) -> io::Result<bool> {
        match self {
            Self::Buffer(spool) => spool.replay_since(writer, last_seq),
            Self::File(spool) => spool.replay_since(writer, last_seq),
        }
    }
}

impl BufferSpool {
    fn new(config: BufferConfig) -> Self {
        Self {
            limit: config.limit,
            keep_messages: config.keep_messages,
            records: VecDeque::new(),
            tail_bytes: 0,
        }
    }

    fn append(&mut self, record: WireRecord) {
        let was_tail = self.records.len() >= self.keep_messages;
        let record_bytes = record_size(&record);
        self.records.push_back(record);
        if was_tail {
            self.tail_bytes = self.tail_bytes.saturating_add(record_bytes);
        }
        self.enforce_limits();
    }

    fn replay_since<W: io::Write>(&self, writer: &mut W, last_seq: &mut u64) -> io::Result<bool> {
        let mut sent_any = false;

        for record in &self.records {
            if record.seq <= *last_seq {
                continue;
            }

            write_record(writer, record)?;
            *last_seq = record.seq;
            sent_any = true;
        }

        Ok(sent_any)
    }

    fn enforce_limits(&mut self) {
        while self.over_limit() && self.records.len() > self.keep_messages {
            let drop_index = self.keep_messages;
            let Some(record) = self.records.remove(drop_index) else {
                break;
            };
            self.tail_bytes = self.tail_bytes.saturating_sub(record_size(&record));
        }
    }

    fn over_limit(&self) -> bool {
        match self.limit {
            BufferLimit::Bytes(max_bytes) => self.tail_bytes > max_bytes,
            BufferLimit::Messages(max_messages) => self.tail_len() > max_messages,
        }
    }

    fn tail_len(&self) -> usize {
        self.records.len().saturating_sub(self.keep_messages)
    }
}

impl FileSpool {
    fn open(config: FileConfig) -> io::Result<Self> {
        fs::create_dir_all(&config.dir)?;

        let base_stem = derive_base_stem(&config.name);
        let segments = list_segments(&config.dir, &base_stem)?;

        let (active_segment_id, active_segment_path, active_size_bytes) = match segments.last() {
            Some(segment) => {
                let size = fs::metadata(&segment.path)?.len();
                if size < config.segment_size_bytes {
                    (segment.id, segment.path.clone(), size)
                } else {
                    let next_id = segment
                        .id
                        .checked_add(1)
                        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "segment id overflow"))?;
                    (next_id, segment_path(&config.dir, &base_stem, next_id), 0)
                }
            }
            None => (0, segment_path(&config.dir, &base_stem, 0), 0),
        };

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_segment_path)?;

        Ok(Self {
            dir: config.dir,
            base_stem,
            active_segment_id,
            active_segment_path,
            active_writer: LogjetWriter::new(file),
            active_size_bytes,
            segment_target_bytes: config.segment_size_bytes,
        })
    }

    fn append(&mut self, record: WireRecord) -> io::Result<()> {
        if self.active_size_bytes >= self.segment_target_bytes && self.active_size_bytes > 0 {
            self.rotate()?;
        }

        self.active_writer
            .push(record.record_type, record.seq, record.ts_unix_ns, &record.payload)
            .map_err(to_io_error)?;
        self.active_writer.flush_block().map_err(to_io_error)?;
        self.refresh_active_size()?;

        if self.active_size_bytes >= self.segment_target_bytes {
            self.rotate()?;
        }

        Ok(())
    }

    fn replay_since<W: io::Write>(&self, writer: &mut W, last_seq: &mut u64) -> io::Result<bool> {
        let mut sent_any = false;

        for segment in list_segments(&self.dir, &self.base_stem)? {
            let file = File::open(&segment.path)?;
            let mut reader = LogjetReader::with_config(BufReader::new(file), ReaderConfig::default());

            while let Some(record) = reader.next_record().map_err(to_io_error)? {
                if record.seq <= *last_seq {
                    continue;
                }

                write_record(
                    writer,
                    &WireRecord {
                        record_type: record.record_type,
                        seq: record.seq,
                        ts_unix_ns: record.ts_unix_ns,
                        payload: record.payload,
                    },
                )?;
                *last_seq = record.seq;
                sent_any = true;
            }
        }

        Ok(sent_any)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.active_writer.flush_block().map_err(to_io_error)?;
        self.active_segment_id = self
            .active_segment_id
            .checked_add(1)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "segment id overflow"))?;
        self.active_segment_path = segment_path(&self.dir, &self.base_stem, self.active_segment_id);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.active_segment_path)?;
        self.active_writer = LogjetWriter::new(file);
        self.active_size_bytes = 0;
        Ok(())
    }

    fn refresh_active_size(&mut self) -> io::Result<()> {
        self.active_size_bytes = fs::metadata(&self.active_segment_path)?.len();
        Ok(())
    }
}

pub fn inspect_path(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() && entry.path().extension().and_then(|ext| ext.to_str()) == Some("logjet") {
                inspect_file(&entry.path())?;
            }
        }
        return Ok(());
    }

    inspect_file(path)
}

fn inspect_file(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let mut reader = LogjetReader::new(BufReader::new(file));

    println!("file={}", path.display());
    while let Some(record) = reader.next_record().map_err(to_io_error)? {
        print_record(&record);
    }
    let stats = reader.stats();
    println!(
        "stats blocks_ok={} blocks_bad={} bytes_skipped={} records_ok={}",
        stats.blocks_ok, stats.blocks_bad, stats.bytes_skipped, stats.records_ok
    );
    Ok(())
}

fn print_record(record: &OwnedRecord) {
    println!(
        "type={:?} seq={} ts={} payload_len={}",
        record.record_type,
        record.seq,
        record.ts_unix_ns,
        record.payload.len()
    );
}

fn list_segments(dir: &Path, base_stem: &str) -> io::Result<Vec<SegmentInfo>> {
    let mut segments = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = parse_segment_id(name, base_stem) else {
            continue;
        };

        segments.push(SegmentInfo { id, path: entry.path() });
    }

    segments.sort_by_key(|segment| segment.id);
    Ok(segments)
}

pub fn list_named_segments(dir: &Path, file_name: &str) -> io::Result<Vec<SegmentInfo>> {
    list_segments(dir, &derive_base_stem(file_name))
}

fn derive_base_stem(file_name: &str) -> String {
    file_name
        .strip_suffix(".logjet")
        .unwrap_or(file_name)
        .to_string()
}

fn parse_segment_id(name: &str, base_stem: &str) -> Option<u64> {
    if name == format!("{base_stem}.logjet") {
        return Some(0);
    }
    let prefix = format!("{base_stem}-");
    let suffix = ".logjet";
    if !name.starts_with(&prefix) || !name.ends_with(suffix) {
        return None;
    }
    let value = &name[prefix.len()..name.len() - suffix.len()];
    value.parse().ok()
}

fn segment_path(dir: &Path, base_stem: &str, id: u64) -> PathBuf {
    if id == 0 {
        dir.join(format!("{base_stem}.logjet"))
    } else {
        dir.join(format!("{base_stem}-{id}.logjet"))
    }
}

fn record_size(record: &WireRecord) -> usize {
    1usize
        .saturating_add(8)
        .saturating_add(8)
        .saturating_add(4)
        .saturating_add(record.payload.len())
}

fn to_io_error(err: logjet::Error) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::BufferSpool;
    use crate::config::{BufferConfig, BufferLimit};
    use crate::protocol::WireRecord;
    use logjet::RecordType;

    #[test]
    fn preserve_prefix_survives_eviction() {
        let mut spool = BufferSpool::new(BufferConfig {
            limit: BufferLimit::Bytes(90),
            keep_messages: 2,
        });

        for seq in 1..=5 {
            spool.append(WireRecord {
                record_type: RecordType::Logs,
                seq,
                ts_unix_ns: seq,
                payload: vec![0u8; 20],
            });
        }

        let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
        assert_eq!(&kept[..2], &[1, 2]);
    }

    #[test]
    fn message_limit_rotates_tail_only() {
        let mut spool = BufferSpool::new(BufferConfig {
            limit: BufferLimit::Messages(4),
            keep_messages: 2,
        });

        for seq in 1..=8 {
            spool.append(WireRecord {
                record_type: logjet::RecordType::Logs,
                seq,
                ts_unix_ns: seq,
                payload: vec![0u8; 8],
            });
        }

        let kept: Vec<u64> = spool.records.iter().map(|record| record.seq).collect();
        assert_eq!(kept, vec![1, 2, 5, 6, 7, 8]);
    }
}
