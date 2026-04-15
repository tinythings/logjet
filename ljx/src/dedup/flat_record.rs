//! Flattened representation of a single OTLP log record, extracted from
//! the nested ResourceLogs → ScopeLogs → LogRecord hierarchy.
//!
//! All fields needed for bucketing, hashing, and re-emission are captured
//! here so that downstream stages never touch protobuf again.

/// Selects which fields form the bucket key for dedup grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketKeyKind {
    /// Global: ignore bucket boundaries entirely.
    Global,
    /// Default: (service_name, severity_number).
    Default,
    /// Adds instrumentation scope name.
    Scope,
    /// Adds code.filepath + code.lineno from OTel attributes.
    SourceLine,
    /// Adds both scope and source line.
    ScopeAndSourceLine,
}

/// Hard partition key. Records with different keys never merge.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BucketKey {
    pub service_name: String,
    pub severity_number: i32,
    pub scope_name: Option<String>,
    pub code_filepath: Option<String>,
    pub code_lineno: Option<i64>,
}

impl BucketKey {
    pub fn from_record(rec: &FlatRecord, kind: BucketKeyKind) -> Self {
        let scope_name = match kind {
            BucketKeyKind::Scope | BucketKeyKind::ScopeAndSourceLine => Some(rec.scope_name.clone()),
            _ => None,
        };
        let (code_filepath, code_lineno) = match kind {
            BucketKeyKind::SourceLine | BucketKeyKind::ScopeAndSourceLine => (rec.code_filepath.clone(), rec.code_lineno),
            _ => (None, None),
        };
        match kind {
            BucketKeyKind::Global => {
                Self { service_name: String::new(), severity_number: 0, scope_name: None, code_filepath: None, code_lineno: None }
            }
            _ => Self { service_name: rec.service_name.clone(), severity_number: rec.severity_number, scope_name, code_filepath, code_lineno },
        }
    }
}

/// One log record, fully flattened. Owns all data.
#[derive(Debug, Clone)]
pub struct FlatRecord {
    // identity / bucketing
    pub service_name: String,
    pub severity_number: i32,
    pub severity_text: String,
    pub scope_name: String,
    pub event_name: String,

    // optional source location (from OTel attributes)
    pub code_filepath: Option<String>,
    pub code_lineno: Option<i64>,

    // trace context
    pub trace_id: Vec<u8>,
    pub span_id: Vec<u8>,

    // timestamps
    pub time_unix_nano: u64,
    pub observed_time_unix_nano: u64,

    // body (the string we hash / canonicalise)
    pub body: String,

    // raw attribute bags (preserved for re-emission)
    pub resource_attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
    pub scope_attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
    pub record_attrs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
}
