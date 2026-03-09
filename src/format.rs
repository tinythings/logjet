//! Binary format definitions.
//!
//! A file is a sequence of blocks:
//! 1. 8-byte sync marker
//! 2. fixed-size block header
//! 3. fixed header extension containing `base_seq` and `base_ts_unix_ns`
//! 4. compressed or raw payload bytes
//! 5. trailing CRC32C covering header + extension + payload
//!
//! Each uncompressed block payload is a concatenation of records:
//! 1. `record_type: u8`
//! 2. `seq_delta: uvarint`
//! 3. `ts_delta_ns: uvarint`
//! 4. `payload_len: uvarint`
//! 5. raw OTLP protobuf payload bytes
//!
//! Recovery rules:
//! readers scan for the next sync marker, validate the header and CRCs, and if
//! validation fails resume scanning from the next byte after the rejected sync.

use crate::codec::Codec;
use crate::crc::crc32c;
use crate::error::{Error, Result};

pub const FORMAT_VERSION: u8 = 1;
pub const DEFAULT_SYNC_MARKER: [u8; 8] = *b"OTLPBLK!";
pub const DEFAULT_BLOCK_TARGET_SIZE: usize = 64 * 1024;
pub const DEFAULT_MAX_BLOCK_SIZE: usize = 16 * 1024 * 1024;

pub const BLOCK_HEADER_FIXED_LEN: usize = 24;
pub const BLOCK_HEADER_EXT_LEN: usize = 16;
pub const BLOCK_HEADER_TOTAL_LEN: usize = BLOCK_HEADER_FIXED_LEN + BLOCK_HEADER_EXT_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeader {
    pub version: u8,
    pub codec: Codec,
    pub flags: u16,
    pub header_len: u16,
    pub reserved: u16,
    pub uncompressed_len: u32,
    pub compressed_len: u32,
    pub record_count: u32,
    pub header_crc32c: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeaderExt {
    pub base_seq: u64,
    pub base_ts_unix_ns: u64,
}

impl BlockHeader {
    pub fn encode_without_crc(&self, output: &mut Vec<u8>) {
        output.push(self.version);
        output.push(self.codec as u8);
        output.extend_from_slice(&self.flags.to_le_bytes());
        output.extend_from_slice(&self.header_len.to_le_bytes());
        output.extend_from_slice(&self.reserved.to_le_bytes());
        output.extend_from_slice(&self.uncompressed_len.to_le_bytes());
        output.extend_from_slice(&self.compressed_len.to_le_bytes());
        output.extend_from_slice(&self.record_count.to_le_bytes());
    }

    pub fn encode(&self, output: &mut Vec<u8>) {
        self.encode_without_crc(output);
        output.extend_from_slice(&self.header_crc32c.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < BLOCK_HEADER_FIXED_LEN {
            return Err(Error::Truncated("block header"));
        }

        let version = bytes[0];
        if version != FORMAT_VERSION {
            return Err(Error::UnknownVersion(version));
        }

        let codec = Codec::from_u8(bytes[1])?;
        let flags = u16::from_le_bytes([bytes[2], bytes[3]]);
        let header_len = u16::from_le_bytes([bytes[4], bytes[5]]);
        let reserved = u16::from_le_bytes([bytes[6], bytes[7]]);
        let uncompressed_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let compressed_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let record_count = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let header_crc32c = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);

        Ok(Self {
            version,
            codec,
            flags,
            header_len,
            reserved,
            uncompressed_len,
            compressed_len,
            record_count,
            header_crc32c,
        })
    }

    pub fn compute_header_crc(&self, ext_bytes: &[u8]) -> u32 {
        let mut bytes = Vec::with_capacity(BLOCK_HEADER_TOTAL_LEN - 4);
        self.encode_without_crc(&mut bytes);
        bytes.extend_from_slice(ext_bytes);
        crc32c(&bytes)
    }
}

impl BlockHeaderExt {
    pub fn encode(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.base_seq.to_le_bytes());
        output.extend_from_slice(&self.base_ts_unix_ns.to_le_bytes());
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < BLOCK_HEADER_EXT_LEN {
            return Err(Error::Truncated("block header extension"));
        }

        Ok(Self {
            base_seq: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            base_ts_unix_ns: u64::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ]),
        })
    }
}
