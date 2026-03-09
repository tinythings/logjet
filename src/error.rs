use std::fmt::{Display, Formatter};
use std::io;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidRecordType(u8),
    UnknownCodec(u8),
    UnknownVersion(u8),
    HeaderTooShort(u16),
    HeaderCrcMismatch { expected: u32, actual: u32 },
    BlockCrcMismatch { expected: u32, actual: u32 },
    LengthTooLarge { field: &'static str, value: u64, limit: usize },
    InvalidHeader(&'static str),
    Truncated(&'static str),
    VarintTooLong,
    NumericOverflow(&'static str),
    RecordTooLarge { encoded_len: usize, block_target_size: usize },
    Codec(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::InvalidRecordType(value) => write!(f, "invalid record type: {value}"),
            Self::UnknownCodec(value) => write!(f, "unknown codec: {value}"),
            Self::UnknownVersion(value) => write!(f, "unknown version: {value}"),
            Self::HeaderTooShort(value) => write!(f, "header too short: {value}"),
            Self::HeaderCrcMismatch { expected, actual } => {
                write!(f, "header crc mismatch: expected {expected:#010x}, got {actual:#010x}")
            }
            Self::BlockCrcMismatch { expected, actual } => {
                write!(f, "block crc mismatch: expected {expected:#010x}, got {actual:#010x}")
            }
            Self::LengthTooLarge { field, value, limit } => {
                write!(f, "{field} too large: {value} > {limit}")
            }
            Self::InvalidHeader(msg) => write!(f, "invalid header: {msg}"),
            Self::Truncated(msg) => write!(f, "truncated data: {msg}"),
            Self::VarintTooLong => write!(f, "varint too long"),
            Self::NumericOverflow(field) => write!(f, "numeric overflow: {field}"),
            Self::RecordTooLarge { encoded_len, block_target_size } => {
                write!(f, "record too large for block target: {encoded_len} > {block_target_size}")
            }
            Self::Codec(msg) => write!(f, "codec error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}
