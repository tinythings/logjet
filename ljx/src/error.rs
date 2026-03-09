use std::fmt::{Display, Formatter};
use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Logjet(logjet::Error),
    Usage(String),
    Unimplemented(&'static str),
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Logjet(err) => write!(f, "{err}"),
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::Unimplemented(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<logjet::Error> for Error {
    fn from(value: logjet::Error) -> Self {
        Self::Logjet(value)
    }
}
