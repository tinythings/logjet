use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use opentelemetry_proto::tonic::common::v1::any_value::Value as OtelValue;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};

use crate::error::CelError;

pub(crate) struct LogRecordContext {
    pub body: String,
    pub severity_text: String,
    pub severity_number: i64,
    pub event_name: String,
    pub service_name: String,
    pub scope_name: String,
    pub time_unix_nano: i64,
    pub observed_time_unix_nano: i64,
    pub trace_id: String,
    pub span_id: String,
    pub flags: i32,
    pub resource: cel::Value,
    pub scope: cel::Value,
    pub attributes: cel::Value,
}

impl LogRecordContext {
    pub(crate) fn to_cel_context(&self) -> Result<cel::Context<'_>, CelError> {
        let mut ctx = cel::Context::default();
        ctx.add_variable("body", self.body.clone())
            .map_err(|e| CelError::Compile(format!("body: {e}")))?;
        ctx.add_variable("severity_text", self.severity_text.clone())
            .map_err(|e| CelError::Compile(format!("severity_text: {e}")))?;
        ctx.add_variable("severity_number", self.severity_number)
            .map_err(|e| CelError::Compile(format!("severity_number: {e}")))?;
        if !self.event_name.is_empty() {
            ctx.add_variable("event_name", self.event_name.clone())
                .map_err(|e| CelError::Compile(format!("event_name: {e}")))?;
        }
        if !self.service_name.is_empty() {
            ctx.add_variable("service_name", self.service_name.clone())
                .map_err(|e| CelError::Compile(format!("service_name: {e}")))?;
        }
        if !self.scope_name.is_empty() {
            ctx.add_variable("scope_name", self.scope_name.clone())
                .map_err(|e| CelError::Compile(format!("scope_name: {e}")))?;
        }
        ctx.add_variable("time_unix_nano", self.time_unix_nano)
            .map_err(|e| CelError::Compile(format!("time_unix_nano: {e}")))?;
        if self.observed_time_unix_nano > 0 {
            ctx.add_variable("observed_time_unix_nano", self.observed_time_unix_nano)
                .map_err(|e| CelError::Compile(format!("observed_time_unix_nano: {e}")))?;
        }
        if !self.trace_id.is_empty() {
            ctx.add_variable("trace_id", self.trace_id.clone())
                .map_err(|e| CelError::Compile(format!("trace_id: {e}")))?;
        }
        if !self.span_id.is_empty() {
            ctx.add_variable("span_id", self.span_id.clone())
                .map_err(|e| CelError::Compile(format!("span_id: {e}")))?;
        }
        if self.flags != 0 {
            ctx.add_variable("flags", self.flags)
                .map_err(|e| CelError::Compile(format!("flags: {e}")))?;
        }
        ctx.add_variable("resource", self.resource.clone())
            .map_err(|e| CelError::Compile(format!("resource: {e}")))?;
        ctx.add_variable("scope", self.scope.clone())
            .map_err(|e| CelError::Compile(format!("scope: {e}")))?;
        ctx.add_variable("attributes", self.attributes.clone())
            .map_err(|e| CelError::Compile(format!("attributes: {e}")))?;
        Ok(ctx)
    }
}

pub(crate) struct MetricDataPointContext {
    pub metric_name: String,
    pub metric_unit: String,
    pub metric_description: String,
    pub metric_type: String,
    pub value: f64,
    pub count: i64,
    pub sum: f64,
    pub time_unix_nano: i64,
    pub service_name: String,
    pub scope_name: String,
    pub resource: cel::Value,
    pub scope: cel::Value,
    pub attributes: cel::Value,
}

impl MetricDataPointContext {
    pub(crate) fn to_cel_context(&self) -> Result<cel::Context<'_>, CelError> {
        let mut ctx = cel::Context::default();
        ctx.add_variable("metric_name", self.metric_name.clone())
            .map_err(|e| CelError::Compile(format!("metric_name: {e}")))?;
        if !self.metric_unit.is_empty() {
            ctx.add_variable("metric_unit", self.metric_unit.clone())
                .map_err(|e| CelError::Compile(format!("metric_unit: {e}")))?;
        }
        if !self.metric_description.is_empty() {
            ctx.add_variable("metric_description", self.metric_description.clone())
                .map_err(|e| CelError::Compile(format!("metric_description: {e}")))?;
        }
        ctx.add_variable("metric_type", self.metric_type.clone())
            .map_err(|e| CelError::Compile(format!("metric_type: {e}")))?;
        ctx.add_variable("value", self.value)
            .map_err(|e| CelError::Compile(format!("value: {e}")))?;
        ctx.add_variable("count", self.count)
            .map_err(|e| CelError::Compile(format!("count: {e}")))?;
        ctx.add_variable("sum", self.sum)
            .map_err(|e| CelError::Compile(format!("sum: {e}")))?;
        if self.time_unix_nano > 0 {
            ctx.add_variable("time_unix_nano", self.time_unix_nano)
                .map_err(|e| CelError::Compile(format!("time_unix_nano: {e}")))?;
        }
        if !self.service_name.is_empty() {
            ctx.add_variable("service_name", self.service_name.clone())
                .map_err(|e| CelError::Compile(format!("service_name: {e}")))?;
        }
        if !self.scope_name.is_empty() {
            ctx.add_variable("scope_name", self.scope_name.clone())
                .map_err(|e| CelError::Compile(format!("scope_name: {e}")))?;
        }
        ctx.add_variable("resource", self.resource.clone())
            .map_err(|e| CelError::Compile(format!("resource: {e}")))?;
        ctx.add_variable("scope", self.scope.clone())
            .map_err(|e| CelError::Compile(format!("scope: {e}")))?;
        ctx.add_variable("attributes", self.attributes.clone())
            .map_err(|e| CelError::Compile(format!("attributes: {e}")))?;
        Ok(ctx)
    }
}

pub(crate) struct SpanContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
    pub kind: i64,
    pub start_time_unix_nano: i64,
    pub end_time_unix_nano: i64,
    pub duration_ns: i64,
    pub status_code: i64,
    pub status_message: String,
    pub service_name: String,
    pub scope_name: String,
    pub resource: cel::Value,
    pub scope: cel::Value,
    pub attributes: cel::Value,
}

impl SpanContext {
    pub(crate) fn to_cel_context(&self) -> Result<cel::Context<'_>, CelError> {
        let mut ctx = cel::Context::default();
        if !self.trace_id.is_empty() {
            ctx.add_variable("trace_id", self.trace_id.clone())
                .map_err(|e| CelError::Compile(format!("trace_id: {e}")))?;
        }
        ctx.add_variable("span_id", self.span_id.clone())
            .map_err(|e| CelError::Compile(format!("span_id: {e}")))?;
        if !self.parent_span_id.is_empty() {
            ctx.add_variable("parent_span_id", self.parent_span_id.clone())
                .map_err(|e| CelError::Compile(format!("parent_span_id: {e}")))?;
        }
        ctx.add_variable("name", self.name.clone())
            .map_err(|e| CelError::Compile(format!("name: {e}")))?;
        ctx.add_variable("kind", self.kind)
            .map_err(|e| CelError::Compile(format!("kind: {e}")))?;
        if self.start_time_unix_nano > 0 {
            ctx.add_variable("start_time_unix_nano", self.start_time_unix_nano)
                .map_err(|e| CelError::Compile(format!("start_time_unix_nano: {e}")))?;
        }
        if self.end_time_unix_nano > 0 {
            ctx.add_variable("end_time_unix_nano", self.end_time_unix_nano)
                .map_err(|e| CelError::Compile(format!("end_time_unix_nano: {e}")))?;
        }
        if self.duration_ns > 0 {
            ctx.add_variable("duration_ns", self.duration_ns)
                .map_err(|e| CelError::Compile(format!("duration_ns: {e}")))?;
        }
        ctx.add_variable("status_code", self.status_code)
            .map_err(|e| CelError::Compile(format!("status_code: {e}")))?;
        if !self.status_message.is_empty() {
            ctx.add_variable("status_message", self.status_message.clone())
                .map_err(|e| CelError::Compile(format!("status_message: {e}")))?;
        }
        if !self.service_name.is_empty() {
            ctx.add_variable("service_name", self.service_name.clone())
                .map_err(|e| CelError::Compile(format!("service_name: {e}")))?;
        }
        if !self.scope_name.is_empty() {
            ctx.add_variable("scope_name", self.scope_name.clone())
                .map_err(|e| CelError::Compile(format!("scope_name: {e}")))?;
        }
        ctx.add_variable("resource", self.resource.clone())
            .map_err(|e| CelError::Compile(format!("resource: {e}")))?;
        ctx.add_variable("scope", self.scope.clone())
            .map_err(|e| CelError::Compile(format!("scope: {e}")))?;
        ctx.add_variable("attributes", self.attributes.clone())
            .map_err(|e| CelError::Compile(format!("attributes: {e}")))?;
        Ok(ctx)
    }
}

pub(crate) fn extract_service_name(resource_attrs: &[KeyValue]) -> String {
    resource_attrs
        .iter()
        .find(|a| a.key == "service.name")
        .and_then(|a| a.value.as_ref())
        .and_then(|v| match &v.value {
            Some(OtelValue::StringValue(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn key_values_to_cel_map(attrs: &[KeyValue]) -> cel::Value {
    let mut seen: HashSet<String> = HashSet::new();
    let mut map: HashMap<Arc<String>, cel::Value> = HashMap::new();
    for attr in attrs {
        if !seen.insert(attr.key.clone()) {
            continue;
        }
        if let Some(value) = attr.value.as_ref() {
            map.insert(Arc::new(attr.key.clone()), any_value_to_cel(value));
        }
    }
    cel::Value::Map(map.into())
    }


fn any_value_to_cel(value: &AnyValue) -> cel::Value {
    match &value.value {
        Some(OtelValue::StringValue(s)) => cel::Value::String(Arc::new(s.clone())),
        Some(OtelValue::BoolValue(b)) => cel::Value::Bool(*b),
        Some(OtelValue::IntValue(n)) => cel::Value::Int(*n),
        Some(OtelValue::DoubleValue(d)) => cel::Value::Float(*d),
        Some(OtelValue::BytesValue(b)) => {
            cel::Value::String(Arc::new(format!("<{} bytes>", b.len())))
        }
        Some(OtelValue::ArrayValue(a)) => {
            let list: Vec<cel::Value> = a.values.iter().map(any_value_to_cel).collect();
            cel::Value::List(Arc::new(list))
        }
        Some(OtelValue::KvlistValue(k)) => {
            let mut map: HashMap<Arc<String>, cel::Value> = HashMap::new();
            for item in &k.values {
                if let Some(value) = item.value.as_ref() {
                    map.insert(Arc::new(item.key.clone()), any_value_to_cel(value));
                }
            }
            cel::Value::Map(map.into())
        }
        None => cel::Value::String(Arc::new(String::new())),
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
