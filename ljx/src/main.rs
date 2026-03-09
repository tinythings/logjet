mod cli;
mod commands;
mod error;
mod input;
mod predicate;

use clap::FromArgMatches;

use crate::cli::{Cli, Command};
use crate::error::Result;

fn main() {
    if let Err(err) = run() {
        eprintln!("ljx: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut command = cli::build_cli();
    let mut matches = command.get_matches_mut();
    let cli = Cli::from_arg_matches_mut(&mut matches)
        .map_err(|err| crate::error::Error::Usage(err.to_string()))?;
    match cli.command {
        Command::Split(args) => commands::split::run(args),
        Command::Join(args) => commands::join::run(args),
        Command::Filter(args) => commands::filter::run(args),
        Command::Count(args) => commands::count::run(args),
        Command::Stats(args) => commands::stats::run(args),
        Command::View(args) => commands::view::run(args),
    }
}
