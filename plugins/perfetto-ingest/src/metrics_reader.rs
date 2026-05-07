//! Parses Perfetto metrics JSON output.
#![allow(dead_code)]

use std::path::Path;

/// One parsed metric from the Perfetto metrics JSON output.
#[derive(Debug, Clone)]
pub struct PerfettoMetric {
    /// Metric name, e.g. "trace_stats" or "android_startup".
    pub name: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Unit, e.g. "ms", "bytes", or empty.
    pub unit: Option<String>,
    /// Scalar value, if the metric is a simple number.
    pub scalar_value: Option<f64>,
    /// String labels attached to the metric (key-value pairs).
    pub labels: Vec<(String, String)>,
    /// Nested sub-metrics (for structured metric outputs).
    pub children: Vec<PerfettoMetric>,
}

/// Parses a Perfetto metrics JSON file into a flat-ish list of metrics.
///
/// The JSON structure from trace_processor is typically:
/// ```json
/// {
///   "metric_name": {
///     "value": 123,
///     "description": "...",
///     "unit": "ms"
///   }
/// }
/// ```
/// or for structured metrics with nested entries.
pub fn parse_metrics_json(path: &Path) -> Result<Vec<PerfettoMetric>, String> {
    let bytes = std::fs::read(path).map_err(|err| format!("failed to read metrics JSON file {}: {err}", path.display()))?;

    let root: serde_json::Value = serde_json::from_slice(&bytes).map_err(|err| format!("failed to parse metrics JSON: {err}"))?;

    let obj = root.as_object().ok_or_else(|| format!("metrics JSON root is not an object: {}", path.display()))?;

    let mut out = Vec::new();
    for (key, value) in obj {
        let metric = parse_metric(key, value);
        out.push(metric);
    }
    Ok(out)
}

fn parse_metric(name: &str, value: &serde_json::Value) -> PerfettoMetric {
    let obj = value.as_object();

    let scalar_value = obj.and_then(|o| o.get("value")).and_then(|v| v.as_f64()).or_else(|| value.as_f64());

    let description = obj.and_then(|o| o.get("description")).and_then(|v| v.as_str()).map(String::from);

    let unit = obj.and_then(|o| o.get("unit")).and_then(|v| v.as_str()).map(String::from);

    let labels = obj
        .map(|o| {
            o.iter()
                .filter(|(k, v)| !matches!(k.as_str(), "value" | "description" | "unit") && v.is_string())
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();

    let children = obj.map(|o| o.iter().filter(|(_, v)| v.is_object()).map(|(k, v)| parse_metric(k, v)).collect()).unwrap_or_default();

    PerfettoMetric { name: name.to_string(), description, unit, scalar_value, labels, children }
}
