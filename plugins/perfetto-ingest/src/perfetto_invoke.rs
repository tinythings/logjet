//! Spawns Perfetto `trace_processor` and captures its output.

use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locates the trace_processor binary.
///
/// Checks `LJD_PERFETTO_TRACE_PROCESSOR` env var first, then falls back to
/// PATH search for `trace_processor` (or `trace_processor_shell`).
#[allow(dead_code)]
pub fn find_trace_processor() -> io::Result<PathBuf> {
    if let Ok(raw) = env::var("LJD_PERFETTO_TRACE_PROCESSOR") {
        let path = PathBuf::from(raw);
        if path.is_file() {
            return Ok(path);
        }
        return Err(io::Error::other(format!("LJD_PERFETTO_TRACE_PROCESSOR is set but not a file: {}", path.display())));
    }

    for name in ["trace_processor", "trace_processor_shell"] {
        if let Some(path) = find_on_path(name) {
            return Ok(path);
        }
    }

    Err(io::Error::other("trace_processor not found. Set LJD_PERFETTO_TRACE_PROCESSOR or install Perfetto tools on PATH."))
}

#[allow(dead_code)]
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Runs `trace_processor export sqlite -o <output> <trace>` and returns the
/// path to the exported SQLite database.
#[allow(dead_code)]
pub fn export_sqlite(trace_file: &Path, tp_path: &Path) -> io::Result<PathBuf> {
    let output = temp_file_path("perfetto-export", "sqlite")?;

    let status = Command::new(tp_path)
        .arg("export")
        .arg("sqlite")
        .arg("-o")
        .arg(&output)
        .arg(trace_file)
        .status()
        .map_err(|err| io::Error::other(format!("failed to spawn trace_processor for sqlite export: {err}")))?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let _ = std::fs::remove_file(&output);
        return Err(io::Error::other(format!("trace_processor export sqlite exited with code {code}")));
    }

    if !output.exists() {
        return Err(io::Error::other("trace_processor export sqlite did not produce output file"));
    }

    Ok(output)
}

/// Runs `trace_processor metrics <trace> --run <metrics> --output json` and
/// returns the path to the captured JSON output. Returns `None` if metrics
/// list is empty.
#[allow(dead_code)]
pub fn export_metrics(trace_file: &Path, tp_path: &Path, metrics: &[&str]) -> io::Result<Option<PathBuf>> {
    if metrics.is_empty() {
        return Ok(None);
    }

    let output = temp_file_path("perfetto-metrics", "json")?;
    let metrics_arg = metrics.join(",");

    let result = Command::new(tp_path)
        .arg("metrics")
        .arg(trace_file)
        .arg("--run")
        .arg(&metrics_arg)
        .arg("--output")
        .arg("json")
        .output()
        .map_err(|err| io::Error::other(format!("failed to spawn trace_processor for metrics: {err}")))?;

    if !result.status.success() {
        let code = result.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&result.stderr);
        let _ = std::fs::remove_file(&output);
        return Err(io::Error::other(format!("trace_processor metrics exited with code {code}: {stderr}")));
    }

    std::fs::write(&output, &result.stdout)?;
    Ok(Some(output))
}

#[allow(dead_code)]
/// Runs `trace_processor server stdio <trace>` and returns a connected
/// `RpcClient`. The process stays alive until the client is shut down.
pub fn start_server(trace_file: &Path, tp_path: &Path) -> io::Result<crate::rpc_client::RpcClient> {
    crate::rpc_client::RpcClient::connect(tp_path, trace_file)
}

fn temp_file_path(prefix: &str, suffix: &str) -> io::Result<PathBuf> {
    let pid = std::process::id();
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_micros();
    let name = format!("{prefix}-{ts}-{pid}.{suffix}");
    Ok(env::temp_dir().join(name))
}
