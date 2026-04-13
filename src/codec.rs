use crate::error::{Error, Result};

/// Block payload compression codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    None = 0,
    Lz4 = 1,
    Zstd = 2,
}

impl Codec {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Zstd),
            other => Err(Error::UnknownCodec(other)),
        }
    }

    pub fn compress(self, input: &[u8], output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        match self {
            Self::None => output.extend_from_slice(input),
            Self::Lz4 => output.extend_from_slice(&lz4_flex::block::compress(input)),
            Self::Zstd => {
                let compressed = zstd::bulk::compress(input, 3).map_err(|err| Error::Codec(err.to_string()))?;
                output.extend_from_slice(&compressed);
            }
        }
        Ok(())
    }

    pub fn decompress(self, input: &[u8], expected_len: usize, output: &mut Vec<u8>) -> Result<()> {
        output.clear();
        match self {
            Self::None => {
                if input.len() != expected_len {
                    return Err(Error::InvalidHeader("raw payload length mismatch"));
                }
                output.extend_from_slice(input);
            }
            Self::Lz4 => {
                let decoded = lz4_flex::block::decompress(input, expected_len).map_err(|err| Error::Codec(err.to_string()))?;
                output.extend_from_slice(&decoded);
            }
            Self::Zstd => {
                let decoded = zstd::bulk::decompress(input, expected_len).map_err(|err| Error::Codec(err.to_string()))?;
                output.extend_from_slice(&decoded);
            }
        }
        Ok(())
    }
}
