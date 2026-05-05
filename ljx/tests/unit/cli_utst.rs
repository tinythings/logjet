use std::path::PathBuf;

use crate::cli::{Cli, Command, DedupBehaviorArg, DedupMatchArg, ViewOrderArg, build_cli};
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
    assert_eq!(cli.input, Some(vec![PathBuf::from("input.logjet") ]));
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
fn top_level_query_parses_literal_filter() {
    let cli = Cli::try_parse_from(["ljx", "input.logjet", "-F", "error"]).expect("cli parses");
    assert_eq!(cli.input, Some(vec![PathBuf::from("input.logjet") ]));
    assert!(cli.format.is_none());
    assert_eq!(cli.predicate.fixed_string, vec!["error".to_string()]);
}

#[test]
fn top_level_query_accepts_explicit_ndjson_format() {
    let cli = Cli::try_parse_from(["ljx", "--format", "ndjson", "input.logjet", "-e", "error|panic"]).expect("cli parses");
    assert_eq!(cli.input, Some(vec![PathBuf::from("input.logjet") ]));
    assert!(matches!(cli.format, Some(crate::cli::ExportFormat::Ndjson)));
    assert_eq!(cli.predicate.grep, vec!["error|panic".to_string()]);
}

#[test]
fn top_level_query_parses_repeated_literal_and_regex_filters() {
    let cli = Cli::try_parse_from(["ljx", "input.logjet", "-F", "foo", "-F", "bar", "-e", "baz|qux", "-e", "zap"]).expect("cli parses");
    assert_eq!(cli.predicate.fixed_string, vec!["foo".to_string(), "bar".to_string()]);
    assert_eq!(cli.predicate.grep, vec!["baz|qux".to_string(), "zap".to_string()]);
}

#[test]
fn top_level_query_parses_field_selection() {
    let cli = Cli::try_parse_from(["ljx", "input.logjet", "--fields", "body,timestamp", "--fields", "service_name"]).expect("cli parses");
    assert_eq!(cli.fields, vec!["body".to_string(), "timestamp".to_string(), "service_name".to_string()]);
}

#[test]
fn view_accepts_multiple_inputs() {
    let cli = Cli::try_parse_from(["ljx", "view", "one.logjet", "two.logjet", "three.logjet"]).expect("cli parses");
    let Some(Command::View(args)) = cli.command else { panic!("expected view command") };
    assert_eq!(args.inputs, vec![PathBuf::from("one.logjet"), PathBuf::from("two.logjet"), PathBuf::from("three.logjet")]);
}

#[test]
fn discover_accepts_dataset_pagination_and_filters() {
    let cli = Cli::try_parse_from([
        "ljx",
        "discover",
        "--offset",
        "5",
        "--limit",
        "10",
        "--ndjson",
        "--service",
        "api",
        "--severity",
        "ERROR",
        "--type",
        "logs",
        "one.logjet",
        "two.logjet",
    ])
    .expect("cli parses");
    let Some(Command::Discover(args)) = cli.command else { panic!("expected discover command") };
    assert_eq!(args.inputs, vec![PathBuf::from("one.logjet"), PathBuf::from("two.logjet")]);
    assert_eq!(args.offset, 5);
    assert_eq!(args.limit, Some(10));
    assert!(args.ndjson);
    assert_eq!(args.services, vec!["api".to_string()]);
    assert_eq!(args.severities, vec!["ERROR".to_string()]);
    assert!(args.predicate.record_type.is_some());
}

#[test]
fn view_accepts_merge_order() {
    let cli = Cli::try_parse_from(["ljx", "view", "--dataset-order", "merge-ts", "one.logjet", "two.logjet"]).expect("cli parses");
    let Some(Command::View(args)) = cli.command else { panic!("expected view command") };
    assert_eq!(args.inputs, vec![PathBuf::from("one.logjet"), PathBuf::from("two.logjet")]);
    assert!(matches!(args.dataset_order, ViewOrderArg::MergeTs));
}

#[test]
fn view_accepts_nfs_flag() {
    let cli = Cli::try_parse_from(["ljx", "view", "--nfs", "one.logjet"]).expect("cli parses");
    let Some(Command::View(args)) = cli.command else { panic!("expected view command") };
    assert!(args.nfs);
}

#[test]
fn discover_accepts_nfs_flag() {
    let cli = Cli::try_parse_from(["ljx", "discover", "--nfs", "one.logjet"]).expect("cli parses");
    let Some(Command::Discover(args)) = cli.command else { panic!("expected discover command") };
    assert!(args.nfs);
}

#[test]
fn help_lists_current_export_formats() {
    let help = build_cli().render_long_help().to_string();
    assert!(help.contains("ljx <INPUT...> [--format <FORMAT>] [FILTER OPTIONS]"));
    assert!(help.contains("--fields <KEY[,KEY...]>"));
    assert!(help.contains("--export <FORMAT>"));
    assert!(help.contains("Export one .logjet input to a built-in or plugin format: ndjson"));
    assert!(!help.contains("Built-in export formats:"));
    assert!(!help.contains("Discovered plugin export formats:"));
    assert!(!help.contains("All export formats now:"));
}
