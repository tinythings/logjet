mod cli;
mod commands;
mod dataset;
mod dataset_index;
mod dedup;
mod error;
mod exporter;
mod input;
mod predicate;

use clap::FromArgMatches;

use crate::cli::{Cli, Command, ExportFormat};
use crate::error::{Error, Result};

fn main() {
    if let Err(err) = run() {
        if err.is_machine_readable() {
            eprintln!("{err}");
        } else {
            eprintln!("ljx: {err}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut command = cli::build_cli();
    if std::env::args_os().len() == 1 {
        command.print_long_help()?;
        println!();
        return Ok(());
    }
    let mut matches = command.get_matches_mut();
    let cli = Cli::from_arg_matches_mut(&mut matches).map_err(|err| crate::error::Error::Usage(err.to_string()))?;
    if let Some(format) = cli.export.as_deref() {
        let input = cli.input.ok_or_else(|| Error::Usage("missing export input; use `ljx --export <format> <input> -o <output>`".to_string()))?;
        let output = cli.output.ok_or_else(|| Error::Usage("missing export output; use `ljx --export <format> <input> -o <output>`".to_string()))?;
        return commands::export::run(format, &input, &output, cli.force, &cli.fields);
    }
    if let Some(input) = cli.input.as_deref() {
        let predicate = cli.predicate.build()?;
        return match cli.format.unwrap_or(ExportFormat::Ndjson) {
            ExportFormat::Ndjson => commands::export::run_query_ndjson(input, &predicate, &cli.fields),
        };
    }
    if cli.format.is_some() || cli.predicate.has_filters() {
        return Err(Error::Usage("missing query input; use `ljx <input> [--format ndjson] [FILTER OPTIONS]`".to_string()));
    }

    match cli.command {
        Some(Command::Split(args)) => commands::split::run(args),
        Some(Command::Join(args)) => commands::join::run(args),
        Some(Command::Filter(args)) => commands::filter::run(args),
        Some(Command::Count(args)) => commands::count::run(args),
        Some(Command::Stats(args)) => commands::stats::run(args),
        Some(Command::Discover(args)) => commands::discover::run(args),
        Some(Command::View(args)) => commands::view::run(args),
        Some(Command::Dedup(args)) => commands::dedup::run(args),
        None => {
            command.print_long_help()?;
            println!();
            Ok(())
        }
    }
}
