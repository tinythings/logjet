use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Seek, SeekFrom};

use crate::crc::crc32c;
use crate::error::{Error, Result};
use crate::format::{
    BLOCK_HEADER_EXT_LEN, BLOCK_HEADER_FIXED_LEN, BLOCK_HEADER_TOTAL_LEN, BlockHeader,
    BlockHeaderExt, DEFAULT_MAX_BLOCK_SIZE, DEFAULT_SYNC_MARKER,
};
use crate::record::{OwnedRecord, RecordType};

#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub sync_marker: [u8; 8],
    pub max_compressed_len: usize,
    pub max_uncompressed_len: usize,
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            sync_marker: DEFAULT_SYNC_MARKER,
            max_compressed_len: DEFAULT_MAX_BLOCK_SIZE,
            max_uncompressed_len: DEFAULT_MAX_BLOCK_SIZE,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReaderStats {
    pub blocks_ok: u64,
    pub blocks_bad: u64,
    pub bytes_skipped: u64,
    pub records_ok: u64,
}

#[derive(Debug)]
pub struct LogjetReader<R: Read + Seek> {
    #[allow(dead_code)]
    inner: R,
    #[allow(dead_code)]
    config: ReaderConfig,
    #[allow(dead_code)]
    stats: ReaderStats,
    #[allow(dead_code)]
    pending: VecDeque<OwnedRecord>,
}

impl<R: Read + Seek> LogjetReader<R> {
    pub fn new(inner: R) -> Self {
        Self::with_config(inner, ReaderConfig::default())
    }

    pub fn with_config(inner: R, config: ReaderConfig) -> Self {
        Self {
            inner,
            config,
            stats: ReaderStats::default(),
            pending: VecDeque::new(),
        }
    }

    pub fn next_record(&mut self) -> Result<Option<OwnedRecord>> {
        loop {
            if let Some(record) = self.pending.pop_front() {
                self.stats.records_ok = self
                    .stats
                    .records_ok
                    .checked_add(1)
                    .ok_or(Error::NumericOverflow("records_ok"))?;
                return Ok(Some(record));
            }

            if !self.scan_next_block()? {
                return Ok(None);
            }
        }
    }

    pub fn scan_next_block(&mut self) -> Result<bool> {
        let scan_origin = self.inner.stream_position()?;

        loop {
            let Some(sync_start) = self.find_next_sync()? else {
                let eof = self.inner.seek(SeekFrom::End(0))?;
                self.stats.bytes_skipped = self
                    .stats
                    .bytes_skipped
                    .checked_add(eof.saturating_sub(scan_origin))
                    .ok_or(Error::NumericOverflow("bytes_skipped"))?;
                self.inner.seek(SeekFrom::Start(eof))?;
                return Ok(false);
            };

            match self.try_read_block_at(sync_start) {
                Ok(records) => {
                    self.stats.blocks_ok = self
                        .stats
                        .blocks_ok
                        .checked_add(1)
                        .ok_or(Error::NumericOverflow("blocks_ok"))?;
                    self.stats.bytes_skipped = self
                        .stats
                        .bytes_skipped
                        .checked_add(sync_start.saturating_sub(scan_origin))
                        .ok_or(Error::NumericOverflow("bytes_skipped"))?;
                    self.pending.extend(records);
                    return Ok(true);
                }
                Err(Error::Io(err)) if err.kind() == ErrorKind::UnexpectedEof => {
                    self.stats.blocks_bad = self
                        .stats
                        .blocks_bad
                        .checked_add(1)
                        .ok_or(Error::NumericOverflow("blocks_bad"))?;
                    self.inner.seek(SeekFrom::Start(sync_start + 1))?;
                }
                Err(Error::Io(err)) => return Err(Error::Io(err)),
                Err(_) => {
                    self.stats.blocks_bad = self
                        .stats
                        .blocks_bad
                        .checked_add(1)
                        .ok_or(Error::NumericOverflow("blocks_bad"))?;
                    self.inner.seek(SeekFrom::Start(sync_start + 1))?;
                }
            }
        }
    }

    pub fn stats(&self) -> ReaderStats {
        self.stats
    }

    fn find_next_sync(&mut self) -> Result<Option<u64>> {
        let start = self.inner.stream_position()?;
        let mut matched = 0usize;
        let mut offset = 0u64;
        let mut byte = [0u8; 1];

        loop {
            let read = self.inner.read(&mut byte)?;
            if read == 0 {
                return Ok(None);
            }

            let candidate = byte[0];
            if candidate == self.config.sync_marker[matched] {
                matched += 1;
                if matched == self.config.sync_marker.len() {
                    let sync_start = start + offset + 1 - self.config.sync_marker.len() as u64;
                    self.inner.seek(SeekFrom::Start(sync_start))?;
                    return Ok(Some(sync_start));
                }
            } else {
                matched = usize::from(candidate == self.config.sync_marker[0]);
            }

            offset += 1;
        }
    }

    fn try_read_block_at(&mut self, sync_start: u64) -> Result<VecDeque<OwnedRecord>> {
        self.inner.seek(SeekFrom::Start(sync_start))?;

        let mut sync = [0u8; 8];
        self.inner.read_exact(&mut sync)?;
        if sync != self.config.sync_marker {
            return Err(Error::InvalidHeader("sync mismatch"));
        }

        let mut fixed = [0u8; BLOCK_HEADER_FIXED_LEN];
        self.inner.read_exact(&mut fixed)?;
        let header = BlockHeader::decode(&fixed)?;
        if usize::from(header.header_len) < BLOCK_HEADER_TOTAL_LEN {
            return Err(Error::HeaderTooShort(header.header_len));
        }
        let header_len = usize::from(header.header_len);
        let ext_len = header_len
            .checked_sub(BLOCK_HEADER_FIXED_LEN)
            .ok_or(Error::InvalidHeader("header length underflow"))?;
        if ext_len < BLOCK_HEADER_EXT_LEN {
            return Err(Error::InvalidHeader("missing required header extension"));
        }

        validate_length(
            "compressed_len",
            header.compressed_len as usize,
            self.config.max_compressed_len,
        )?;
        validate_length(
            "uncompressed_len",
            header.uncompressed_len as usize,
            self.config.max_uncompressed_len,
        )?;

        let mut ext_bytes = vec![0u8; ext_len];
        self.inner.read_exact(&mut ext_bytes)?;
        let expected_header_crc = header.compute_header_crc(&ext_bytes);
        if expected_header_crc != header.header_crc32c {
            return Err(Error::HeaderCrcMismatch {
                expected: header.header_crc32c,
                actual: expected_header_crc,
            });
        }
        let ext = BlockHeaderExt::decode(&ext_bytes[..BLOCK_HEADER_EXT_LEN])?;

        let mut compressed = vec![0u8; header.compressed_len as usize];
        self.inner.read_exact(&mut compressed)?;

        let mut block_crc_bytes = [0u8; 4];
        self.inner.read_exact(&mut block_crc_bytes)?;
        let expected_block_crc = u32::from_le_bytes(block_crc_bytes);

        let mut crc_bytes =
            Vec::with_capacity(BLOCK_HEADER_FIXED_LEN + ext_bytes.len() + compressed.len());
        crc_bytes.extend_from_slice(&fixed);
        crc_bytes.extend_from_slice(&ext_bytes);
        crc_bytes.extend_from_slice(&compressed);
        let actual_block_crc = crc32c(&crc_bytes);
        if expected_block_crc != actual_block_crc {
            return Err(Error::BlockCrcMismatch {
                expected: expected_block_crc,
                actual: actual_block_crc,
            });
        }

        let mut payload = Vec::with_capacity(header.uncompressed_len as usize);
        header
            .codec
            .decompress(&compressed, header.uncompressed_len as usize, &mut payload)?;

        let records = parse_records(&payload, header.record_count, ext)?;
        Ok(records)
    }
}

fn validate_length(field: &'static str, value: usize, limit: usize) -> Result<()> {
    if value > limit {
        return Err(Error::LengthTooLarge {
            field,
            value: value as u64,
            limit,
        });
    }
    Ok(())
}

fn parse_records(
    payload: &[u8],
    record_count: u32,
    ext: BlockHeaderExt,
) -> Result<VecDeque<OwnedRecord>> {
    let mut records = VecDeque::with_capacity(record_count as usize);
    let mut cursor = 0usize;

    for _ in 0..record_count {
        if cursor >= payload.len() {
            return Err(Error::Truncated("record_type"));
        }
        let record_type = RecordType::from_u8(payload[cursor])?;
        cursor += 1;

        let seq_delta = decode_varint(payload, &mut cursor)?;
        let ts_delta = decode_varint(payload, &mut cursor)?;
        let payload_len = decode_varint(payload, &mut cursor)?;
        let payload_len =
            usize::try_from(payload_len).map_err(|_| Error::NumericOverflow("payload_len"))?;

        let end = cursor
            .checked_add(payload_len)
            .ok_or(Error::NumericOverflow("record payload end"))?;
        if end > payload.len() {
            return Err(Error::Truncated("record payload"));
        }

        records.push_back(OwnedRecord {
            record_type,
            seq: ext
                .base_seq
                .checked_add(seq_delta)
                .ok_or(Error::NumericOverflow("record seq"))?,
            ts_unix_ns: ext
                .base_ts_unix_ns
                .checked_add(ts_delta)
                .ok_or(Error::NumericOverflow("record ts"))?,
            payload: payload[cursor..end].to_vec(),
        });
        cursor = end;
    }

    if cursor != payload.len() {
        return Err(Error::InvalidHeader("payload has trailing bytes"));
    }

    Ok(records)
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
        if shift >= 64 {
            return Err(Error::VarintTooLong);
        }
    }

    Err(Error::Truncated("varint"))
}
