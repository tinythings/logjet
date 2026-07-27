use std::sync::Arc;

use cel::Program;
use logjet::RecordType;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as AnyValueKind;
use opentelemetry_proto::tonic::metrics::v1::metric::Data as MetricData;
use prost::Message;

use crate::context::{LogRecordContext, MetricDataPointContext, SpanContext, extract_service_name, hex_encode, key_values_to_map};
use crate::error::CelError;

#[derive(Debug, Clone)]
pub struct CelExpression {
    program: Arc<Program>,
    source: String,
}

impl CelExpression {
    pub fn compile(expr: &str) -> Result<Self, CelError> {
        let program = Program::compile(expr).map_err(|e| CelError::Compile(e.to_string()))?;
        Ok(Self { program: Arc::new(program), source: expr.to_string() })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn matches_payload(&self, record_type: RecordType, payload: &[u8]) -> Result<bool, CelError> {
        match record_type {
            RecordType::Logs => self.matches_logs_payload(payload),
            RecordType::Metrics => self.matches_metrics_payload(payload),
            RecordType::Traces => self.matches_traces_payload(payload),
            _ => Ok(true),
        }
    }

    pub fn matches_logs_payload(&self, payload: &[u8]) -> Result<bool, CelError> {
        let batch = ExportLogsServiceRequest::decode(payload).map_err(|e| CelError::Decode(e.to_string()))?;

        for rl in &batch.resource_logs {
            let resource_attrs =
                key_values_to_map(rl.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]));
            let service_name = extract_service_name(
                rl.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]),
            );

            for sl in &rl.scope_logs {
                let scope_name = sl.scope.as_ref().map(|s| s.name.clone()).unwrap_or_default();
                let scope_attrs =
                    key_values_to_map(sl.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]));

                for lr in &sl.log_records {
                    let body = lr
                        .body
                        .as_ref()
                        .and_then(|v| match &v.value {
                            Some(AnyValueKind::StringValue(s)) => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();

                    let ctx = LogRecordContext {
                        body,
                        severity_text: lr.severity_text.clone(),
                        severity_number: lr.severity_number as i64,
                        event_name: lr.event_name.clone(),
                        service_name: service_name.clone(),
                        scope_name: scope_name.clone(),
                        time_unix_nano: lr.time_unix_nano as i64,
                        observed_time_unix_nano: lr.observed_time_unix_nano as i64,
                        trace_id: hex_encode(&lr.trace_id),
                        span_id: hex_encode(&lr.span_id),
                        flags: lr.flags as i32,
                        resource: resource_attrs.clone(),
                        scope: scope_attrs.clone(),
                        attributes: key_values_to_map(&lr.attributes),
                    };

                    if self.eval_log(&ctx)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn matches_metrics_payload(&self, payload: &[u8]) -> Result<bool, CelError> {
        let batch = ExportMetricsServiceRequest::decode(payload).map_err(|e| CelError::Decode(e.to_string()))?;

        for rm in &batch.resource_metrics {
            let resource_attrs =
                key_values_to_map(rm.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]));
            let service_name = extract_service_name(
                rm.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]),
            );

            for sm in &rm.scope_metrics {
                let scope_name = sm.scope.as_ref().map(|s| s.name.clone()).unwrap_or_default();
                let scope_attrs =
                    key_values_to_map(sm.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]));

                for metric in &sm.metrics {
                    let dp_contexts: Vec<MetricDataPointContext> = match &metric.data {
                        Some(MetricData::Gauge(g)) => g.data_points.iter().map(|dp| {
                            let value = dp.value.as_ref().map_or(0.0, |v| match v {
                                opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d) => *d,
                                opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i) => *i as f64,
                            });
                            MetricDataPointContext {
                                metric_name: metric.name.clone(),
                                metric_unit: metric.unit.clone(),
                                metric_description: metric.description.clone(),
                                metric_type: "Gauge".to_string(),
                                value,
                                count: 0,
                                sum: 0.0,
                                time_unix_nano: dp.time_unix_nano as i64,
                                service_name: service_name.clone(),
                                scope_name: scope_name.clone(),
                                resource: resource_attrs.clone(),
                                scope: scope_attrs.clone(),
                                attributes: key_values_to_map(&dp.attributes),
                            }
                        }).collect(),
                        Some(MetricData::Sum(s)) => s.data_points.iter().map(|dp| {
                            let value = dp.value.as_ref().map_or(0.0, |v| match v {
                                opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d) => *d,
                                opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i) => *i as f64,
                            });
                            MetricDataPointContext {
                                metric_name: metric.name.clone(),
                                metric_unit: metric.unit.clone(),
                                metric_description: metric.description.clone(),
                                metric_type: "Sum".to_string(),
                                value,
                                count: 0,
                                sum: 0.0,
                                time_unix_nano: dp.time_unix_nano as i64,
                                service_name: service_name.clone(),
                                scope_name: scope_name.clone(),
                                resource: resource_attrs.clone(),
                                scope: scope_attrs.clone(),
                                attributes: key_values_to_map(&dp.attributes),
                            }
                        }).collect(),
                        Some(MetricData::Histogram(h)) => h.data_points.iter().map(|dp| {
                            MetricDataPointContext {
                                metric_name: metric.name.clone(),
                                metric_unit: metric.unit.clone(),
                                metric_description: metric.description.clone(),
                                metric_type: "Histogram".to_string(),
                                value: 0.0,
                                count: dp.count as i64,
                                sum: dp.sum.unwrap_or(0.0),
                                time_unix_nano: dp.time_unix_nano as i64,
                                service_name: service_name.clone(),
                                scope_name: scope_name.clone(),
                                resource: resource_attrs.clone(),
                                scope: scope_attrs.clone(),
                                attributes: key_values_to_map(&dp.attributes),
                            }
                        }).collect(),
                        Some(MetricData::ExponentialHistogram(eh)) => eh.data_points.iter().map(|dp| {
                            MetricDataPointContext {
                                metric_name: metric.name.clone(),
                                metric_unit: metric.unit.clone(),
                                metric_description: metric.description.clone(),
                                metric_type: "ExponentialHistogram".to_string(),
                                value: 0.0,
                                count: dp.count as i64,
                                sum: dp.sum.unwrap_or(0.0),
                                time_unix_nano: dp.time_unix_nano as i64,
                                service_name: service_name.clone(),
                                scope_name: scope_name.clone(),
                                resource: resource_attrs.clone(),
                                scope: scope_attrs.clone(),
                                attributes: key_values_to_map(&dp.attributes),
                            }
                        }).collect(),
                        Some(MetricData::Summary(s)) => s.data_points.iter().map(|dp| {
                            MetricDataPointContext {
                                metric_name: metric.name.clone(),
                                metric_unit: metric.unit.clone(),
                                metric_description: metric.description.clone(),
                                metric_type: "Summary".to_string(),
                                value: 0.0,
                                count: dp.count as i64,
                                sum: dp.sum,
                                time_unix_nano: dp.time_unix_nano as i64,
                                service_name: service_name.clone(),
                                scope_name: scope_name.clone(),
                                resource: resource_attrs.clone(),
                                scope: scope_attrs.clone(),
                                attributes: key_values_to_map(&dp.attributes),
                            }
                        }).collect(),
                        None => continue,
                    };

                    for ctx in &dp_contexts {
                        if self.eval_metric(ctx)? {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn matches_traces_payload(&self, payload: &[u8]) -> Result<bool, CelError> {
        let batch = ExportTraceServiceRequest::decode(payload).map_err(|e| CelError::Decode(e.to_string()))?;

        for rs in &batch.resource_spans {
            let resource_attrs =
                key_values_to_map(rs.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]));
            let service_name = extract_service_name(
                rs.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]),
            );

            for ss in &rs.scope_spans {
                let scope_name = ss.scope.as_ref().map(|s| s.name.clone()).unwrap_or_default();
                let scope_attrs =
                    key_values_to_map(ss.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]));

                for span in &ss.spans {
                    let duration_ns =
                        if span.end_time_unix_nano > span.start_time_unix_nano {
                            span.end_time_unix_nano - span.start_time_unix_nano
                        } else {
                            0
                        };

                    let (status_code, status_message) = span.status.as_ref().map_or((0, String::new()), |s| {
                        (s.code, s.message.clone())
                    });

                    let ctx = SpanContext {
                        trace_id: hex_encode(&span.trace_id),
                        span_id: hex_encode(&span.span_id),
                        parent_span_id: hex_encode(&span.parent_span_id),
                        name: span.name.clone(),
                        kind: span.kind as i64,
                        start_time_unix_nano: span.start_time_unix_nano as i64,
                        end_time_unix_nano: span.end_time_unix_nano as i64,
                        duration_ns: duration_ns as i64,
                        status_code: status_code as i64,
                        status_message,
                        service_name: service_name.clone(),
                        scope_name: scope_name.clone(),
                        resource: resource_attrs.clone(),
                        scope: scope_attrs.clone(),
                        attributes: key_values_to_map(&span.attributes),
                    };

                    if self.eval_span(&ctx)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn eval_log(&self, ctx: &LogRecordContext) -> Result<bool, CelError> {
        let cel_ctx = ctx.to_cel_context()?;
        self.eval_bool(&cel_ctx)
    }

    fn eval_metric(&self, ctx: &MetricDataPointContext) -> Result<bool, CelError> {
        let cel_ctx = ctx.to_cel_context()?;
        self.eval_bool(&cel_ctx)
    }

    fn eval_span(&self, ctx: &SpanContext) -> Result<bool, CelError> {
        let cel_ctx = ctx.to_cel_context()?;
        self.eval_bool(&cel_ctx)
    }

    fn eval_bool(&self, cel_ctx: &cel::Context<'_>) -> Result<bool, CelError> {
        match self.program.execute(cel_ctx) {
            Ok(cel::Value::Bool(true)) => Ok(true),
            Ok(cel::Value::Bool(false)) => Ok(false),
            Ok(other) => Err(CelError::NotBoolean(format!("{other:?}"))),
            Err(e) => Err(CelError::Compile(e.to_string())),
        }
    }
}
