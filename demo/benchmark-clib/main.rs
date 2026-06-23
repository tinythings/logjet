//! Evidence benchmark for the liblogjet C API.
//!
//! Decomposes the per-record cost into:
//!   1. writing one record to a `.logjet` file (storage format, no network)
//!   2. sending one record to the OTEL backend via the C API (`lj_logger_log`)
//!   3. sending one record to the OTEL backend reusing the connection
//!      (`lj_logger_log_reuse`)
//!
//! It reproduces the slow per-connection path and shows where the per-record
//! time goes.
//!
//! The "file" row links the `logjet` crate directly; the "backend" rows call
//! the exported C ABI symbols of `liblogjet` (the same code a C/C++ caller
//! exercises through the shared library).

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ptr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use liblogjet::{
    LjAttribute, LjLogRecord, lj_error_message, lj_logger, lj_logger_async_dropped, lj_logger_async_errors, lj_logger_flush, lj_logger_free, lj_logger_log, lj_logger_log_async,
    lj_logger_log_batch, lj_logger_log_reuse, lj_logger_new_grpc,
};
use logjet::{LogjetWriter, RecordType};

const LJ_ATTR_STRING: i32 = 0;
const SEVERITY_INFO: i32 = 9;
const TIMEOUT_MS: u64 = 2000;
const PAYLOAD_LEN: usize = 200;

type LogFn = unsafe extern "C" fn(*mut lj_logger, *const LjLogRecord) -> bool;

struct Stats {
    count: usize,
    total: f64,
    mean: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    min: f64,
    max: f64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().unwrap_or_else(|| "127.0.0.1:4317".to_string());
    let count: usize = args.next().and_then(|value| value.parse().ok()).filter(|n| *n > 0).unwrap_or(1000);
    let batch_size: usize = args.next().and_then(|value| value.parse().ok()).filter(|n| *n > 0).unwrap_or(100);

    println!("benchmark-clib: {count} records per phase, batch={batch_size}, endpoint {endpoint}\n");

    let file_samples = match run_file_phase(count) {
        Ok(samples) => samples,
        Err(err) => {
            eprintln!("logjet file phase failed: {err}");
            std::process::exit(1);
        }
    };

    let endpoint_c = CString::new(endpoint.as_str()).expect("endpoint has interior NUL");
    let service_c = CString::new("benchmark-clib").expect("service name");
    let body_c = CString::new("benchmark log record body from benchmark-clib evidence run").expect("body");

    let mut keepalive: Vec<CString> = Vec::new();
    let mut attrs: Vec<LjAttribute> = Vec::new();
    for (key, value) in [("appliance.kind", "benchmark"), ("character", "Bender"), ("location", "Planet Express")] {
        let key_c = CString::new(key).expect("attr key");
        let value_c = CString::new(value).expect("attr value");
        attrs.push(LjAttribute { key: key_c.as_ptr(), value: value_c.as_ptr(), value_type: LJ_ATTR_STRING });
        keepalive.push(key_c);
        keepalive.push(value_c);
    }

    let mut record = LjLogRecord {
        timestamp_unix_ns: 0,
        severity_number: SEVERITY_INFO,
        severity_text: ptr::null(),
        body: body_c.as_ptr(),
        attributes: attrs.as_ptr(),
        attributes_len: attrs.len(),
        event_name: ptr::null(),
        service_name: ptr::null(),
        scope_name: ptr::null(),
        resource_attrs: ptr::null(),
        resource_attrs_len: 0,
        scope_attrs: ptr::null(),
        scope_attrs_len: 0,
    };

    let per_connection = match unsafe { run_backend_phase(&endpoint_c, &service_c, &mut record, count, lj_logger_log) } {
        Ok(result) => result,
        Err(err) => {
            eprintln!("per-connection backend phase failed: {err}");
            eprintln!("is ljd listening on {endpoint}? start it with run-demo.sh");
            std::process::exit(1);
        }
    };

    let reuse = match unsafe { run_backend_phase(&endpoint_c, &service_c, &mut record, count, lj_logger_log_reuse) } {
        Ok(result) => result,
        Err(err) => {
            eprintln!("reuse backend phase failed: {err}");
            std::process::exit(1);
        }
    };

    let batch = match unsafe { run_batch_phase(&endpoint_c, &service_c, &record, count, batch_size) } {
        Ok(result) => result,
        Err(err) => {
            eprintln!("batch backend phase failed: {err}");
            std::process::exit(1);
        }
    };

    let async_phase = match unsafe { run_async_phase(&endpoint_c, &service_c, &record, count) } {
        Ok(result) => result,
        Err(err) => {
            eprintln!("async backend phase failed: {err}");
            std::process::exit(1);
        }
    };

    let file_stats = summarize(file_samples);
    let per_connection_stats = summarize(per_connection.samples);
    let reuse_cold = reuse.cold_first;
    let reuse_stats = summarize(reuse.samples);
    let raw_batch_mean = batch.raw_batch_mean;
    let batch_stats = summarize(batch.samples);
    let batch_label = format!("backend OTLP/gRPC (batch={batch_size})");
    let async_stats = summarize(async_phase.samples);

    print_table(&[
        ("logjet file (LogjetWriter::push)", &file_stats),
        ("backend OTLP/gRPC (per-connection)", &per_connection_stats),
        ("backend OTLP/gRPC (reuse)", &reuse_stats),
        (batch_label.as_str(), &batch_stats),
        ("backend OTLP/gRPC (async enqueue)", &async_stats),
    ]);

    println!();
    println!("note: batch row is per-record amortized; raw per-batch-call mean: {}", fmt_dur(raw_batch_mean));
    println!(
        "note: async row is the caller-thread enqueue cost (fire-and-forget); flush: {}, errors: {}, dropped: {}",
        fmt_dur(async_phase.flush_ns),
        async_phase.errors,
        async_phase.dropped,
    );
    println!("reuse first/cold call (one-time connect): {}", fmt_dur(reuse_cold));
    if reuse_stats.mean > 0.0 {
        println!("backend speedup (mean, per-connection -> reuse): {:.1}x", per_connection_stats.mean / reuse_stats.mean);
    }
    if batch_stats.mean > 0.0 {
        println!("backend speedup (mean, per-connection -> batch): {:.1}x", per_connection_stats.mean / batch_stats.mean);
    }
    if async_stats.mean > 0.0 {
        println!("backend speedup (mean, per-connection -> async enqueue): {:.1}x", per_connection_stats.mean / async_stats.mean);
    }
}

fn run_file_phase(count: usize) -> std::io::Result<Vec<u128>> {
    let path = std::env::temp_dir().join("logjet-benchmark-clib.logjet");
    let file = File::create(&path)?;
    let mut writer = LogjetWriter::new(BufWriter::new(file));
    let payload = vec![0xABu8; PAYLOAD_LEN];
    let base_ts = 1_700_000_000_000_000_000u64;

    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let seq = index as u64 + 1;
        let ts = base_ts + index as u64 * 1_000;
        let start = Instant::now();
        writer.push(RecordType::Logs, seq, ts, &payload).map_err(|err| std::io::Error::other(err.to_string()))?;
        samples.push(start.elapsed().as_nanos());
    }

    let mut inner = writer.into_inner().map_err(|err| std::io::Error::other(err.to_string()))?;
    inner.flush()?;
    let _ = std::fs::remove_file(&path);
    Ok(samples)
}

struct BackendResult {
    samples: Vec<u128>,
    cold_first: f64,
}

unsafe fn run_backend_phase(endpoint: &CStr, service: &CStr, record: &mut LjLogRecord, count: usize, log_fn: LogFn) -> Result<BackendResult, String> {
    let logger = unsafe { lj_logger_new_grpc(endpoint.as_ptr(), service.as_ptr(), TIMEOUT_MS) };
    if logger.is_null() {
        return Err(last_error());
    }

    let mut samples = Vec::with_capacity(count);
    let mut error: Option<String> = None;
    for _ in 0..count {
        record.timestamp_unix_ns = now_unix_ns();
        let start = Instant::now();
        let ok = unsafe { log_fn(logger, record as *const LjLogRecord) };
        let elapsed = start.elapsed().as_nanos();
        if !ok {
            error = Some(last_error());
            break;
        }
        samples.push(elapsed);
    }

    unsafe { lj_logger_free(logger) };

    if let Some(err) = error {
        return Err(err);
    }

    let cold_first = samples.first().copied().unwrap_or(0) as f64;
    Ok(BackendResult { samples, cold_first })
}

struct BatchResult {
    samples: Vec<u128>,
    raw_batch_mean: f64,
}

fn clone_record(template: &LjLogRecord) -> LjLogRecord {
    LjLogRecord {
        timestamp_unix_ns: template.timestamp_unix_ns,
        severity_number: template.severity_number,
        severity_text: template.severity_text,
        body: template.body,
        attributes: template.attributes,
        attributes_len: template.attributes_len,
        event_name: template.event_name,
        service_name: template.service_name,
        scope_name: template.scope_name,
        resource_attrs: template.resource_attrs,
        resource_attrs_len: template.resource_attrs_len,
        scope_attrs: template.scope_attrs,
        scope_attrs_len: template.scope_attrs_len,
    }
}

unsafe fn run_batch_phase(endpoint: &CStr, service: &CStr, template: &LjLogRecord, count: usize, batch_size: usize) -> Result<BatchResult, String> {
    let logger = unsafe { lj_logger_new_grpc(endpoint.as_ptr(), service.as_ptr(), TIMEOUT_MS) };
    if logger.is_null() {
        return Err(last_error());
    }

    let mut batch: Vec<LjLogRecord> = (0..batch_size).map(|_| clone_record(template)).collect();
    let mut samples = Vec::with_capacity(count);
    let mut total_batch_ns: u128 = 0;
    let mut batch_calls: u128 = 0;
    let mut sent = 0;
    let mut error: Option<String> = None;

    while sent < count {
        let this = batch_size.min(count - sent);
        let now = now_unix_ns();
        for entry in batch[..this].iter_mut() {
            entry.timestamp_unix_ns = now;
        }
        let start = Instant::now();
        let ok = unsafe { lj_logger_log_batch(logger, batch.as_ptr(), this) };
        let elapsed = start.elapsed().as_nanos();
        if !ok {
            error = Some(last_error());
            break;
        }
        total_batch_ns += elapsed;
        batch_calls += 1;
        let per_record = elapsed / this as u128;
        for _ in 0..this {
            samples.push(per_record);
        }
        sent += this;
    }

    unsafe { lj_logger_free(logger) };

    if let Some(err) = error {
        return Err(err);
    }

    let raw_batch_mean = if batch_calls > 0 { total_batch_ns as f64 / batch_calls as f64 } else { 0.0 };
    Ok(BatchResult { samples, raw_batch_mean })
}

struct AsyncResult {
    samples: Vec<u128>,
    flush_ns: f64,
    errors: u64,
    dropped: u64,
}

unsafe fn run_async_phase(endpoint: &CStr, service: &CStr, template: &LjLogRecord, count: usize) -> Result<AsyncResult, String> {
    let logger = unsafe { lj_logger_new_grpc(endpoint.as_ptr(), service.as_ptr(), TIMEOUT_MS) };
    if logger.is_null() {
        return Err(last_error());
    }

    let mut record = clone_record(template);
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        record.timestamp_unix_ns = now_unix_ns();
        let start = Instant::now();
        let ok = unsafe { lj_logger_log_async(logger, &record as *const LjLogRecord) };
        let elapsed = start.elapsed().as_nanos();
        if !ok {
            let err = last_error();
            unsafe { lj_logger_free(logger) };
            return Err(err);
        }
        samples.push(elapsed);
    }

    let flush_start = Instant::now();
    unsafe { lj_logger_flush(logger, 60_000) };
    let flush_ns = flush_start.elapsed().as_nanos() as f64;

    let errors = unsafe { lj_logger_async_errors(logger) };
    let dropped = unsafe { lj_logger_async_dropped(logger) };

    unsafe { lj_logger_free(logger) };
    Ok(AsyncResult { samples, flush_ns, errors, dropped })
}

fn last_error() -> String {
    let ptr = lj_error_message();
    if ptr.is_null() {
        return "unknown error".to_string();
    }
    let message = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    if message.is_empty() { "unknown error".to_string() } else { message }
}

fn now_unix_ns() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64
}

fn summarize(mut samples: Vec<u128>) -> Stats {
    assert!(!samples.is_empty(), "no samples collected");
    samples.sort_unstable();
    let count = samples.len();
    let total: u128 = samples.iter().sum();
    let percentile = |pct: f64| {
        let rank = ((pct / 100.0) * count as f64).ceil() as usize;
        let index = rank.saturating_sub(1).min(count - 1);
        samples[index] as f64
    };
    Stats {
        count,
        total: total as f64,
        mean: total as f64 / count as f64,
        p50: percentile(50.0),
        p95: percentile(95.0),
        p99: percentile(99.0),
        min: samples[0] as f64,
        max: samples[count - 1] as f64,
    }
}

fn fmt_dur(ns: f64) -> String {
    if ns >= 1e9 {
        format!("{:.2} s", ns / 1e9)
    } else if ns >= 1e6 {
        format!("{:.2} ms", ns / 1e6)
    } else if ns >= 1e3 {
        format!("{:.2} us", ns / 1e3)
    } else {
        format!("{ns:.0} ns")
    }
}

fn print_table(rows: &[(&str, &Stats)]) {
    println!("{:<36} {:>7} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11}", "path", "calls", "total", "mean", "p50", "p95", "p99", "min", "max");
    println!("{}", "-".repeat(36 + 7 + 11 * 7 + 8));
    for (name, stats) in rows {
        println!(
            "{:<36} {:>7} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11} {:>11}",
            name,
            stats.count,
            fmt_dur(stats.total),
            fmt_dur(stats.mean),
            fmt_dur(stats.p50),
            fmt_dur(stats.p95),
            fmt_dur(stats.p99),
            fmt_dur(stats.min),
            fmt_dur(stats.max),
        );
    }
    let _ = std::io::stdout().flush();
}
