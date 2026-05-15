use chrono::{TimeZone, Utc};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{Map as JsonMap, Value as JsonValue};

use super::text::{fit_to_width, hex_dump, hex_preview, smart_wrap, text_preview, trim_single_line};
use super::types::{DETAIL_PREVIEW_BYTES, DetailRecord, MODAL_ATTR_ENTRY_LIMIT_PER_KIND};
use logjet::RecordType;

pub(crate) fn format_summary(detail: &DetailRecord, hex_payload: bool) -> String {
    if hex_payload {
        hex_preview(&detail.payload, 32)
    } else if detail.meta.record_type == RecordType::Metrics {
        extract_otlp_metrics_summary(&detail.payload).unwrap_or_else(|| text_preview(&detail.payload, 160))
    } else if detail.meta.record_type == RecordType::Traces {
        extract_otlp_traces_summary(&detail.payload).unwrap_or_else(|| text_preview(&detail.payload, 160))
    } else if let Some(message) = extract_otlp_log_message(&detail.payload) {
        trim_single_line(&message, 160)
    } else {
        text_preview(&detail.payload, 160)
    }
}

pub(super) fn render_detail_lines(detail: &DetailRecord, hex_payload: bool) -> Vec<Line<'static>> {
    let mut lines = vec![
        key_value_line(
            "Record type:",
            record_kind_label(detail.meta.record_type).to_string(),
            Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
        ),
        key_value_line("Sequence:", detail.meta.seq.to_string(), Style::default().fg(Color::White)),
        key_value_line("Timestamp:", format_timestamp(detail.meta.ts_unix_ns), Style::default().fg(Color::White)),
        key_value_line("Payload:", format!("{} bytes", detail.meta.payload_len), Style::default().fg(Color::White)),
        Line::from(""),
    ];

    lines.extend(render_otlp_lines(detail));
    if lines.len() == 5 {
        let preview = if hex_payload { hex_preview(&detail.payload, 64) } else { text_preview(&detail.payload, DETAIL_PREVIEW_BYTES) };
        lines.push(key_value_line("Preview:", preview, Style::default().fg(Color::White)));
    }

    lines
}

fn render_otlp_lines(detail: &DetailRecord) -> Vec<Line<'static>> {
    match detail.meta.record_type {
        RecordType::Logs => render_otlp_log_lines(detail),
        RecordType::Metrics => render_otlp_metrics_lines(detail),
        RecordType::Traces => render_otlp_traces_lines(detail),
        _ => Vec::new(),
    }
}

fn render_otlp_traces_lines(detail: &DetailRecord) -> Vec<Line<'static>> {
    let Ok(batch) = ExportTraceServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![Line::from(vec![
            Span::styled("OTLP traces: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("payload decode failed; showing raw preview"),
        ])];
    };

    let mut span_count = 0usize;
    let mut services = Vec::new();

    for resource_spans in &batch.resource_spans {
        if let Some(resource) = &resource_spans.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !services.iter().any(|existing| existing == service)
                {
                    services.push(service.clone());
                }
            }
        }
        for scope_spans in &resource_spans.scope_spans {
            span_count += scope_spans.spans.len();
        }
    }

    let mut lines = vec![
        key_value_line("OTLP kind:", "traces".to_string(), Style::default().fg(Color::White)),
        key_value_line("Resources:", batch.resource_spans.len().to_string(), Style::default().fg(Color::White)),
        key_value_line("Spans:", span_count.to_string(), Style::default().fg(Color::White)),
    ];

    if !services.is_empty() {
        lines.push(key_value_line("Services:", services.join(", "), Style::default().fg(Color::White)));
    }

    lines
}

fn render_otlp_metrics_lines(detail: &DetailRecord) -> Vec<Line<'static>> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;

    let Ok(batch) = ExportMetricsServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![Line::from(vec![
            Span::styled("OTLP metrics: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("payload decode failed; showing raw preview"),
        ])];
    };

    let mut metric_count = 0usize;
    let mut datapoint_count = 0usize;
    let mut services = Vec::new();

    for resource_metrics in &batch.resource_metrics {
        if let Some(resource) = &resource_metrics.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !services.iter().any(|existing| existing == service)
                {
                    services.push(service.clone());
                }
            }
        }
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                metric_count += 1;
                let dp_len = match metric.data.as_ref() {
                    Some(Data::Gauge(g)) => g.data_points.len(),
                    Some(Data::Sum(s)) => s.data_points.len(),
                    Some(Data::Histogram(h)) => h.data_points.len(),
                    Some(Data::ExponentialHistogram(eh)) => eh.data_points.len(),
                    Some(Data::Summary(s)) => s.data_points.len(),
                    None => 0,
                };
                datapoint_count += dp_len;
            }
        }
    }

    let mut lines = vec![
        key_value_line("OTLP kind:", "metrics".to_string(), Style::default().fg(Color::White)),
        key_value_line("Resources:", batch.resource_metrics.len().to_string(), Style::default().fg(Color::White)),
        key_value_line("Metrics:", metric_count.to_string(), Style::default().fg(Color::White)),
        key_value_line("Datapoints:", datapoint_count.to_string(), Style::default().fg(Color::White)),
    ];

    if !services.is_empty() {
        lines.push(key_value_line("Services:", services.join(", "), Style::default().fg(Color::White)));
    }

    lines
}

fn render_otlp_log_lines(detail: &DetailRecord) -> Vec<Line<'static>> {
    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![Line::from(vec![
            Span::styled("OTLP logs: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("payload decode failed; showing raw preview"),
        ])];
    };

    let mut services = Vec::new();
    let mut severities = Vec::new();
    let mut record_count = 0usize;
    let mut scope_count = 0usize;

    for resource_logs in &batch.resource_logs {
        if let Some(resource) = &resource_logs.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !services.iter().any(|existing| existing == service)
                {
                    services.push(service.clone());
                }
            }
        }

        for scope_logs in &resource_logs.scope_logs {
            scope_count += 1;
            for log_record in &scope_logs.log_records {
                record_count += 1;
                if !log_record.severity_text.is_empty() && !severities.iter().any(|existing| existing == &log_record.severity_text) {
                    severities.push(log_record.severity_text.clone());
                }
            }
        }
    }

    let mut lines = vec![
        key_value_line("OTLP kind:", "logs".to_string(), Style::default().fg(Color::White)),
        key_value_line("Resources:", batch.resource_logs.len().to_string(), Style::default().fg(Color::White)),
        key_value_line("Scopes:", scope_count.to_string(), Style::default().fg(Color::White)),
        key_value_line("Log records:", record_count.to_string(), Style::default().fg(Color::White)),
    ];

    if !services.is_empty() {
        lines.push(key_value_line("Services:", services.join(", "), Style::default().fg(Color::White)));
    }
    if !severities.is_empty() {
        lines.push(key_value_line("Severity:", severities.join(", "), severity_style(severities.first().map(String::as_str).unwrap_or(""))));
    }

    lines
}

pub(crate) fn extract_otlp_log_message(payload: &[u8]) -> Option<String> {
    let batch = ExportLogsServiceRequest::decode(payload).ok()?;
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                if let Some(body) = &log_record.body
                    && let Some(Value::StringValue(message)) = &body.value
                {
                    return Some(message.clone());
                }
            }
        }
    }
    None
}

pub(crate) fn extract_otlp_log_severity(payload: &[u8]) -> Option<String> {
    let batch = ExportLogsServiceRequest::decode(payload).ok()?;
    for resource_logs in &batch.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                if !log_record.severity_text.is_empty() {
                    return Some(log_record.severity_text.clone());
                }
                if let Some(severity) = severity_number_label(log_record.severity_number) {
                    return Some(severity.to_string());
                }
            }
        }
    }
    None
}

pub(crate) fn render_modal_message(detail: &DetailRecord, hex_payload: bool) -> String {
    if let Some(message) = extract_otlp_log_message(&detail.payload) {
        return message;
    }
    if detail.meta.record_type == RecordType::Metrics
        && let Some(message) = extract_otlp_metrics_message(&detail.payload)
    {
        return message;
    }
    if detail.meta.record_type == RecordType::Traces
        && let Some(message) = extract_otlp_traces_message(&detail.payload)
    {
        return message;
    }

    if hex_payload { hex_dump(&detail.payload) } else { String::from_utf8_lossy(&detail.payload).into_owned() }
}

pub(crate) fn extract_otlp_metrics_summary(payload: &[u8]) -> Option<String> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;

    let batch = ExportMetricsServiceRequest::decode(payload).ok()?;
    let mut parts = Vec::new();

    for resource_metrics in &batch.resource_metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                let value = match metric.data.as_ref() {
                    Some(Data::Gauge(g)) => g.data_points.first().and_then(|dp| dp.value.as_ref()).map(format_data_point_value),
                    Some(Data::Sum(s)) => s.data_points.first().and_then(|dp| dp.value.as_ref()).map(format_data_point_value),
                    Some(Data::Histogram(h)) => h.data_points.first().map(|dp| format!("count={}", dp.count)),
                    Some(Data::ExponentialHistogram(eh)) => eh.data_points.first().map(|dp| format!("count={}", dp.count)),
                    Some(Data::Summary(s)) => s.data_points.first().map(|dp| format!("count={}", dp.count)),
                    None => None,
                };
                if let Some(v) = value {
                    parts.push(format!("{}={}{}", metric.name, v, metric.unit));
                } else {
                    parts.push(metric.name.clone());
                }
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

pub(crate) fn extract_otlp_traces_summary(payload: &[u8]) -> Option<String> {
    let batch = ExportTraceServiceRequest::decode(payload).ok()?;
    let mut parts = Vec::new();

    for resource_spans in &batch.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                let status = span.status.as_ref().map(|s| s.code.to_string()).unwrap_or_default();
                let name = &span.name;
                let kind = format_span_kind(span.kind);
                if status.is_empty() {
                    parts.push(format!("{name} ({kind})"));
                } else {
                    parts.push(format!("{name} ({kind}, status={status})"));
                }
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

pub(crate) fn extract_otlp_traces_message(payload: &[u8]) -> Option<String> {
    let batch = ExportTraceServiceRequest::decode(payload).ok()?;
    let mut lines = Vec::new();

    for resource_spans in &batch.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for span in &scope_spans.spans {
                lines.push(format!("Span: {}", span.name));
                if !span.trace_id.is_empty() {
                    lines.push(format!("  Trace ID: {}", hex_encode(&span.trace_id)));
                }
                if !span.span_id.is_empty() {
                    lines.push(format!("  Span ID: {}", hex_encode(&span.span_id)));
                }
                if !span.parent_span_id.is_empty() {
                    lines.push(format!("  Parent Span ID: {}", hex_encode(&span.parent_span_id)));
                }
                lines.push(format!("  Kind: {}", format_span_kind(span.kind)));
                lines.push(format!("  Start: {}", format_timestamp(span.start_time_unix_nano)));
                lines.push(format!("  End:   {}", format_timestamp(span.end_time_unix_nano)));
                if let Some(status) = &span.status {
                    lines.push(format!("  Status: code={} message={}", status.code, status.message));
                }
                for attr in &span.attributes {
                    lines.push(format!("  Attr: {}={}", attr.key, attr.value.as_ref().map(|v| format_any_value(Some(v))).unwrap_or_default()));
                }
                lines.push(String::new());
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn format_span_kind(kind: i32) -> String {
    match kind {
        1 => "Internal".to_string(),
        2 => "Server".to_string(),
        3 => "Client".to_string(),
        4 => "Producer".to_string(),
        5 => "Consumer".to_string(),
        _ => format!("Unknown({kind})"),
    }
}

pub(crate) fn extract_otlp_metrics_message(payload: &[u8]) -> Option<String> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;

    let batch = ExportMetricsServiceRequest::decode(payload).ok()?;
    let mut lines = Vec::new();

    for resource_metrics in &batch.resource_metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                lines.push(format!("Metric: {}", metric.name));
                if !metric.description.is_empty() {
                    lines.push(format!("  Description: {}", metric.description));
                }
                if !metric.unit.is_empty() {
                    lines.push(format!("  Unit: {}", metric.unit));
                }

                match metric.data.as_ref() {
                    Some(Data::Gauge(g)) => {
                        lines.push("  Type: Gauge".to_string());
                        for dp in &g.data_points {
                            lines.push(format!("  - time={}, value={}", format_timestamp(dp.time_unix_nano), dp.value.as_ref().map(format_data_point_value).unwrap_or_default()));
                        }
                    }
                    Some(Data::Sum(s)) => {
                        lines.push(format!("  Type: Sum (monotonic={}, temporality={})", s.is_monotonic, s.aggregation_temporality));
                        for dp in &s.data_points {
                            lines.push(format!("  - time={}, start_time={}, value={}", format_timestamp(dp.time_unix_nano), format_timestamp(dp.start_time_unix_nano), dp.value.as_ref().map(format_data_point_value).unwrap_or_default()));
                        }
                    }
                    Some(Data::Histogram(h)) => {
                        lines.push("  Type: Histogram".to_string());
                        for dp in &h.data_points {
                            lines.push(format!("  - time={}, count={}, sum={:?}", format_timestamp(dp.time_unix_nano), dp.count, dp.sum));
                        }
                    }
                    Some(Data::ExponentialHistogram(eh)) => {
                        lines.push("  Type: ExponentialHistogram".to_string());
                        for dp in &eh.data_points {
                            lines.push(format!("  - time={}, count={}, scale={}", format_timestamp(dp.time_unix_nano), dp.count, dp.scale));
                        }
                    }
                    Some(Data::Summary(s)) => {
                        lines.push("  Type: Summary".to_string());
                        for dp in &s.data_points {
                            lines.push(format!("  - time={}, count={}, sum={}", format_timestamp(dp.time_unix_nano), dp.count, dp.sum));
                        }
                    }
                    None => {}
                }
                lines.push(String::new());
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn format_data_point_value(value: &opentelemetry_proto::tonic::metrics::v1::number_data_point::Value) -> String {
    match value {
        opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(v) => format!("{v}"),
        opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(v) => v.to_string(),
    }
}

pub(super) fn render_modal_footer(detail: &DetailRecord) -> Line<'static> {
    let (size_num, size_unit) = format_size_parts(detail.meta.payload_len);
    Line::from(vec![
        Span::styled(format!("#{}", detail.meta.seq), Style::default().fg(Color::LightGreen)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled(format_timestamp(detail.meta.ts_unix_ns), Style::default().fg(Color::White)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled(record_kind_label(detail.meta.record_type).to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled(size_num, Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::styled(size_unit, Style::default().fg(Color::White)),
    ])
}

pub(super) fn render_modal_footer_placeholder() -> Line<'static> {
    Line::from(vec![
        Span::styled("#", Style::default().fg(Color::LightGreen)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled("", Style::default().fg(Color::White)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled("", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(" | ", Style::default().fg(Color::White)),
        Span::styled("", Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::styled("", Style::default().fg(Color::White)),
    ])
}

pub(crate) fn render_modal_info_entries(detail: &DetailRecord) -> Vec<(String, String)> {
    let mut lines = vec![
        ("type".to_string(), record_kind_label(detail.meta.record_type).to_string()),
        ("seq".to_string(), detail.meta.seq.to_string()),
        ("ts_unix_ns".to_string(), detail.meta.ts_unix_ns.to_string()),
        ("time".to_string(), format_timestamp(detail.meta.ts_unix_ns)),
        ("payload_bytes".to_string(), detail.meta.payload_len.to_string()),
    ];

    match detail.meta.record_type {
        RecordType::Logs => lines.extend(render_modal_log_info_entries(detail)),
        RecordType::Metrics => lines.extend(render_modal_metrics_info_entries(detail)),
        RecordType::Traces => lines.extend(render_modal_traces_info_entries(detail)),
        _ => {}
    }

    lines
}

fn render_modal_traces_info_entries(detail: &DetailRecord) -> Vec<(String, String)> {
    let Ok(batch) = ExportTraceServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![("otlp".to_string(), "decode failed".to_string())];
    };

    let mut entries = vec![("otlp.kind".to_string(), "traces".to_string())];
    entries.push(("resources".to_string(), batch.resource_spans.len().to_string()));

    let mut service_names = Vec::new();
    let mut scope_names = Vec::new();
    let mut span_names = Vec::new();
    let mut span_count = 0usize;
    let mut kind_counts = std::collections::HashMap::new();
    let mut status_counts = std::collections::HashMap::new();

    for resource_spans in &batch.resource_spans {
        if let Some(resource) = &resource_spans.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !service_names.iter().any(|existing| existing == service)
                {
                    service_names.push(service.clone());
                }
            }
        }
        for scope_spans in &resource_spans.scope_spans {
            if let Some(scope) = &scope_spans.scope
                && !scope.name.is_empty()
                && !scope_names.iter().any(|existing| existing == &scope.name)
            {
                scope_names.push(scope.name.clone());
            }
            for span in &scope_spans.spans {
                span_count += 1;
                if !span.name.is_empty() && !span_names.iter().any(|existing| existing == &span.name) {
                    span_names.push(span.name.clone());
                }
                *kind_counts.entry(format_span_kind(span.kind)).or_insert(0usize) += 1;
                if let Some(status) = &span.status {
                    *status_counts.entry(status.code.to_string()).or_insert(0usize) += 1;
                }
            }
        }
    }

    if !service_names.is_empty() {
        entries.push(("service.name".to_string(), service_names.join(", ")));
    }
    if !scope_names.is_empty() {
        entries.push(("scope".to_string(), scope_names.join(", ")));
    }
    entries.push(("spans".to_string(), span_count.to_string()));
    if !span_names.is_empty() {
        entries.push(("span.names".to_string(), span_names.join(", ")));
    }
    for (kind, count) in kind_counts {
        entries.push((format!("span.kind.{kind}"), count.to_string()));
    }
    for (code, count) in status_counts {
        entries.push((format!("span.status.{code}"), count.to_string()));
    }

    entries
}

fn render_modal_metrics_info_entries(detail: &DetailRecord) -> Vec<(String, String)> {
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;

    let Ok(batch) = ExportMetricsServiceRequest::decode(detail.payload.as_slice()) else {
        return vec![("otlp".to_string(), "decode failed".to_string())];
    };

    let mut entries = vec![("otlp.kind".to_string(), "metrics".to_string())];
    entries.push(("resources".to_string(), batch.resource_metrics.len().to_string()));

    let mut metric_names = Vec::new();
    let mut metric_count = 0usize;
    let mut datapoint_count = 0usize;
    let mut service_names = Vec::new();
    let mut scope_names = Vec::new();

    for resource_metrics in &batch.resource_metrics {
        if let Some(resource) = &resource_metrics.resource {
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !service_names.iter().any(|existing| existing == service)
                {
                    service_names.push(service.clone());
                }
            }
        }
        for scope_metrics in &resource_metrics.scope_metrics {
            if let Some(scope) = &scope_metrics.scope
                && !scope.name.is_empty()
                && !scope_names.iter().any(|existing| existing == &scope.name)
            {
                scope_names.push(scope.name.clone());
            }
            for metric in &scope_metrics.metrics {
                metric_count += 1;
                if !metric.name.is_empty() && !metric_names.iter().any(|existing| existing == &metric.name) {
                    metric_names.push(metric.name.clone());
                }
                let dp_len = match metric.data.as_ref() {
                    Some(Data::Gauge(g)) => g.data_points.len(),
                    Some(Data::Sum(s)) => s.data_points.len(),
                    Some(Data::Histogram(h)) => h.data_points.len(),
                    Some(Data::ExponentialHistogram(eh)) => eh.data_points.len(),
                    Some(Data::Summary(s)) => s.data_points.len(),
                    None => 0,
                };
                datapoint_count += dp_len;
            }
        }
    }

    if !service_names.is_empty() {
        entries.push(("service.name".to_string(), service_names.join(", ")));
    }
    if !scope_names.is_empty() {
        entries.push(("scope".to_string(), scope_names.join(", ")));
    }
    entries.push(("metrics".to_string(), metric_count.to_string()));
    entries.push(("datapoints".to_string(), datapoint_count.to_string()));
    if !metric_names.is_empty() {
        entries.push(("metric.names".to_string(), metric_names.join(", ")));
    }

    // Add per-metric detail entries
    for resource_metrics in &batch.resource_metrics {
        for scope_metrics in &resource_metrics.scope_metrics {
            for metric in &scope_metrics.metrics {
                let prefix = format!("metric.{}" , metric.name);
                entries.push((format!("{prefix}.unit"), metric.unit.clone()));
                if !metric.description.is_empty() {
                    entries.push((format!("{prefix}.description"), metric.description.clone()));
                }
                match metric.data.as_ref() {
                    Some(Data::Gauge(g)) => {
                        entries.push((format!("{prefix}.kind"), "Gauge".to_string()));
                        for (i, dp) in g.data_points.iter().enumerate() {
                            let val = dp.value.as_ref().map(format_data_point_value).unwrap_or_default();
                            entries.push((format!("{prefix}.dp{i}.value"), val));
                            entries.push((format!("{prefix}.dp{i}.time"), format_timestamp(dp.time_unix_nano)));
                        }
                    }
                    Some(Data::Sum(s)) => {
                        entries.push((format!("{prefix}.kind"), "Sum".to_string()));
                        entries.push((format!("{prefix}.monotonic"), s.is_monotonic.to_string()));
                        entries.push((format!("{prefix}.temporality"), s.aggregation_temporality.to_string()));
                        for (i, dp) in s.data_points.iter().enumerate() {
                            let val = dp.value.as_ref().map(format_data_point_value).unwrap_or_default();
                            entries.push((format!("{prefix}.dp{i}.value"), val));
                            entries.push((format!("{prefix}.dp{i}.time"), format_timestamp(dp.time_unix_nano)));
                            entries.push((format!("{prefix}.dp{i}.start_time"), format_timestamp(dp.start_time_unix_nano)));
                        }
                    }
                    Some(Data::Histogram(h)) => {
                        entries.push((format!("{prefix}.kind"), "Histogram".to_string()));
                        for (i, dp) in h.data_points.iter().enumerate() {
                            entries.push((format!("{prefix}.dp{i}.count"), dp.count.to_string()));
                            entries.push((format!("{prefix}.dp{i}.time"), format_timestamp(dp.time_unix_nano)));
                        }
                    }
                    Some(Data::ExponentialHistogram(eh)) => {
                        entries.push((format!("{prefix}.kind"), "ExpHistogram".to_string()));
                        for (i, dp) in eh.data_points.iter().enumerate() {
                            entries.push((format!("{prefix}.dp{i}.count"), dp.count.to_string()));
                            entries.push((format!("{prefix}.dp{i}.time"), format_timestamp(dp.time_unix_nano)));
                        }
                    }
                    Some(Data::Summary(s)) => {
                        entries.push((format!("{prefix}.kind"), "Summary".to_string()));
                        for (i, dp) in s.data_points.iter().enumerate() {
                            entries.push((format!("{prefix}.dp{i}.count"), dp.count.to_string()));
                            entries.push((format!("{prefix}.dp{i}.sum"), dp.sum.to_string()));
                            entries.push((format!("{prefix}.dp{i}.time"), format_timestamp(dp.time_unix_nano)));
                        }
                    }
                    None => {}
                }
            }
        }
    }

    entries
}

fn render_modal_log_info_entries(detail: &DetailRecord) -> Vec<(String, String)> {
    let mut lines = vec![];
    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        lines.push(("otlp".to_string(), "decode failed".to_string()));
        return lines;
    };

    let mut service_names = Vec::new();
    let mut scopes = Vec::new();
    let mut severities = Vec::new();
    let mut event_names = Vec::new();
    let mut resource_attr_count = 0usize;
    let mut scope_attr_count = 0usize;
    let mut record_attr_count = 0usize;
    let mut trace_ids = 0usize;
    let mut span_ids = 0usize;
    let mut resource_attr_entries = Vec::new();
    let mut scope_attr_entries = Vec::new();
    let mut record_attr_entries = Vec::new();
    let mut resource_attr_omitted = 0usize;
    let mut scope_attr_omitted = 0usize;
    let mut record_attr_omitted = 0usize;

    for resource_logs in &batch.resource_logs {
        if let Some(resource) = &resource_logs.resource {
            resource_attr_count += resource.attributes.len();
            for attr in &resource.attributes {
                if attr.key == "service.name"
                    && let Some(value) = &attr.value
                    && let Some(Value::StringValue(service)) = &value.value
                    && !service_names.iter().any(|existing| existing == service)
                {
                    service_names.push(service.clone());
                }
                push_modal_attribute_entry(&mut resource_attr_entries, &mut resource_attr_omitted, "resource", &attr.key, attr.value.as_ref());
            }
        }

        for scope_logs in &resource_logs.scope_logs {
            if let Some(scope) = &scope_logs.scope
                && !scope.name.is_empty()
                && !scopes.iter().any(|existing| existing == &scope.name)
            {
                scopes.push(scope.name.clone());
            }
            if let Some(scope) = &scope_logs.scope {
                scope_attr_count += scope.attributes.len();
                for attr in &scope.attributes {
                    push_modal_attribute_entry(&mut scope_attr_entries, &mut scope_attr_omitted, "scope", &attr.key, attr.value.as_ref());
                }
            }
            for record in &scope_logs.log_records {
                record_attr_count += record.attributes.len();
                if !record.severity_text.is_empty() && !severities.iter().any(|existing| existing == &record.severity_text) {
                    severities.push(record.severity_text.clone());
                }
                if !record.event_name.is_empty() && !event_names.iter().any(|existing| existing == &record.event_name) {
                    event_names.push(record.event_name.clone());
                }
                if !record.trace_id.is_empty() {
                    trace_ids += 1;
                }
                if !record.span_id.is_empty() {
                    span_ids += 1;
                }
                for attr in &record.attributes {
                    push_modal_attribute_entry(&mut record_attr_entries, &mut record_attr_omitted, "record", &attr.key, attr.value.as_ref());
                }
            }
        }
    }

    lines.push(("otlp.kind".to_string(), "logs".to_string()));
    lines.push(("resources".to_string(), batch.resource_logs.len().to_string()));
    if !service_names.is_empty() {
        lines.push(("service.name".to_string(), service_names.join(", ")));
    }
    if !scopes.is_empty() {
        lines.push(("scope".to_string(), scopes.join(", ")));
    }
    if !severities.is_empty() {
        lines.push(("severity".to_string(), severities.join(", ")));
    }
    if !event_names.is_empty() {
        lines.push(("event".to_string(), event_names.join(", ")));
    }
    lines.push(("resource.attrs".to_string(), resource_attr_count.to_string()));
    lines.push(("scope.attrs".to_string(), scope_attr_count.to_string()));
    lines.push(("record.attrs".to_string(), record_attr_count.to_string()));
    lines.extend(flatten_modal_entries(resource_attr_entries));
    if resource_attr_omitted > 0 {
        lines.push(("resource.attrs.more".to_string(), format!("{resource_attr_omitted} not shown")));
    }
    lines.extend(flatten_modal_entries(scope_attr_entries));
    if scope_attr_omitted > 0 {
        lines.push(("scope.attrs.more".to_string(), format!("{scope_attr_omitted} not shown")));
    }
    lines.extend(flatten_modal_entries(record_attr_entries));
    if record_attr_omitted > 0 {
        lines.push(("record.attrs.more".to_string(), format!("{record_attr_omitted} not shown")));
    }
    if trace_ids > 0 {
        lines.push(("trace_id".to_string(), format!("{trace_ids} present")));
    }
    if span_ids > 0 {
        lines.push(("span_id".to_string(), format!("{span_ids} present")));
    }

    lines
}

fn flatten_modal_entries(entries: Vec<(String, String, String)>) -> Vec<(String, String)> {
    entries
        .into_iter()
        .map(|(kind, key, value)| if kind.is_empty() && key.is_empty() { (String::new(), value) } else { (format!("{kind}.{key}"), value) })
        .collect()
}

pub(crate) fn parse_export_selection(input: &str, total: usize, selected: usize) -> std::result::Result<(usize, usize), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Range must not be empty.".to_string());
    }
    if trimmed.eq_ignore_ascii_case("all") || trimmed.eq_ignore_ascii_case("a") {
        return Ok((0, total));
    }
    if trimmed.eq_ignore_ascii_case("current") || trimmed.eq_ignore_ascii_case("c") || trimmed == "0" {
        if total == 0 {
            return Ok((0, 0));
        }
        let current = selected.min(total.saturating_sub(1));
        return Ok((current, current + 1));
    }
    if let Some((start, end)) = trimmed.split_once('-') {
        let start = start.trim().parse::<usize>().map_err(|_| "Invalid range start.".to_string())?;
        let end = end.trim().parse::<usize>().map_err(|_| "Invalid range end.".to_string())?;
        if start == 0 || end == 0 {
            return Err("Range is 1-based; values must be >= 1.".to_string());
        }
        if start > end {
            return Err("Range start must be <= end.".to_string());
        }
        if start > total {
            return Ok((total, total));
        }
        return Ok((start - 1, end.min(total)));
    }

    let amount = trimmed.parse::<usize>().map_err(|_| "Range must be all/a, current/c/0, N, or N-N.".to_string())?;
    if amount == 0 {
        return Err("Amount must be >= 1.".to_string());
    }
    Ok((0, amount.min(total)))
}

pub(super) fn export_ndjson_objects(detail: &DetailRecord) -> Vec<JsonValue> {
    if detail.meta.record_type != RecordType::Logs {
        let mut obj = JsonMap::new();
        obj.insert("record_type".to_string(), JsonValue::String(record_kind_label(detail.meta.record_type).to_string()));
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(detail.meta.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&detail.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    }

    let Ok(batch) = ExportLogsServiceRequest::decode(detail.payload.as_slice()) else {
        let mut obj = JsonMap::new();
        obj.insert("timestamp".to_string(), JsonValue::String(format_timestamp(detail.meta.ts_unix_ns)));
        obj.insert("payload".to_string(), JsonValue::String(String::from_utf8_lossy(&detail.payload).to_string()));
        return vec![JsonValue::Object(obj)];
    };

    let mut out = Vec::new();
    for resource_logs in &batch.resource_logs {
        let resource_attrs = resource_logs.resource.as_ref().map(|r| &r.attributes).map(Vec::as_slice).unwrap_or(&[]);
        for scope_logs in &resource_logs.scope_logs {
            let scope_attrs = scope_logs.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]);
            for record in &scope_logs.log_records {
                let mut obj = JsonMap::new();
                if let Some(scope) = &scope_logs.scope {
                    if !scope.name.is_empty() {
                        obj.insert("scope_name".to_string(), JsonValue::String(scope.name.clone()));
                    }
                    if !scope.version.is_empty() {
                        obj.insert("scope_version".to_string(), JsonValue::String(scope.version.clone()));
                    }
                }
                insert_otlp_log_fields(&mut obj, record, detail.meta.ts_unix_ns);
                flatten_otlp_attrs_into_json(&mut obj, resource_attrs);
                flatten_otlp_attrs_into_json(&mut obj, scope_attrs);
                flatten_otlp_attrs_into_json(&mut obj, &record.attributes);
                out.push(JsonValue::Object(obj));
            }
        }
    }
    out
}

fn insert_otlp_log_fields(
    target: &mut JsonMap<String, JsonValue>, record: &opentelemetry_proto::tonic::logs::v1::LogRecord, fallback_ts_unix_ns: u64,
) {
    target.insert(
        "body".to_string(),
        JsonValue::String(record.body.as_ref().map(|v| format_any_value(Some(v))).filter(|s| !s.is_empty()).unwrap_or_default()),
    );
    target.insert("timestamp".to_string(), JsonValue::String(format_timestamp(record.time_unix_nano.max(fallback_ts_unix_ns))));
    if record.observed_time_unix_nano > 0 {
        target.insert("observed_timestamp".to_string(), JsonValue::String(format_timestamp(record.observed_time_unix_nano)));
    }
    if record.severity_number != 0 {
        target.insert("severity_number".to_string(), JsonValue::Number(record.severity_number.into()));
    }
    if !record.severity_text.is_empty() {
        target.insert("severity_text".to_string(), JsonValue::String(record.severity_text.clone()));
    }
    if record.flags != 0 {
        target.insert("flags".to_string(), JsonValue::Number(record.flags.into()));
    }
    if !record.event_name.is_empty() {
        target.insert("event_name".to_string(), JsonValue::String(record.event_name.clone()));
    }
    if !record.trace_id.is_empty() {
        target.insert("trace_id".to_string(), JsonValue::String(hex_encode(&record.trace_id)));
    }
    if !record.span_id.is_empty() {
        target.insert("span_id".to_string(), JsonValue::String(hex_encode(&record.span_id)));
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn flatten_otlp_attrs_into_json(target: &mut JsonMap<String, JsonValue>, attrs: &[KeyValue]) {
    for attr in attrs {
        let key = attr.key.replace('.', "_");
        if target.contains_key(&key) {
            continue;
        }
        let Some(value) = attr.value.as_ref() else {
            continue;
        };
        if let Some(json) = any_value_to_json(value) {
            target.insert(key, json);
        }
    }
}

fn any_value_to_json(value: &AnyValue) -> Option<JsonValue> {
    match &value.value {
        Some(Value::StringValue(text)) => Some(JsonValue::String(text.clone())),
        Some(Value::BoolValue(flag)) => Some(JsonValue::Bool(*flag)),
        Some(Value::IntValue(number)) => Some(JsonValue::Number((*number).into())),
        Some(Value::DoubleValue(number)) => serde_json::Number::from_f64(*number).map(JsonValue::Number),
        Some(Value::BytesValue(bytes)) => Some(JsonValue::String(format!("<{} bytes>", bytes.len()))),
        Some(Value::ArrayValue(array)) => Some(JsonValue::Array(array.values.iter().filter_map(any_value_to_json).collect())),
        Some(Value::KvlistValue(map)) => {
            let mut obj = JsonMap::new();
            for item in &map.values {
                if let Some(inner) = item.value.as_ref().and_then(any_value_to_json) {
                    obj.insert(item.key.clone().replace('.', "_"), inner);
                }
            }
            Some(JsonValue::Object(obj))
        }
        None => None,
    }
}

fn push_modal_attribute_entry(entries: &mut Vec<(String, String, String)>, omitted: &mut usize, kind: &str, key: &str, value: Option<&AnyValue>) {
    let Some(any) = value else {
        if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
            entries.push((kind.to_string(), key.to_string(), "null".to_string()));
        } else {
            *omitted += 1;
        }
        return;
    };

    if let Some(Value::ArrayValue(array)) = &any.value {
        let elements: Vec<String> = array.values.iter().map(|e| format_any_value(Some(e))).filter(|s| !s.is_empty()).collect();
        if elements.is_empty() {
            if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
                entries.push((kind.to_string(), key.to_string(), "\x00N/A".to_string()));
            } else {
                *omitted += 1;
            }
        } else {
            for (i, formatted) in elements.into_iter().enumerate() {
                if entries.len() >= MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
                    *omitted += 1;
                    continue;
                }
                if i == 0 {
                    entries.push((kind.to_string(), key.to_string(), formatted));
                } else {
                    entries.push((String::new(), String::new(), formatted));
                }
            }
        }
        return;
    }

    if entries.len() < MODAL_ATTR_ENTRY_LIMIT_PER_KIND {
        entries.push((kind.to_string(), key.to_string(), format_any_value(value)));
    } else {
        *omitted += 1;
    }
}

pub(super) fn modal_info_line(key: &str, value: String, key_width: usize, value_width: usize) -> Line<'static> {
    let bg = Color::Indexed(30);
    if value == "\x00N/A" {
        let key_style = if is_otlp_attribute_entry(key) && !is_standard_otlp_attribute_entry(key) {
            Style::default().fg(Color::LightCyan).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::LightYellow).bg(bg).add_modifier(Modifier::BOLD)
        };
        return Line::from(vec![
            Span::styled(format!("{key:<width$}: ", width = key_width), key_style),
            Span::styled("N/A", Style::default().fg(Color::Red).bg(bg).add_modifier(Modifier::BOLD)),
        ]);
    }

    let value = trim_single_line(&value, value_width);
    if key.is_empty() {
        return Line::from(vec![
            Span::styled(format!("{:<width$}  ", "", width = key_width), Style::default().bg(bg)),
            Span::styled(value, Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD)),
        ]);
    }

    let (key_style, value_style) = if is_otlp_attribute_entry(key) && !is_standard_otlp_attribute_entry(key) {
        (
            Style::default().fg(Color::LightCyan).bg(bg).add_modifier(Modifier::BOLD),
            Style::default().fg(Color::White).bg(bg).add_modifier(Modifier::BOLD),
        )
    } else {
        (Style::default().fg(Color::LightYellow).bg(bg).add_modifier(Modifier::BOLD), Style::default().fg(Color::White).bg(bg))
    };
    Line::from(vec![Span::styled(format!("{key:<width$}: ", width = key_width), key_style), Span::styled(value, value_style)])
}

fn format_any_value(value: Option<&AnyValue>) -> String {
    let Some(value) = value else {
        return "null".to_string();
    };
    match &value.value {
        Some(Value::StringValue(text)) => text.clone(),
        Some(Value::BoolValue(flag)) => flag.to_string(),
        Some(Value::IntValue(number)) => number.to_string(),
        Some(Value::DoubleValue(number)) => number.to_string(),
        Some(Value::BytesValue(bytes)) => format!("<{} bytes>", bytes.len()),
        Some(Value::ArrayValue(array)) => format!("<array:{}>", array.values.len()),
        Some(Value::KvlistValue(map)) => format!("<map:{}>", map.values.len()),
        None => "null".to_string(),
    }
}

fn is_otlp_attribute_entry(key: &str) -> bool {
    (key.starts_with("resource.") && key != "resource.attrs")
        || (key.starts_with("scope.") && key != "scope.attrs")
        || (key.starts_with("record.") && key != "record.attrs")
        || (key.starts_with("span.") && key != "span.attrs")
}

fn is_standard_otlp_attribute_entry(key: &str) -> bool {
    let Some((_, attr_key)) = key.split_once('.') else {
        return false;
    };

    const STANDARD_PREFIXES: &[&str] = &[
        "service.",
        "telemetry.",
        "host.",
        "os.",
        "process.",
        "container.",
        "k8s.",
        "cloud.",
        "deployment.",
        "device.",
        "faas.",
        "enduser.",
        "server.",
        "client.",
        "http.",
        "url.",
        "network.",
        "net.",
        "rpc.",
        "db.",
        "messaging.",
        "exception.",
        "code.",
        "thread.",
        "gen_ai.",
        "browser.",
        "user_agent.",
        "aws.",
        "gcp.",
        "azure.",
        "vcs.",
    ];

    STANDARD_PREFIXES.iter().any(|prefix| attr_key.starts_with(prefix))
}

fn format_size_parts(bytes: u64) -> (String, String) {
    if bytes >= 1024 * 1024 {
        (format!("{:.1}", bytes as f64 / (1024.0 * 1024.0)), " Mb".to_string())
    } else if bytes >= 1024 {
        (format!("{:.1}", bytes as f64 / 1024.0), " Kb".to_string())
    } else {
        (bytes.to_string(), " Bt".to_string())
    }
}

fn record_kind_label(record_type: RecordType) -> &'static str {
    match record_type {
        RecordType::Logs => "logs",
        RecordType::Metrics => "metrics",
        RecordType::Traces => "traces",
        RecordType::Events => "events",
    }
}

fn key_value_line(label: &str, value: String, value_style: Style) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<12} "), Style::default().fg(Color::Indexed(136))), Span::styled(value, value_style)])
}

pub(super) fn severity_style(value: &str) -> Style {
    let upper = value.to_ascii_uppercase();
    let color = if upper.contains("ERROR") || upper.contains("ERR") || upper.contains("FATAL") {
        Color::LightRed
    } else if upper.contains("WARN") {
        Color::Indexed(214)
    } else if upper.contains("INFO") {
        Color::LightGreen
    } else if upper.contains("DEBUG") {
        Color::LightCyan
    } else if upper.contains("TRACE") {
        Color::LightBlue
    } else {
        Color::White
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

pub(super) fn severity_initial(value: &str) -> String {
    value.chars().find(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_else(|| " ".to_string())
}

pub(super) fn format_timestamp(ts_unix_ns: u64) -> String {
    let secs = (ts_unix_ns / 1_000_000_000) as i64;
    let nanos = (ts_unix_ns % 1_000_000_000) as u32;
    match Utc.timestamp_opt(secs, nanos).single() {
        Some(ts) => ts.format("%Y-%m-%d %H:%M:%S.%f UTC").to_string(),
        None => ts_unix_ns.to_string(),
    }
}

pub(super) fn format_syslog_timestamp(ts_unix_ns: u64) -> String {
    let secs = (ts_unix_ns / 1_000_000_000) as i64;
    let nanos = (ts_unix_ns % 1_000_000_000) as u32;
    match Utc.timestamp_opt(secs, nanos).single() {
        Some(ts) => ts.format("%b %e %H:%M:%S").to_string(),
        None => format!("{ts_unix_ns:<15}").chars().take(15).collect(),
    }
}

fn severity_number_label(value: i32) -> Option<&'static str> {
    match value {
        1..=4 => Some("TRACE"),
        5..=8 => Some("DEBUG"),
        9..=12 => Some("INFO"),
        13..=16 => Some("WARN"),
        17..=20 => Some("ERROR"),
        21..=24 => Some("FATAL"),
        _ => None,
    }
}

pub(super) fn fit_modal_body(message: &str, width: usize) -> (String, u16) {
    let wrapped = smart_wrap(message, width);
    let line_count = wrapped.lines().count() as u16;
    (wrapped, line_count)
}

pub(super) fn fit_line(input: &str, width: usize) -> String {
    fit_to_width(input, width)
}
