//! `ljx dedup` command: run the dedup pipeline on a .logjet file.

use std::io::Write;

use logjet::{LogjetReader, LogjetWriter};

use crate::cli::{DedupArgs, DedupBehaviorArg, DedupMatchArg};
use crate::dedup::flat_record::BucketKeyKind;
use crate::dedup::{self, DedupMatchMode, DedupMode, DedupOpts, DrainOpts};
use crate::error::Result;
use crate::input::{InputHandle, open_output};

impl From<DedupBehaviorArg> for DedupMode {
    fn from(value: DedupBehaviorArg) -> Self {
        match value {
            DedupBehaviorArg::Distinct => Self::Distinct,
            DedupBehaviorArg::Collapse => Self::Collapse,
        }
    }
}

impl From<DedupMatchArg> for DedupMatchMode {
    fn from(value: DedupMatchArg) -> Self {
        match value {
            DedupMatchArg::Exact => Self::Exact,
            DedupMatchArg::Canon => Self::Hash2,
            DedupMatchArg::Full => Self::Full,
        }
    }
}

pub fn run(args: DedupArgs) -> Result<()> {
    let bucket_key = parse_bucket_by(&args.bucket_by)?;
    let drain = DrainOpts {
        sim_th: args.sim_th.unwrap_or(0.7),
        depth: args.drain_depth.unwrap_or(3),
        extra_delimiters: args.extra_delimiters.as_ref().map(|s| s.split(',').map(String::from).collect()).unwrap_or_default(),
    };
    let opts = DedupOpts { mode: args.mode.into(), match_mode: args.matcher.into(), bucket_key, drain };

    let input = InputHandle::open(&args.input)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());
    let unpacked = dedup::unpack::unpack(&mut reader)?;

    let output = open_output(&args.output)?;
    let mut writer = LogjetWriter::new(output);

    let stats = dedup::dedup(unpacked.records, unpacked.passthrough, &mut writer, &opts)?;

    let mut out = writer.into_inner()?;
    out.flush()?;

    eprintln!("{} records → {} groups ({:.1}% reduction)", stats.total_records, stats.group_count, stats.reduction_pct(),);
    Ok(())
}

fn parse_bucket_by(bucket_by: &Option<String>) -> Result<BucketKeyKind> {
    let Some(val) = bucket_by else {
        return Ok(BucketKeyKind::Default);
    };
    let parts: Vec<&str> = val.split(',').map(str::trim).collect();
    let has_scope = parts.contains(&"scope");
    let has_source = parts.contains(&"source_line");
    for &p in &parts {
        if p != "scope" && p != "source_line" {
            return Err(crate::error::Error::Usage(format!("unknown --bucket-by value: {p:?} (valid: scope, source_line)")));
        }
    }
    Ok(match (has_scope, has_source) {
        (true, true) => BucketKeyKind::ScopeAndSourceLine,
        (true, false) => BucketKeyKind::Scope,
        (false, true) => BucketKeyKind::SourceLine,
        (false, false) => BucketKeyKind::Default,
    })
}
