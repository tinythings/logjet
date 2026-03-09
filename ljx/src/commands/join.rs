use crate::cli::JoinArgs;
use crate::error::{Error, Result};

pub fn run(_args: JoinArgs) -> Result<()> {
    Err(Error::Unimplemented("join is not implemented yet; the CLI shape is defined but ordered multi-segment merging still needs to be added"))
}
