use std::path::PathBuf;

use clap::builder::styling;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use colored::Colorize;

use crate::predicate::PredicateArgs;

#[derive(Debug, Parser)]
#[command(
    name = "ljx",
    version,
    about = "Offline toolbox for .logjet files",
    long_about = None
)]
pub struct Cli {
    #[arg(short = 'x', long = "export", value_name = "FORMAT", value_parser = parse_export_name, requires_all = ["input", "output"], help = "Export one .logjet input to a built-in or plugin format")]
    pub export: Option<String>,

    #[arg(short, long, value_name = "OUTPUT", requires = "export", help = "Output file or - for stdout")]
    pub output: Option<PathBuf>,

    #[arg(value_name = "INPUT", requires = "export", help = "Input .logjet file or - for stdin")]
    pub input: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

pub fn build_cli() -> clap::Command {
    let appname = "ljx";
    let styles = styling::Styles::styled()
        .header(styling::AnsiColor::Yellow.on_default())
        .usage(styling::AnsiColor::Yellow.on_default())
        .literal(styling::AnsiColor::BrightGreen.on_default())
        .placeholder(styling::AnsiColor::BrightMagenta.on_default());

    Cli::command()
        .arg_required_else_help(true)
        .styles(styles)
        .about(format!(
            "{} - {}",
            appname.bright_magenta().bold(),
            "offline toolbox for .logjet streams"
        ))
        .override_usage(format!(
            "{appname} <COMMAND> [OPTIONS] [ARGS]\n  {appname} --export <FORMAT> <INPUT> -o <OUTPUT>"
        ))
        .after_help(
            "Examples:\n  ljx count telemetry.logjet -F error -i\n  ljx filter telemetry.logjet -o only-logs.logjet -e 'java\\..*\\.bs'\n  ljx view telemetry.logjet\n  ljx --export ndjson telemetry.logjet -o telemetry.ndjson\n\nPlugin exporters are discovered from LJX_EXPORTER_PATH, then ./exporters, then <exe>/exporters, then <exe>/../lib/logjet/exporters.",
        )
}

fn parse_export_name(value: &str) -> std::result::Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("export format must not be empty".to_string());
    }
    if normalized.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')) {
        Ok(normalized)
    } else {
        Err("export format must use only lowercase letters, digits, '-' or '_'".to_string())
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportFormat {
    Ndjson,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Split one .logjet input into multiple outputs")]
    Split(SplitArgs),
    #[command(about = "Join multiple .logjet inputs into one ordered output")]
    Join(JoinArgs),
    #[command(about = "Filter records into a new .logjet stream")]
    Filter(FilterArgs),
    #[command(about = "Count records matching a predicate")]
    Count(CountArgs),
    #[command(about = "Compute summary statistics for one .logjet file")]
    Stats(StatsArgs),
    #[command(name = "view", alias = "cat", about = "Interactively browse filtered records in a terminal UI")]
    View(ViewArgs),
    #[command(about = "Deduplicate log records across the whole selection or collapse nearby bursts")]
    Dedup(DedupArgs),
}

#[derive(Debug, Clone, Args)]
pub struct DedupArgs {
    #[arg(value_name = "INPUT", help = "Input .logjet file or - for stdin")]
    pub input: PathBuf,

    #[arg(short, long, value_name = "OUTPUT", help = "Output .logjet file or - for stdout")]
    pub output: PathBuf,

    #[arg(long, value_enum, default_value_t = DedupBehaviorArg::Distinct, help = "Dedup behavior: distinct across the whole selection, or collapse nearby bursts")]
    pub mode: DedupBehaviorArg,

    #[arg(long = "match", alias = "matcher", value_enum, default_value_t = DedupMatchArg::Canon, help = "Matcher level inside the selected mode: exact, canon, or full")]
    pub matcher: DedupMatchArg,

    #[arg(long, value_name = "KEYS", help = "Comma-separated bucket extensions: scope, source_line (used by collapse mode)")]
    pub bucket_by: Option<String>,

    #[arg(long, value_name = "FLOAT", help = "Drain3 similarity threshold (full match only) [default: 0.7]")]
    pub sim_th: Option<f64>,

    #[arg(long, value_name = "INT", help = "Drain3 prefix tree depth (full match only) [default: 3]")]
    pub drain_depth: Option<i64>,

    #[arg(long, value_name = "DELIMS", help = "Drain3 extra delimiters, comma-separated (full match only)")]
    pub extra_delimiters: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DedupBehaviorArg {
    Distinct,
    Collapse,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DedupMatchArg {
    Exact,
    #[value(alias = "hash2")]
    Canon,
    Full,
}

#[derive(Debug, Clone, Args)]
pub struct CountArgs {
    #[arg(value_name = "INPUT", help = "Input .logjet file or - for stdin")]
    pub input: PathBuf,

    #[command(flatten)]
    pub predicate: PredicateArgs,
}

#[derive(Debug, Clone, Args)]
pub struct FilterArgs {
    #[arg(value_name = "INPUT", help = "Input .logjet file or - for stdin")]
    pub input: PathBuf,

    #[arg(short, long, value_name = "OUTPUT", help = "Output .logjet file or - for stdout")]
    pub output: PathBuf,

    #[arg(long, value_enum, default_value_t = OutputCodec::Lz4)]
    pub codec: OutputCodec,

    #[arg(
        long,
        default_value_t = logjet::DEFAULT_BLOCK_TARGET_SIZE,
        help = "Target uncompressed bytes per output block"
    )]
    pub block_target_size: usize,

    #[command(flatten)]
    pub predicate: PredicateArgs,
}

#[derive(Debug, Clone, Args)]
pub struct SplitArgs {
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    #[arg(value_name = "OUTPUT_PREFIX")]
    pub output_prefix: PathBuf,

    #[arg(long)]
    pub max_bytes: Option<u64>,

    #[arg(long)]
    pub max_records: Option<u64>,

    #[arg(long, help = "RFC 3339 or unix-ns range support to be added")]
    pub timestamp_range: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct JoinArgs {
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<PathBuf>,

    #[arg(short, long, value_name = "OUTPUT")]
    pub output: PathBuf,

    #[arg(long)]
    pub validate_sequence_continuity: bool,
}

#[derive(Debug, Clone, Args)]
pub struct StatsArgs {
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    #[arg(long, help = "Compute payload size summaries by record type")]
    pub field_stats: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ViewArgs {
    #[arg(value_name = "INPUT", help = "Input .logjet file or - for stdin")]
    pub input: PathBuf,

    #[arg(long, default_value_t = false, help = "Show payload previews in hex")]
    pub hex_payload: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputCodec {
    None,
    Lz4,
}

impl From<OutputCodec> for logjet::Codec {
    fn from(value: OutputCodec) -> Self {
        match value {
            OutputCodec::None => Self::None,
            OutputCodec::Lz4 => Self::Lz4,
        }
    }
}

#[cfg(test)]
#[path = "cli_utst.rs"]
mod cli_utst;
