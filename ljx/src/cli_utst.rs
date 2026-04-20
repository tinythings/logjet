use std::path::PathBuf;

use crate::cli::{Cli, Command, DedupBehaviorArg, DedupMatchArg, build_cli};
use clap::Parser;

#[test]
fn dedup_defaults_to_distinct_canon() {
    let cli = Cli::try_parse_from(["ljx", "dedup", "input.logjet", "-o", "out.logjet"]).expect("cli parses");
    let Some(Command::Dedup(args)) = cli.command else { panic!("expected dedup command") };
    assert!(matches!(args.mode, DedupBehaviorArg::Distinct));
    assert!(matches!(args.matcher, DedupMatchArg::Canon));
}

#[test]
fn dedup_accepts_collapse_and_full_matcher() {
    let cli = Cli::try_parse_from(["ljx", "dedup", "input.logjet", "-o", "out.logjet", "--mode", "collapse", "--match", "full"]).expect("cli parses");
    let Some(Command::Dedup(args)) = cli.command else { panic!("expected dedup command") };
    assert!(matches!(args.mode, DedupBehaviorArg::Collapse));
    assert!(matches!(args.matcher, DedupMatchArg::Full));
}

#[test]
fn top_level_export_parses() {
    let cli = Cli::try_parse_from(["ljx", "--export", "ndjson", "input.logjet", "-o", "out.ndjson"]).expect("cli parses");
    assert_eq!(cli.export.as_deref(), Some("ndjson"));
    assert_eq!(cli.input, Some(PathBuf::from("input.logjet")));
    assert_eq!(cli.output, Some(PathBuf::from("out.ndjson")));
    assert!(!cli.force);
    assert!(cli.command.is_none());
}

#[test]
fn top_level_export_force_parses() {
    let cli = Cli::try_parse_from(["ljx", "--export", "ndjson", "input.logjet", "-o", "out.ndjson", "--force"]).expect("cli parses");
    assert!(cli.force);
}

#[test]
fn help_lists_current_export_formats() {
    let help = build_cli().render_long_help().to_string();
    assert!(help.contains("--export <FORMAT>"));
    assert!(help.contains("Export one .logjet input to a built-in or plugin format: ndjson"));
    assert!(!help.contains("Built-in export formats:"));
    assert!(!help.contains("Discovered plugin export formats:"));
    assert!(!help.contains("All export formats now:"));
}
