mod config;
mod daemon;
mod protocol;
mod replay;
mod spool;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use config::Config;
use daemon::{DaemonConfig, serve};
use replay::{replay_path_to_otlp_http, validate_replay_path};
use spool::inspect_path;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("logjetd: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args();
    let _program = args.next();
    let mut config_path = PathBuf::from("/etc/logjet.conf");
    let mut command = None;
    let mut command_arg = None;
    let mut replay_path = None;
    let mut replay_name = None;
    let mut replay_dest = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            "-c" | "--config" => {
                config_path = PathBuf::from(args.next().ok_or("missing config path")?);
            }
            "serve" => {
                command = Some("serve");
                break;
            }
            "inspect" => {
                command = Some("inspect");
                command_arg = args.next();
                break;
            }
            "replay" => {
                command = Some("replay");
                while let Some(flag) = args.next() {
                    match flag.as_str() {
                        "--path" => replay_path = Some(PathBuf::from(args.next().ok_or("missing value for --path")?)),
                        "--name" => replay_name = Some(args.next().ok_or("missing value for --name")?),
                        "--dest" => replay_dest = Some(args.next().ok_or("missing value for --dest")?),
                        _ => return Err(format!("unknown replay argument: {flag}").into()),
                    }
                }
                break;
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }

    match command {
        Some("serve") | None => {
            let config = Config::load(&config_path)?;
            serve(DaemonConfig {
                config,
                config_path,
            })?;
        }
        Some("inspect") => {
            let path = PathBuf::from(command_arg.ok_or("missing path")?);
            inspect_path(&path)?;
        }
        Some("replay") => {
            let config = Config::load(&config_path)?;
            let path = replay_path.ok_or("missing --path")?;
            let name = replay_name.ok_or("missing --name")?;
            let mut collector = config.collector;
            if let Some(dest) = replay_dest {
                collector.url = dest;
            }
            let files = validate_replay_path(&path, &name)?;
            if files.is_empty() {
                return Err(format!(
                    "no replay files found for name '{}' in {}",
                    name,
                    path.display()
                )
                .into());
            }

            eprintln!(
                "replaying {} file(s) from {} to {}",
                files.len(),
                path.display(),
                collector.url
            );
            let sent = replay_path_to_otlp_http(&path, &name, &collector)?;
            eprintln!("replayed {sent} OTLP log batch record(s)");
        }
        Some(other) => return Err(format!("unknown command: {other}").into()),
    }

    Ok(())
}

fn print_usage() {
    println!("logjetd");
    println!();
    println!("Usage:");
    println!("  logjetd [serve] [-c|--config <path>]");
    println!("  logjetd inspect <path>");
    println!("  logjetd replay [-c|--config <path>] --path <dir> --name <base.logjet> [--dest <url-or-host:port>]");
    println!();
    println!("Commands:");
    println!("  serve    Run ingest and replay listeners using YAML configuration");
    println!("  inspect  Print stored record metadata from a .logjet file or spool directory");
    println!("  replay   Read ordered .logjet files and blast OTLP log batches to an OTLP/HTTP collector");
    println!();
    println!("Config path defaults to /etc/logjet.conf.");
}
