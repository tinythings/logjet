//! Maps Perfetto metrics to OTel metrics.

use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as DataPointValue;
use opentelemetry_proto::tonic::metrics::v1::{Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics};
use opentelemetry_proto::tonic::resource::v1::Resource;
use prost::Message;

use crate::metrics_reader::PerfettoMetric;
use crate::timestamp::TimestampConverter;

pub fn map_metrics(
    metrics: &[PerfettoMetric], _converter: &TimestampConverter,
    emit: unsafe fn(ctx: &crate::PerfettoPlugin, record_type: u32, ts_unix_ns: u64, payload: &[u8]), plugin: &crate::PerfettoPlugin,
) -> Result<(), String> {
    let now_ns = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;

    let mut otel_metrics: Vec<Metric> = Vec::new();

    for m in metrics {
        flatten_metrics(m, &mut otel_metrics, &mut Vec::new());
    }

    if otel_metrics.is_empty() {
        return Ok(());
    }

    let resource = Resource {
        attributes: vec![KeyValue {
            key: "service.name".to_string(),
            value: Some(AnyValue { value: Some(Value::StringValue("perfetto".to_string())) }),
        }],
        dropped_attributes_count: 0,
        entity_refs: Vec::new(),
    };

    let scope_metrics = ScopeMetrics {
        scope: Some(opentelemetry_proto::tonic::common::v1::InstrumentationScope {
            name: "perfetto-ingest".to_string(),
            version: String::new(),
            attributes: Vec::new(),
            dropped_attributes_count: 0,
        }),
        metrics: otel_metrics,
        schema_url: String::new(),
    };

    let request = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics { resource: Some(resource), scope_metrics: vec![scope_metrics], schema_url: String::new() }],
    };

    let payload = request.encode_to_vec();
    unsafe { emit(plugin, crate::LJ_INGEST_RECORD_TYPE_METRICS, now_ns, &payload) };

    Ok(())
}

fn flatten_metrics(metric: &PerfettoMetric, out: &mut Vec<Metric>, prefix: &mut Vec<String>) {
    prefix.push(metric.name.clone());
    let full_name = prefix.join(".");

    if let Some(scalar) = metric.scalar_value {
        let attrs: Vec<KeyValue> = metric
            .labels
            .iter()
            .map(|(k, v)| KeyValue { key: k.clone(), value: Some(AnyValue { value: Some(Value::StringValue(v.clone())) }) })
            .collect();

        out.push(Metric {
            name: full_name,
            description: metric.description.clone().unwrap_or_default(),
            unit: metric.unit.clone().unwrap_or_default(),
            data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(opentelemetry_proto::tonic::metrics::v1::Gauge {
                data_points: vec![NumberDataPoint {
                    attributes: attrs,
                    start_time_unix_nano: 0,
                    time_unix_nano: 0,
                    value: Some(DataPointValue::AsDouble(scalar)),
                    flags: 0,
                    exemplars: Vec::new(),
                }],
            })),
            metadata: Vec::new(),
        });
    }

    for child in &metric.children {
        flatten_metrics(child, out, prefix);
    }

    prefix.pop();
}
