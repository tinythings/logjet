use std::sync::Arc;

use cel::Program;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use prost::Message;

use crate::context::{LogRecordContext, hex_encode, key_values_to_map};
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

    pub fn matches_logs_payload(&self, payload: &[u8]) -> Result<bool, CelError> {
        let batch = ExportLogsServiceRequest::decode(payload).map_err(|e| CelError::Decode(e.to_string()))?;

        for rl in &batch.resource_logs {
            let resource_attrs =
                key_values_to_map(rl.resource.as_ref().map(|r| r.attributes.as_slice()).unwrap_or(&[]));

            let service_name = rl
                .resource
                .as_ref()
                .and_then(|r| r.attributes.iter().find(|a| a.key == "service.name"))
                .and_then(|a| a.value.as_ref())
                .and_then(|v| match &v.value {
                    Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s)) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default();

            for sl in &rl.scope_logs {
                let scope_name = sl.scope.as_ref().map(|s| s.name.clone()).unwrap_or_default();
                let scope_attrs = key_values_to_map(
                    sl.scope.as_ref().map(|s| s.attributes.as_slice()).unwrap_or(&[]),
                );

                for lr in &sl.log_records {
                    let body = lr
                        .body
                        .as_ref()
                        .and_then(|v| match &v.value {
                            Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s)) => {
                                Some(s.clone())
                            }
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

                    if self.eval(&ctx)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn eval(&self, ctx: &LogRecordContext) -> Result<bool, CelError> {
        let cel_ctx = ctx.to_cel_context()?;
        match self.program.execute(&cel_ctx) {
            Ok(cel::Value::Bool(true)) => Ok(true),
            Ok(cel::Value::Bool(false)) => Ok(false),
            Ok(other) => Err(CelError::NotBoolean(format!("{other:?}"))),
            Err(e) => Err(CelError::Compile(e.to_string())),
        }
    }
}
