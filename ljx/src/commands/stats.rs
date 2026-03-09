use crate::cli::StatsArgs;
use crate::error::{Error, Result};

pub fn run(_args: StatsArgs) -> Result<()> {
    Err(Error::Unimplemented(
        "stats is not implemented yet; the CLI shape is defined but streaming summaries still need to be added",
    ))
}
