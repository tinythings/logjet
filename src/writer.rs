use std::io::Write;

use crate::codec::Codec;
use crate::crc::crc32c;
use crate::error::{Error, Result};
use crate::format::{
    BLOCK_HEADER_EXT_LEN, BLOCK_HEADER_FIXED_LEN, BLOCK_HEADER_TOTAL_LEN, BlockHeader, BlockHeaderExt, DEFAULT_BLOCK_TARGET_SIZE,
    DEFAULT_SYNC_MARKER, FORMAT_VERSION,
};
use crate::record::RecordType;

#[derive(Debug, Clone)]
pub struct WriterConfig {
    pub block_target_size: usize,
    pub codec: Codec,
    pub sync_marker: [u8; 8],
    /// Pad each block to this alignment (bytes). 0 = no padding.
    pub block_alignment: usize,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self { block_target_size: DEFAULT_BLOCK_TARGET_SIZE, codec: Codec::Lz4, sync_marker: DEFAULT_SYNC_MARKER, block_alignment: 0 }
    }
}

#[derive(Debug)]
pub struct LogjetWriter<W: Write> {
    inner: W,
    config: WriterConfig,
    payload_buf: Vec<u8>,
    encoded_record_buf: Vec<u8>,
    compressed_buf: Vec<u8>,
    block_buf: Vec<u8>,
    block_base_seq: Option<u64>,
    block_base_ts_unix_ns: Option<u64>,
    record_count: u32,
}

impl<W: Write> LogjetWriter<W> {
    pub fn new(inner: W) -> Self {
        Self::with_config(inner, WriterConfig::default())
    }

    pub fn with_config(inner: W, config: WriterConfig) -> Self {
        Self {
            inner,
            config,
            payload_buf: Vec::with_capacity(DEFAULT_BLOCK_TARGET_SIZE),
            encoded_record_buf: Vec::new(),
            compressed_buf: Vec::new(),
            block_buf: Vec::new(),
            block_base_seq: None,
            block_base_ts_unix_ns: None,
            record_count: 0,
        }
    }

    pub fn push(&mut self, record_type: RecordType, seq: u64, ts_unix_ns: u64, payload: &[u8]) -> Result<()> {
        self.encoded_record_buf.clear();

        let (base_seq, base_ts) = match (self.block_base_seq, self.block_base_ts_unix_ns) {
            (Some(base_seq), Some(base_ts)) => (base_seq, base_ts),
            _ => {
                self.block_base_seq = Some(seq);
                self.block_base_ts_unix_ns = Some(ts_unix_ns);
                (seq, ts_unix_ns)
            }
        };

        let seq_delta = seq.checked_sub(base_seq).ok_or(Error::InvalidHeader("sequence must be monotonic within block"))?;
        let ts_delta = ts_unix_ns.checked_sub(base_ts).ok_or(Error::InvalidHeader("timestamp must be monotonic within block"))?;

        self.encoded_record_buf.push(record_type as u8);
        encode_varint(seq_delta, &mut self.encoded_record_buf)?;
        encode_varint(ts_delta, &mut self.encoded_record_buf)?;
        encode_varint(u64::try_from(payload.len()).map_err(|_| Error::NumericOverflow("payload len"))?, &mut self.encoded_record_buf)?;
        self.encoded_record_buf.extend_from_slice(payload);

        let projected = self.payload_buf.len().checked_add(self.encoded_record_buf.len()).ok_or(Error::NumericOverflow("payload buf growth"))?;
        if !self.payload_buf.is_empty() && projected > self.config.block_target_size {
            self.flush_block()?;
            self.block_base_seq = Some(seq);
            self.block_base_ts_unix_ns = Some(ts_unix_ns);
            self.encoded_record_buf.clear();
            self.encoded_record_buf.push(record_type as u8);
            encode_varint(0, &mut self.encoded_record_buf)?;
            encode_varint(0, &mut self.encoded_record_buf)?;
            encode_varint(u64::try_from(payload.len()).map_err(|_| Error::NumericOverflow("payload len"))?, &mut self.encoded_record_buf)?;
            self.encoded_record_buf.extend_from_slice(payload);
        }

        self.payload_buf.extend_from_slice(&self.encoded_record_buf);
        self.record_count = self.record_count.checked_add(1).ok_or(Error::NumericOverflow("record_count"))?;

        if self.payload_buf.len() >= self.config.block_target_size {
            self.flush_block()?;
        }

        Ok(())
    }

    pub fn flush_block(&mut self) -> Result<()> {
        if self.payload_buf.is_empty() {
            return Ok(());
        }

        let base_seq = self.block_base_seq.ok_or(Error::InvalidHeader("missing block base seq"))?;
        let base_ts = self.block_base_ts_unix_ns.ok_or(Error::InvalidHeader("missing block base ts"))?;

        self.config.codec.compress(&self.payload_buf, &mut self.compressed_buf)?;

        let uncompressed_len = u32::try_from(self.payload_buf.len()).map_err(|_| Error::LengthTooLarge {
            field: "uncompressed_len",
            value: self.payload_buf.len() as u64,
            limit: u32::MAX as usize,
        })?;
        let compressed_len = u32::try_from(self.compressed_buf.len()).map_err(|_| Error::LengthTooLarge {
            field: "compressed_len",
            value: self.compressed_buf.len() as u64,
            limit: u32::MAX as usize,
        })?;

        let header_ext = BlockHeaderExt { base_seq, base_ts_unix_ns: base_ts };
        let mut ext_bytes = Vec::with_capacity(BLOCK_HEADER_EXT_LEN);
        header_ext.encode(&mut ext_bytes);

        let mut header = BlockHeader {
            version: FORMAT_VERSION,
            codec: self.config.codec,
            flags: 0,
            header_len: u16::try_from(BLOCK_HEADER_TOTAL_LEN).map_err(|_| Error::NumericOverflow("header_len"))?,
            reserved: 0,
            uncompressed_len,
            compressed_len,
            record_count: self.record_count,
            header_crc32c: 0,
        };
        header.header_crc32c = header.compute_header_crc(&ext_bytes);

        self.block_buf.clear();
        self.block_buf.extend_from_slice(&self.config.sync_marker);
        header.encode(&mut self.block_buf);
        self.block_buf.extend_from_slice(&ext_bytes);
        self.block_buf.extend_from_slice(&self.compressed_buf);

        let crc = crc32c(&self.block_buf[self.config.sync_marker.len()..]);
        self.block_buf.extend_from_slice(&crc.to_le_bytes());

        self.inner.write_all(&self.block_buf)?;

        if self.config.block_alignment > 0 {
            let remainder = self.block_buf.len() % self.config.block_alignment;
            if remainder > 0 {
                let pad = self.config.block_alignment - remainder;
                let zeroes = vec![0u8; pad];
                self.inner.write_all(&zeroes)?;
            }
        }

        self.reset_block_state();
        Ok(())
    }

    pub fn into_inner(mut self) -> Result<W> {
        self.flush_block()?;
        Ok(self.inner)
    }

    /// Bytes buffered in the current unflushed block.
    pub fn pending_bytes(&self) -> usize {
        self.payload_buf.len()
    }

    /// Mutable reference to the underlying writer (e.g. for fsync).
    pub fn inner_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    fn reset_block_state(&mut self) {
        self.payload_buf.clear();
        self.encoded_record_buf.clear();
        self.compressed_buf.clear();
        self.block_base_seq = None;
        self.block_base_ts_unix_ns = None;
        self.record_count = 0;
    }
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) -> Result<()> {
    while value >= 0x80 {
        let low = u8::try_from(value & 0x7f).map_err(|_| Error::NumericOverflow("varint byte"))?;
        output.push(low | 0x80);
        value >>= 7;
    }
    output.push(u8::try_from(value).map_err(|_| Error::NumericOverflow("varint final byte"))?);
    Ok(())
}

#[allow(dead_code)]
const _: usize = BLOCK_HEADER_FIXED_LEN;
