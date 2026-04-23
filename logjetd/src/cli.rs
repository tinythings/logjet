use std::path::PathBuf;

use clap::builder::styling;
use clap::{Args, CommandFactory, Parser, Subcommand};
use colored::Colorize;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/logjet.conf";

#[derive(Debug, Parser)]
#[command(
    name = "ljd",
    version,
    about = "OTLP ingest, .logjet storage, replay, and pruning daemon",
    long_about = None
)]
pub struct Cli {
    #[arg(
        short = 'c',
        long = "config",
        value_name = "PATH",
        global = true,
        default_value = DEFAULT_CONFIG_PATH,
        help = "YAML config file to load"
    )]
    pub config: PathBuf,

    #[arg(long = "plugins", global = true, help = "List visible ingest and export plugins, then exit")]
    pub plugins: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Run ingest and replay listeners using YAML configuration")]
    Serve,
    #[command(about = "Print stored record metadata from a .logjet file or spool directory")]
    Inspect(InspectArgs),
    #[command(about = "Print ordered file-segment metadata for one rotated .logjet spool")]
    Segments(SegmentsArgs),
    #[command(about = "Read ordered .logjet files and blast OTLP log batches to an OTLP/HTTP collector")]
    Replay(ReplayArgs),
    #[command(about = "Remove oldest rotated file segments by count or total bytes")]
    Prune(PruneArgs),
    #[command(about = "Connect to a replay listener, drain backlog, stay attached, and forward OTLP logs")]
    Bridge(BridgeArgs),
}

#[derive(Debug, Args)]
pub struct InspectArgs {
    #[arg(long = "verify-otlp", help = "Decode OTLP payloads and include verification summary")]
    pub verify_otlp: bool,

    #[arg(value_name = "PATH", help = "Input .logjet file or spool directory")]
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct SegmentsArgs {
    #[arg(long, value_name = "DIR", help = "Directory containing rotated spool files")]
    pub path: PathBuf,

    #[arg(long, value_name = "BASE.LOGJET", help = "Base spool file name to inspect")]
    pub name: String,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    #[arg(long, value_name = "DIR", help = "Directory containing rotated spool files")]
    pub path: PathBuf,

    #[arg(long, value_name = "BASE.LOGJET", help = "Base spool file name to replay")]
    pub name: String,

    #[arg(long, value_name = "URL-OR-HOST:PORT", help = "Override collector destination from config")]
    pub dest: Option<String>,
}

#[derive(Debug, Args)]
pub struct PruneArgs {
    #[arg(long, value_name = "DIR", help = "Directory containing rotated spool files")]
    pub path: PathBuf,

    #[arg(long, value_name = "BASE.LOGJET", help = "Base spool file name to prune")]
    pub name: String,

    #[arg(long = "keep-files", value_name = "N", conflicts_with = "keep_bytes", help = "Retain at most N newest segments")]
    pub keep_files: Option<usize>,

    #[arg(long = "keep-bytes", value_name = "BYTES", conflicts_with = "keep_files", help = "Retain newest segments until total size fits")]
    pub keep_bytes: Option<u64>,

    #[arg(long = "dry-run", help = "Print removals without deleting files")]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct BridgeArgs {
    #[arg(long, value_name = "HOST:PORT", help = "Override upstream replay source from config")]
    pub source: Option<String>,
}

pub fn build_cli() -> clap::Command {
    let appname = "ljd";
    let styles = styling::Styles::styled()
        .header(styling::AnsiColor::Yellow.on_default())
        .usage(styling::AnsiColor::Yellow.on_default())
        .literal(styling::AnsiColor::BrightGreen.on_default())
        .placeholder(styling::AnsiColor::BrightMagenta.on_default());
    let after_help = format!(
        "Examples:\n  {appname} --config ./logjetd.conf\n  {appname} --plugins\n  {appname} inspect /var/lib/logjet\n  {appname} segments --path /var/lib/logjet --name app.logjet\n  {appname} replay --path /var/lib/logjet --name app.logjet --dest http://127.0.0.1:4318/v1/logs\n  {appname} prune --path /var/lib/logjet --name app.logjet --keep-files 2 --dry-run\n  {appname} bridge --source 10.0.0.15:7002"
    );

    Cli::command()
        .styles(styles)
        .about(format!("{} - {}", appname.bright_magenta().bold(), "telemetry daemon for ingest, replay, and spool management"))
        .override_usage(format!(
            "{appname} [serve] [-c|--config <PATH>] [--plugins]\n  {appname} inspect [--verify-otlp] <PATH>\n  {appname} segments --path <DIR> --name <BASE.LOGJET>\n  {appname} replay [-c|--config <PATH>] --path <DIR> --name <BASE.LOGJET> [--dest <URL-OR-HOST:PORT>]\n  {appname} prune --path <DIR> --name <BASE.LOGJET> [--keep-files <N> | --keep-bytes <BYTES>] [--dry-run]\n  {appname} bridge [-c|--config <PATH>] [--source <HOST:PORT>]"
        ))
        .after_help(after_help)
}
