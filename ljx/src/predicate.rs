use std::collections::HashSet;

use clap::{Args, CommandFactory, FromArgMatches, Parser, ValueEnum};
use logjet::{OwnedRecord, RecordType};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;
use regex::bytes::{Regex, RegexBuilder};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Strings,
    Regex,
}

/// Field-level filter: constrains records by OTLP severity and/or service name.
#[derive(Debug, Clone, Default)]
pub struct FieldFilter {
    pub severities: Option<HashSet<String>>,
    pub services: Option<HashSet<String>>,
}

impl FieldFilter {
    pub fn is_empty(&self) -> bool {
        self.severities.is_none() && self.services.is_none()
    }

    /// Checks whether an OTLP log payload matches the field filter.
    pub fn matches_payload(&self, payload: &[u8]) -> bool {
        if self.is_empty() {
            return true;
        }
        let Ok(batch) = ExportLogsServiceRequest::decode(payload) else {
            return true; // can't decode → don't filter out
        };
        for rl in &batch.resource_logs {
            let service = rl.resource.as_ref().and_then(|r| {
                r.attributes.iter().find(|a| a.key == "service.name").and_then(|a| {
                    a.value.as_ref().and_then(|v| {
                        if let Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s)) = &v.value {
                            Some(s.as_str())
                        } else {
                            None
                        }
                    })
                })
            });
            if let Some(allowed) = &self.services
                && let Some(svc) = service
                && !allowed.contains(svc)
            {
                return false;
            }
            for sl in &rl.scope_logs {
                for lr in &sl.log_records {
                    if let Some(allowed) = &self.severities
                        && !allowed.contains(&lr.severity_text)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[derive(Debug, Clone, Args, Default)]
pub struct PredicateArgs {
    #[arg(long = "type", value_enum, help = "Match only one record type")]
    pub record_type: Option<RecordKind>,

    #[arg(long, help = "Match records with sequence >= this value")]
    pub seq_min: Option<u64>,

    #[arg(long, help = "Match records with sequence <= this value")]
    pub seq_max: Option<u64>,

    #[arg(long, help = "Match records with timestamp >= this unix-ns value")]
    pub ts_min: Option<u64>,

    #[arg(long, help = "Match records with timestamp <= this unix-ns value")]
    pub ts_max: Option<u64>,

    #[arg(short = 'e', long = "grep", value_name = "PATTERN", help = "Regex payload matcher; repeat for AND semantics")]
    pub grep: Vec<String>,

    #[arg(short = 'F', long = "fixed-string", value_name = "TEXT", help = "Literal payload matcher; repeat for AND semantics")]
    pub fixed_string: Vec<String>,

    #[arg(short = 'i', long = "ignore-case", help = "Apply case-insensitive matching to all payload matchers")]
    pub ignore_case: bool,
}

#[derive(Debug, Clone)]
pub struct RecordPredicate {
    record_type: Option<RecordType>,
    seq_min: Option<u64>,
    seq_max: Option<u64>,
    ts_min: Option<u64>,
    ts_max: Option<u64>,
    payload_matchers: Vec<PayloadMatcher>,
    pub field_filter: FieldFilter,
}

#[derive(Debug, Clone)]
struct PayloadMatcher {
    regex: Regex,
}

impl PredicateArgs {
    pub fn has_filters(&self) -> bool {
        self.record_type.is_some()
            || self.seq_min.is_some()
            || self.seq_max.is_some()
            || self.ts_min.is_some()
            || self.ts_max.is_some()
            || !self.grep.is_empty()
            || !self.fixed_string.is_empty()
            || self.ignore_case
    }
}

pub fn parse_string_filter(values: Vec<String>, label: &str) -> Result<Option<HashSet<String>>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut set = HashSet::new();
    for value in values {
        if value.is_empty() {
            return Err(Error::Usage(format!("empty {label} filter values are not allowed")));
        }
        set.insert(value);
    }
    Ok(Some(set))
}

impl PredicateArgs {
    pub fn build(self) -> Result<RecordPredicate> {
        let mut payload_matchers = Vec::with_capacity(self.grep.len() + self.fixed_string.len());
        for pattern in self.grep {
            payload_matchers.push(PayloadMatcher::new(&pattern, false, self.ignore_case)?);
        }
        for text in self.fixed_string {
            payload_matchers.push(PayloadMatcher::new(&text, true, self.ignore_case)?);
        }

        Ok(RecordPredicate {
            record_type: self.record_type.map(Into::into),
            seq_min: self.seq_min,
            seq_max: self.seq_max,
            ts_min: self.ts_min,
            ts_max: self.ts_max,
            payload_matchers,
            field_filter: FieldFilter::default(),
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
            FilterMode::Strings => PredicateArgs { fixed_string: vec![trimmed.to_string()], ..PredicateArgs::default() }.build(),
            FilterMode::Regex => PredicateArgs { grep: vec![trimmed.to_string()], ..PredicateArgs::default() }.build(),
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
        for matcher in &self.payload_matchers {
            if !matcher.is_match(&record.payload) {
                return false;
            }
        }
        if !self.field_filter.matches_payload(&record.payload) {
            return false;
        }

        true
    }

    pub(crate) fn record_type_filter(&self) -> Option<RecordType> {
        self.record_type
    }

    pub(crate) fn seq_min_filter(&self) -> Option<u64> {
        self.seq_min
    }

    pub(crate) fn seq_max_filter(&self) -> Option<u64> {
        self.seq_max
    }

    pub(crate) fn ts_min_filter(&self) -> Option<u64> {
        self.ts_min
    }

    pub(crate) fn ts_max_filter(&self) -> Option<u64> {
        self.ts_max
    }

    pub(crate) fn service_filter(&self) -> Option<&HashSet<String>> {
        self.field_filter.services.as_ref()
    }

    pub(crate) fn severity_filter(&self) -> Option<&HashSet<String>> {
        self.field_filter.severities.as_ref()
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
#[path = "../tests/unit/predicate_ut.rs"]
mod predicate_ut;
