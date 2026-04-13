//! Append-only log container for OTLP protobuf batches.
//!
//! Blocks are independently checksummed and optionally compressed so readers can
//! resynchronise after corruption and continue replaying later valid data.

pub mod codec;
pub mod crc;
pub mod error;
pub mod format;
pub mod reader;
pub mod record;
pub mod writer;

pub use codec::Codec;
pub use error::{Error, Result};
pub use format::{
    BLOCK_HEADER_EXT_LEN, BLOCK_HEADER_FIXED_LEN, BLOCK_HEADER_TOTAL_LEN, BlockHeader, BlockHeaderExt, DEFAULT_BLOCK_TARGET_SIZE,
    DEFAULT_MAX_BLOCK_SIZE, DEFAULT_SYNC_MARKER, FORMAT_VERSION,
};
pub use reader::{LogjetReader, ReaderConfig, ReaderStats};
pub use record::{OwnedRecord, Record, RecordType};
pub use writer::{LogjetWriter, WriterConfig};

#[cfg(test)]
#[path = "writer_utst.rs"]
mod writer_utst;
