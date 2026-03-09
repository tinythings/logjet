use crate::cli::SplitArgs;
use crate::error::{Error, Result};

pub fn run(_args: SplitArgs) -> Result<()> {
    Err(Error::Unimplemented(
        "split is not implemented yet; the CLI shape is defined but block-preserving file partitioning still needs to be added",
    ))
}
