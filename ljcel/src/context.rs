use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use serde_json::{Map as JsonMap, Value as JsonValue};

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
    pub resource: JsonMap<String, JsonValue>,
    pub scope: JsonMap<String, JsonValue>,
    pub attributes: JsonMap<String, JsonValue>,
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

pub(crate) fn key_values_to_map(attrs: &[KeyValue]) -> JsonMap<String, JsonValue> {
    let mut map = JsonMap::new();
    for attr in attrs {
        if map.contains_key(&attr.key) {
            continue;
        }
        if let Some(value) = attr.value.as_ref()
            && let Some(json) = any_value_to_json(value)
        {
            map.insert(attr.key.clone(), json);
        }
    }
    map
}

pub(crate) fn any_value_to_json(value: &AnyValue) -> Option<JsonValue> {
    match &value.value {
        Some(Value::StringValue(s)) => Some(JsonValue::String(s.clone())),
        Some(Value::BoolValue(b)) => Some(JsonValue::Bool(*b)),
        Some(Value::IntValue(n)) => Some(JsonValue::Number((*n).into())),
        Some(Value::DoubleValue(d)) => serde_json::Number::from_f64(*d).map(JsonValue::Number),
        Some(Value::BytesValue(b)) => Some(JsonValue::String(format!("<{} bytes>", b.len()))),
        Some(Value::ArrayValue(a)) => {
            Some(JsonValue::Array(a.values.iter().filter_map(any_value_to_json).collect()))
        }
        Some(Value::KvlistValue(k)) => {
            let mut obj = JsonMap::new();
            for item in &k.values {
                if let Some(inner) = item.value.as_ref().and_then(any_value_to_json) {
                    obj.insert(item.key.clone(), inner);
                }
            }
            Some(JsonValue::Object(obj))
        }
        None => None,
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
