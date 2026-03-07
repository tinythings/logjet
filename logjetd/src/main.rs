mod config;
mod daemon;
mod protocol;
mod spool;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use config::Config;
use daemon::{DaemonConfig, serve};
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
    println!();
    println!("Commands:");
    println!("  serve    Run ingest and replay listeners using YAML configuration");
    println!("  inspect  Print stored record metadata from a .logjet file or spool directory");
    println!();
    println!("Config path defaults to /etc/logjet.conf.");
}
