mod cli;
mod commands;
mod dedup;
mod error;
mod exporter;
mod input;
mod predicate;

use clap::FromArgMatches;

use crate::cli::{Cli, Command};
use crate::error::{Error, Result};

fn main() {
    if let Err(err) = run() {
        eprintln!("ljx: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut command = cli::build_cli();
    let mut matches = command.get_matches_mut();
    let cli = Cli::from_arg_matches_mut(&mut matches).map_err(|err| crate::error::Error::Usage(err.to_string()))?;
    if let Some(format) = cli.export.as_deref() {
        let input = cli.input.ok_or_else(|| Error::Usage("missing export input; use `ljx --export <format> <input> -o <output>`".to_string()))?;
        let output = cli.output.ok_or_else(|| Error::Usage("missing export output; use `ljx --export <format> <input> -o <output>`".to_string()))?;
        return commands::export::run(format, &input, &output, cli.force);
    }

    match cli.command {
        Some(Command::Split(args)) => commands::split::run(args),
        Some(Command::Join(args)) => commands::join::run(args),
        Some(Command::Filter(args)) => commands::filter::run(args),
        Some(Command::Count(args)) => commands::count::run(args),
        Some(Command::Stats(args)) => commands::stats::run(args),
        Some(Command::View(args)) => commands::view::run(args),
        Some(Command::Dedup(args)) => commands::dedup::run(args),
        None => {
            command.print_long_help()?;
            println!();
            Ok(())
        }
    }
}
