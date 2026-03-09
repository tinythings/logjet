use clap::{ArgGroup, Args, CommandFactory, FromArgMatches, Parser, ValueEnum};
use logjet::{OwnedRecord, RecordType};
use regex::bytes::{Regex, RegexBuilder};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Strings,
    Regex,
}

#[derive(Debug, Clone, Args, Default)]
#[command(group(
    ArgGroup::new("payload_match")
        .args(["grep", "fixed_string"])
        .multiple(false)
))]
pub struct PredicateArgs {
    #[arg(long = "type", value_enum)]
    pub record_type: Option<RecordKind>,

    #[arg(long)]
    pub seq_min: Option<u64>,

    #[arg(long)]
    pub seq_max: Option<u64>,

    #[arg(long)]
    pub ts_min: Option<u64>,

    #[arg(long)]
    pub ts_max: Option<u64>,

    #[arg(short = 'e', long = "grep", value_name = "PATTERN")]
    pub grep: Option<String>,

    #[arg(short = 'F', long = "fixed-string", value_name = "TEXT")]
    pub fixed_string: Option<String>,

    #[arg(short = 'i', long = "ignore-case")]
    pub ignore_case: bool,
}

#[derive(Debug, Clone)]
pub struct RecordPredicate {
    record_type: Option<RecordType>,
    seq_min: Option<u64>,
    seq_max: Option<u64>,
    ts_min: Option<u64>,
    ts_max: Option<u64>,
    payload_matcher: Option<PayloadMatcher>,
}

#[derive(Debug, Clone)]
struct PayloadMatcher {
    regex: Regex,
}

impl PredicateArgs {
    pub fn build(self) -> Result<RecordPredicate> {
        let payload_matcher = match (self.grep, self.fixed_string) {
            (Some(pattern), None) => Some(PayloadMatcher::new(&pattern, false, self.ignore_case)?),
            (None, Some(text)) => Some(PayloadMatcher::new(&text, true, self.ignore_case)?),
            (None, None) => None,
            (Some(_), Some(_)) => {
                return Err(Error::Usage("choose either -e/--grep or -F/--fixed-string, not both".to_string()));
            }
        };

        Ok(RecordPredicate {
            record_type: self.record_type.map(Into::into),
            seq_min: self.seq_min,
            seq_max: self.seq_max,
            ts_min: self.ts_min,
            ts_max: self.ts_max,
            payload_matcher,
        })
    }
}

#[derive(Debug, Parser)]
struct PredicateCli {
    #[command(flatten)]
    predicate: PredicateArgs,
}

pub fn parse_filter_query(query: &str, mode: FilterMode) -> Result<RecordPredicate> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return PredicateArgs::default().build();
    }

    // Bare text in the TUI stays ergonomic and depends on the active filter mode.
    if !trimmed.starts_with('-') {
        return match mode {
            FilterMode::Strings => PredicateArgs { fixed_string: Some(trimmed.to_string()), ..PredicateArgs::default() }.build(),
            FilterMode::Regex => PredicateArgs { grep: Some(trimmed.to_string()), ..PredicateArgs::default() }.build(),
        };
    }

    let argv = shlex::split(trimmed).ok_or_else(|| Error::Usage("invalid filter expression: unterminated quotes".to_string()))?;
    let mut full_argv = Vec::with_capacity(argv.len() + 1);
    full_argv.push("view-filter".to_string());
    full_argv.extend(argv);

    let mut command = PredicateCli::command();
    let mut matches = command.try_get_matches_from_mut(full_argv).map_err(|err| Error::Usage(err.to_string()))?;
    let parsed = PredicateCli::from_arg_matches_mut(&mut matches).map_err(|err| Error::Usage(err.to_string()))?;
    parsed.predicate.build()
}

impl RecordPredicate {
    pub fn matches(&self, record: &OwnedRecord) -> bool {
        if let Some(expected) = self.record_type
            && record.record_type != expected
        {
            return false;
        }
        if let Some(min) = self.seq_min
            && record.seq < min
        {
            return false;
        }
        if let Some(max) = self.seq_max
            && record.seq > max
        {
            return false;
        }
        if let Some(min) = self.ts_min
            && record.ts_unix_ns < min
        {
            return false;
        }
        if let Some(max) = self.ts_max
            && record.ts_unix_ns > max
        {
            return false;
        }
        if let Some(matcher) = &self.payload_matcher
            && !matcher.is_match(&record.payload)
        {
            return false;
        }

        true
    }
}

impl PayloadMatcher {
    fn new(pattern: &str, fixed_string: bool, ignore_case: bool) -> Result<Self> {
        let source = if fixed_string { regex::escape(pattern) } else { pattern.to_string() };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(ignore_case)
            .build()
            .map_err(|err| Error::Usage(format!("invalid payload matcher: {err}")))?;
        Ok(Self { regex })
    }

    fn is_match(&self, payload: &[u8]) -> bool {
        self.regex.is_match(payload)
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RecordKind {
    Logs,
    Metrics,
    Traces,
}

impl From<RecordKind> for RecordType {
    fn from(value: RecordKind) -> Self {
        match value {
            RecordKind::Logs => Self::Logs,
            RecordKind::Metrics => Self::Metrics,
            RecordKind::Traces => Self::Traces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FilterMode, PredicateArgs, RecordKind, parse_filter_query};
    use logjet::{OwnedRecord, RecordType};

    fn sample_record(payload: &[u8]) -> OwnedRecord {
        OwnedRecord { record_type: RecordType::Logs, seq: 42, ts_unix_ns: 1_700_000_000, payload: payload.to_vec() }
    }

    #[test]
    fn fixed_string_match_is_literal() {
        let predicate = PredicateArgs { fixed_string: Some("java.crap.failed".to_string()), ..PredicateArgs::default() }.build().unwrap();

        assert!(predicate.matches(&sample_record(b"xxx java.crap.failed yyy")));
        assert!(!predicate.matches(&sample_record(b"javaXcrapXfailed")));
    }

    #[test]
    fn regex_match_supports_wildcards() {
        let predicate = PredicateArgs { grep: Some(r"java\..*\.bs".to_string()), ..PredicateArgs::default() }.build().unwrap();

        assert!(predicate.matches(&sample_record(b"java.very.long.bs")));
        assert!(!predicate.matches(&sample_record(b"java.very.long.cs")));
    }

    #[test]
    fn ignore_case_applies_to_fixed_string_and_regex() {
        let fixed = PredicateArgs { fixed_string: Some("error".to_string()), ignore_case: true, ..PredicateArgs::default() }.build().unwrap();
        let regex = PredicateArgs { grep: Some("error".to_string()), ignore_case: true, ..PredicateArgs::default() }.build().unwrap();

        let record = sample_record(b"prefix eRrOr suffix");
        assert!(fixed.matches(&record));
        assert!(regex.matches(&record));
    }

    #[test]
    fn matcher_combines_with_record_fields() {
        let predicate = PredicateArgs {
            record_type: Some(RecordKind::Logs),
            seq_min: Some(40),
            seq_max: Some(45),
            ts_min: Some(1_699_999_999),
            ts_max: Some(1_700_000_001),
            fixed_string: Some("hello".to_string()),
            ..PredicateArgs::default()
        }
        .build()
        .unwrap();

        assert!(predicate.matches(&sample_record(b"hello world")));
        assert!(!predicate.matches(&sample_record(b"bye world")));
    }

    #[test]
    fn invalid_regex_is_reported() {
        let error = PredicateArgs { grep: Some("(".to_string()), ..PredicateArgs::default() }.build().unwrap_err();

        assert!(error.to_string().contains("invalid payload matcher"));
    }

    #[test]
    fn parse_filter_query_treats_bare_text_as_fixed_string() {
        let predicate = parse_filter_query("hello world", FilterMode::Strings).unwrap();
        assert!(predicate.matches(&sample_record(b"say hello world now")));
        assert!(!predicate.matches(&sample_record(b"say hello now")));
    }

    #[test]
    fn parse_filter_query_supports_cli_style_flags() {
        let predicate = parse_filter_query(r#"--type logs -e "error|panic" -i"#, FilterMode::Strings).unwrap();
        assert!(predicate.matches(&sample_record(b"PANIC happened")));
        assert!(!predicate.matches(&OwnedRecord {
            record_type: RecordType::Metrics,
            seq: 42,
            ts_unix_ns: 1_700_000_000,
            payload: b"panic".to_vec(),
        }));
    }

    #[test]
    fn parse_filter_query_uses_regex_mode_for_bare_text() {
        let predicate = parse_filter_query("reb.*", FilterMode::Regex).unwrap();
        assert!(predicate.matches(&sample_record(b"rebooted node")));
        assert!(!predicate.matches(&sample_record(b"stopped node")));
    }
}
