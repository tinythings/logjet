use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordType {
    Logs = 1,
    Metrics = 2,
    Traces = 3,
}

impl RecordType {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Logs),
            2 => Ok(Self::Metrics),
            3 => Ok(Self::Traces),
            other => Err(Error::InvalidRecordType(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub record_type: RecordType,
    pub seq: u64,
    pub ts_unix_ns: u64,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedRecord {
    pub record_type: RecordType,
    pub seq: u64,
    pub ts_unix_ns: u64,
    pub payload: Vec<u8>,
}

impl OwnedRecord {
    pub fn as_record(&self) -> Record<'_> {
        Record {
            record_type: self.record_type,
            seq: self.seq,
            ts_unix_ns: self.ts_unix_ns,
            payload: &self.payload,
        }
    }
}

impl<'a> From<Record<'a>> for OwnedRecord {
    fn from(value: Record<'a>) -> Self {
        Self {
            record_type: value.record_type,
            seq: value.seq,
            ts_unix_ns: value.ts_unix_ns,
            payload: value.payload.to_vec(),
        }
    }
}
