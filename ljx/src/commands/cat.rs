use crate::cli::CatArgs;
use crate::error::{Error, Result};

pub fn run(_args: CatArgs) -> Result<()> {
    Err(Error::Unimplemented(
        "cat is not implemented yet; the CLI shape is defined but record rendering still needs to be added",
    ))
}
