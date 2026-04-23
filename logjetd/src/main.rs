mod cli;
mod config;
mod daemon;
mod plugin;
mod protocol;
mod replay;
mod spool;
mod tls;

use std::process::ExitCode;

use clap::FromArgMatches;
use cli::{Cli, Command};
use config::Config;
use daemon::{DaemonConfig, serve};
use replay::{bridge_wire_to_otlp_http, replay_path_to_otlp_http, validate_replay_path};
use spool::{inspect_path, print_named_segments, prune_named_segments};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("ljd: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut command = cli::build_cli();
    let mut matches = command.get_matches_mut();
    let cli = Cli::from_arg_matches_mut(&mut matches)?;

    if cli.plugins {
        let config = Config::load(&cli.config)?;
        plugin::print_visible_plugins(config.ingest_plugin_dir.as_deref(), config.ingest_plugin_path.as_deref())?;
        return Ok(());
    }

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            let config = Config::load(&cli.config)?;
            serve(DaemonConfig { config, config_path: cli.config.clone() })?;
        }
        Command::Inspect(args) => {
            inspect_path(&args.path, args.verify_otlp)?;
        }
        Command::Segments(args) => {
            print_named_segments(&args.path, &args.name)?;
        }
        Command::Replay(args) => {
            let config = Config::load(&cli.config)?;
            let path = args.path;
            let name = args.name;
            let mut collector = config.collector;
            if let Some(dest) = args.dest {
                collector.urls = vec![dest];
            }
            let files = validate_replay_path(&path, &name)?;
            if files.is_empty() {
                return Err(format!("no replay files found for name '{}' in {}", name, path.display()).into());
            }

            eprintln!("replaying {} file(s) from {} to {}", files.len(), path.display(), collector.describe_urls());
            let sent = replay_path_to_otlp_http(&path, &name, &collector)?;
            eprintln!("replayed {sent} OTLP log batch record(s)");
        }
        Command::Bridge(args) => {
            let config = Config::load(&cli.config)?;
            let source = match args.source.or_else(|| config.upstream.replay_addr.clone()) {
                Some(source) => source,
                None => return Err("missing bridge source; set --source or upstream.replay".into()),
            };

            eprintln!("bridging from {} to {}", source, config.collector.describe_urls());
            bridge_wire_to_otlp_http(&source, &config.collector, &config.backpressure, &config.upstream, &config.tls)?;
        }
        Command::Prune(args) => {
            let path = args.path;
            let name = args.name;
            let removed = prune_named_segments(&path, &name, args.keep_files, args.keep_bytes, args.dry_run)?;
            for entry in &removed {
                if args.dry_run {
                    println!("would_remove={}", entry.display());
                } else {
                    println!("removed={}", entry.display());
                }
            }
            println!("removed_count={}", removed.len());
        }
    }

    Ok(())
}
