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
    #[command(subcommand)]
    pub command: Command,
}

pub fn build_cli() -> clap::Command {
    let appname = "ljx";
    let styles = styling::Styles::styled()
        .header(styling::AnsiColor::Yellow.on_default())
        .usage(styling::AnsiColor::Yellow.on_default())
        .literal(styling::AnsiColor::BrightGreen.on_default())
        .placeholder(styling::AnsiColor::BrightMagenta.on_default());

    Cli::command()
        .styles(styles)
        .about(format!(
            "{} - {}",
            appname.bright_magenta().bold(),
            "offline toolbox for .logjet streams"
        ))
        .override_usage(format!(
            "{appname} <COMMAND> [OPTIONS] [ARGS]"
        ))
        .after_help(
            "Examples:\n  ljx count telemetry.logjet -F error -i\n  ljx filter telemetry.logjet -o only-logs.logjet -e 'java\\..*\\.bs'",
        )
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Split(SplitArgs),
    Join(JoinArgs),
    Filter(FilterArgs),
    Count(CountArgs),
    Stats(StatsArgs),
    Cat(CatArgs),
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

    #[arg(
        short,
        long,
        value_name = "OUTPUT",
        help = "Output .logjet file or - for stdout"
    )]
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
pub struct CatArgs {
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    #[arg(long, default_value_t = false)]
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
