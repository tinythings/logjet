use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum CelError {
    Compile(String),
    Decode(String),
    NotBoolean(String),
}

impl Display for CelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compile(msg) => write!(f, "CEL compilation failed: {msg}"),
            Self::Decode(msg) => write!(f, "OTLP decode failed: {msg}"),
            Self::NotBoolean(val) => write!(f, "CEL result was not boolean: {val}"),
        }
    }
}

impl std::error::Error for CelError {}
